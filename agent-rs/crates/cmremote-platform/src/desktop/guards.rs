// Source: CMRemote, clean-room implementation.

//! Shared safety guards every [`DesktopTransportProvider`] must run a
//! request through before doing real work.
//!
//! These checks lift the security contract documented in the parent
//! module out of the trait doc and into code, so the stub
//! [`super::NotSupportedDesktopTransport`] *and* future WebRTC-backed
//! drivers all enforce the same rules. The agent's runtime constructs
//! every provider with the local [`cmremote_wire::ConnectionInfo`]'s
//! `organization_id`; the guards refuse:
//!
//! 1. **Cross-org sessions.** A request whose `org_id` does not match
//!    the agent's own organisation is refused so a server bug or
//!    compromise cannot pivot a viewer onto a device it does not own.
//! 2. **Hostile operator-supplied strings.** Display names,
//!    organisation names, and SignalR connection ids are
//!    length-capped and refused if they contain control characters,
//!    embedded NULs, or lone surrogates so they cannot inject
//!    terminal escapes / shell metacharacters / UI-spoofing strings
//!    into the connected notification or the audit log.
//! 3. **Non-canonical session ids.** The .NET hub only ever sends a
//!    canonical lowercase UUID for `session_id`; any other shape is
//!    treated as an attack and refused.
//!
//! Every rejection produces a [`GuardRejection`] which the provider
//! converts to a [`DesktopTransportResult::failed`] via
//! [`GuardRejection::into_result`]. Sensitive fields (`access_key`)
//! are *never* read by these guards, so a leaked rejection message
//! cannot disclose them.

use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, IceCandidate, IceCredentialType,
    IceServer, IceServerConfig, ProvideIceServersRequest, RemoteControlSessionRequest,
    RestartScreenCasterRequest, SdpAnswer, SdpOffer, MAX_ICE_CREDENTIAL_LEN, MAX_ICE_SERVERS,
    MAX_ICE_URL_LEN, MAX_SDP_BYTES, MAX_SIGNALLING_STRING_LEN, MAX_URLS_PER_ICE_SERVER,
};

/// Maximum byte length permitted for any operator-supplied string
/// (display name, organisation name, SignalR connection id).
///
/// 256 bytes is comfortably above any reasonable on-screen display
/// name and well below the limits enforced by every UI surface that
/// renders these values. A request that exceeds it is treated as a
/// probe and refused outright rather than silently truncated.
pub const MAX_OPERATOR_STRING_LEN: usize = 256;

/// Outcome of running [`check_remote_control`] (and friends) against a
/// request. The provider converts this into a structured
/// [`DesktopTransportResult`] via [`Self::into_result`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardRejection {
    /// `session_id` echoed back to the server in the failure result.
    /// Empty when the rejected request itself had no session id (or
    /// had one that failed validation — in that case we do not echo
    /// the malformed value back).
    pub session_id: String,
    /// Operator-facing message naming the field that failed and the
    /// reason. Never contains the raw operator-supplied value (so a
    /// hostile control character cannot reach a downstream log
    /// renderer) and never references `access_key`.
    pub message: String,
}

impl GuardRejection {
    /// Convert into a wire-level [`DesktopTransportResult::failed`].
    pub fn into_result(self) -> DesktopTransportResult {
        DesktopTransportResult::failed(self.session_id, self.message)
    }
}

/// `true` when `s` is a canonical lowercase UUID matching the
/// `8-4-4-4-12` lower-hex pattern the .NET hub emits.
///
/// Refusing mixed case (or the `{...}` / `urn:uuid:` renderings .NET
/// can also produce on the wire) is intentional: every legitimate
/// caller routes through the hub's `Guid.ToString("D")` which is
/// always lowercase.
fn is_canonical_lowercase_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let want_dash = matches!(i, 8 | 13 | 18 | 23);
        if want_dash {
            if b != b'-' {
                return false;
            }
        } else if !matches!(b, b'0'..=b'9' | b'a'..=b'f') {
            return false;
        }
    }
    true
}

/// Validate a single operator-supplied string. Refuses empty values,
/// values exceeding [`MAX_OPERATOR_STRING_LEN`] bytes, and values
/// containing any non-printable code point (controls including
/// embedded NUL, the DEL character, and Unicode `Cc`/`Cf` general
/// categories such as bidi overrides). Lone surrogates are
/// impossible in `&str` (Rust's UTF-8 invariant excludes them) so
/// rejecting them is automatic.
///
/// Public so other safety surfaces in this crate (notably the
/// [`super::notification`] payload builder, which renders the same
/// operator-supplied strings on the host's screen) reuse exactly
/// the same contract — keeping the security guarantee uniform.
pub fn validate_operator_string(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.len() > MAX_OPERATOR_STRING_LEN {
        return Err(format!(
            "{field} exceeds {MAX_OPERATOR_STRING_LEN}-byte limit"
        ));
    }
    for c in value.chars() {
        // `char::is_control` covers ASCII C0 (incl. NUL), DEL, and
        // the C1 block — exactly the bytes that can drive a terminal
        // escape sequence or break a single-line audit-log entry.
        if c.is_control() {
            return Err(format!("{field} contains a non-printable character"));
        }
        // Refuse the Unicode directional-formatting code points that
        // make up the "Trojan Source" attack family
        // (https://trojansource.codes/). Concretely:
        //   U+200E LEFT-TO-RIGHT MARK
        //   U+200F RIGHT-TO-LEFT MARK
        //   U+202A LEFT-TO-RIGHT EMBEDDING
        //   U+202B RIGHT-TO-LEFT EMBEDDING
        //   U+202C POP DIRECTIONAL FORMATTING
        //   U+202D LEFT-TO-RIGHT OVERRIDE
        //   U+202E RIGHT-TO-LEFT OVERRIDE
        //   U+2066 LEFT-TO-RIGHT ISOLATE
        //   U+2067 RIGHT-TO-LEFT ISOLATE
        //   U+2068 FIRST STRONG ISOLATE
        //   U+2069 POP DIRECTIONAL ISOLATE
        // A hostile org name containing any of these can disguise
        // itself in the on-host connected notification or audit log.
        if matches!(
            c,
            '\u{200E}' | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        ) {
            return Err(format!("{field} contains a bidi-override character"));
        }
    }
    Ok(())
}

/// Cross-org guard. Refuses if the agent has a known
/// `expected_org_id` and the request's `org_id` does not match it
/// case-insensitively. When the agent has *no* org id of its own
/// (only possible during early bootstrapping) the check is skipped —
/// a provider that wants strict mode can wrap the rejection itself.
fn validate_org(expected: Option<&str>, got: &str) -> Result<(), String> {
    if got.is_empty() {
        return Err("org_id must not be empty".into());
    }
    if !is_canonical_lowercase_uuid(got) {
        return Err("org_id is not a canonical lowercase UUID".into());
    }
    let Some(expected) = expected else {
        return Ok(());
    };
    if !expected.eq_ignore_ascii_case(got) {
        // Do not echo either id — it is not secret but logging both
        // helps an attacker confirm probing succeeded. The audit log
        // already captures the request envelope separately.
        return Err("request org_id does not match this agent's organisation".into());
    }
    Ok(())
}

fn validate_session_id(s: &str) -> Result<(), String> {
    if !is_canonical_lowercase_uuid(s) {
        return Err("session_id is not a canonical lowercase UUID".into());
    }
    Ok(())
}

/// Run the security-contract guards against a
/// [`RemoteControlSessionRequest`]. The provider calls this *before*
/// reading any other field (in particular the sensitive `access_key`).
pub fn check_remote_control(
    request: &RemoteControlSessionRequest,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    run(|s| {
        validate_session_id(&request.session_id)?;
        // Echo the session id back only after it has passed the UUID
        // check — never reflect a malformed value.
        *s = request.session_id.clone();
        validate_org(expected_org_id, &request.org_id)?;
        validate_operator_string("user_connection_id", &request.user_connection_id)?;
        validate_operator_string("requester_name", &request.requester_name)?;
        validate_operator_string("org_name", &request.org_name)?;
        Ok(())
    })
}

/// Run the security-contract guards against a
/// [`RestartScreenCasterRequest`]. Each `viewer_ids` entry is also
/// length-capped and printable-only; the .NET hub passes SignalR
/// connection ids here which never legitimately contain controls.
pub fn check_restart_screen_caster(
    request: &RestartScreenCasterRequest,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    run(|s| {
        validate_session_id(&request.session_id)?;
        *s = request.session_id.clone();
        validate_org(expected_org_id, &request.org_id)?;
        validate_operator_string("user_connection_id", &request.user_connection_id)?;
        validate_operator_string("requester_name", &request.requester_name)?;
        validate_operator_string("org_name", &request.org_name)?;
        for (idx, v) in request.viewer_ids.iter().enumerate() {
            // Per-element field name keeps the rejection message
            // pointed at the offending entry without echoing its
            // contents.
            validate_operator_string(&format!("viewer_ids[{idx}]"), v)?;
        }
        Ok(())
    })
}

/// Run the security-contract guards against a
/// [`ChangeWindowsSessionRequest`]. `target_session_id` is a signed
/// integer chosen by the operator UI; we accept any value and let
/// the per-OS driver decide whether the id is reachable.
pub fn check_change_windows_session(
    request: &ChangeWindowsSessionRequest,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    run(|s| {
        validate_session_id(&request.session_id)?;
        *s = request.session_id.clone();
        validate_org(expected_org_id, &request.org_id)?;
        validate_operator_string("viewer_connection_id", &request.viewer_connection_id)?;
        validate_operator_string("user_connection_id", &request.user_connection_id)?;
        validate_operator_string("requester_name", &request.requester_name)?;
        validate_operator_string("org_name", &request.org_name)?;
        Ok(())
    })
}

/// Helper that wraps a per-method check closure and packages any
/// inner `Err` into a [`GuardRejection`]. The closure can write the
/// rejection's `session_id` once it has positively validated the
/// request's session id; on failure the rejection's `session_id` is
/// left empty so a malformed value never reaches the wire.
fn run<F>(body: F) -> Result<(), GuardRejection>
where
    F: FnOnce(&mut String) -> Result<(), String>,
{
    let mut session_id = String::new();
    match body(&mut session_id) {
        Ok(()) => Ok(()),
        Err(message) => Err(GuardRejection {
            session_id,
            message,
        }),
    }
}

// ---------------------------------------------------------------------------
// Slice R7.g — signalling DTO guards.
//
// Same shape as the four method-surface guards above, plus an
// SDP-body length check (capped at `MAX_SDP_BYTES`) and per-field
// length checks on the ICE candidate / sdp-mid strings (capped at
// `MAX_SIGNALLING_STRING_LEN`). Semantic validation of the SDP /
// candidate grammar is left to the WebRTC driver.
// ---------------------------------------------------------------------------

/// Validate an inline SDP body. Refuses an empty payload (the .NET
/// hub never sends one — an end-of-negotiation marker uses a
/// dedicated DTO type) and any payload exceeding [`MAX_SDP_BYTES`].
/// The body itself is not echoed into the rejection message — only
/// its length — so a hostile SDP cannot poison the audit log.
fn validate_sdp(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.len() > MAX_SDP_BYTES {
        return Err(format!("{field} exceeds {MAX_SDP_BYTES}-byte limit"));
    }
    Ok(())
}

/// Validate a per-line signalling string (an ICE candidate line or
/// an `sdp-mid`). Refuses values exceeding
/// [`MAX_SIGNALLING_STRING_LEN`] bytes. Empty is allowed for the
/// candidate field (end-of-candidates marker) so the caller decides
/// per-field whether emptiness is acceptable.
fn validate_signalling_string_allowing_empty(field: &str, value: &str) -> Result<(), String> {
    if value.len() > MAX_SIGNALLING_STRING_LEN {
        return Err(format!(
            "{field} exceeds {MAX_SIGNALLING_STRING_LEN}-byte limit"
        ));
    }
    // Allow embedded controls inside the candidate / sdp-mid line —
    // the SDP grammar uses CR/LF and tab — but reject the
    // bidi-override range, which has no place in a transport line.
    for c in value.chars() {
        if matches!(
            c,
            '\u{200E}' | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        ) {
            return Err(format!("{field} contains a bidi-override character"));
        }
    }
    Ok(())
}

fn check_signalling_envelope<F>(
    session_id: &str,
    org_id: &str,
    viewer_connection_id: &str,
    requester_name: &str,
    org_name: &str,
    expected_org_id: Option<&str>,
    body: F,
) -> Result<(), GuardRejection>
where
    F: FnOnce() -> Result<(), String>,
{
    run(|s| {
        validate_session_id(session_id)?;
        *s = session_id.to_string();
        validate_org(expected_org_id, org_id)?;
        validate_operator_string("viewer_connection_id", viewer_connection_id)?;
        validate_operator_string("requester_name", requester_name)?;
        validate_operator_string("org_name", org_name)?;
        body()
    })
}

/// Run the security-contract guards against an [`SdpOffer`]. Called
/// by the desktop-transport provider *before* the SDP body itself is
/// parsed by the WebRTC layer.
pub fn check_sdp_offer(
    request: &SdpOffer,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    check_signalling_envelope(
        &request.session_id,
        &request.org_id,
        &request.viewer_connection_id,
        &request.requester_name,
        &request.org_name,
        expected_org_id,
        || validate_sdp("sdp", &request.sdp),
    )
}

/// Run the security-contract guards against an [`SdpAnswer`]. Same
/// shape as [`check_sdp_offer`].
pub fn check_sdp_answer(
    request: &SdpAnswer,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    check_signalling_envelope(
        &request.session_id,
        &request.org_id,
        &request.viewer_connection_id,
        &request.requester_name,
        &request.org_name,
        expected_org_id,
        || validate_sdp("sdp", &request.sdp),
    )
}

/// Run the security-contract guards against an [`IceCandidate`].
/// Allows an empty `candidate` line (RFC 8838 end-of-candidates
/// marker) and treats the `sdp_mid` length cap as additive on top
/// of the envelope checks.
pub fn check_ice_candidate(
    request: &IceCandidate,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    check_signalling_envelope(
        &request.session_id,
        &request.org_id,
        &request.viewer_connection_id,
        &request.requester_name,
        &request.org_name,
        expected_org_id,
        || {
            validate_signalling_string_allowing_empty("candidate", &request.candidate)?;
            if let Some(mid) = request.sdp_mid.as_deref() {
                validate_signalling_string_allowing_empty("sdp_mid", mid)?;
            }
            Ok(())
        },
    )
}

// ---------------------------------------------------------------------------
// Slice R7.i — ICE / TURN server configuration guard.
//
// `IceServerConfig` is delivered to the agent before the WebRTC peer
// connection starts gathering candidates. The guard refuses any
// shape that the eventual driver could not safely consume:
//
// 1. URL scheme allow-list (`stun:`, `stuns:`, `turn:`, `turns:`).
//    No `http(s)://`, no `file://`, no scheme-relative `//host`.
// 2. Per-config and per-server count caps and per-URL byte cap, so
//    a hostile config cannot exhaust the resolver budget.
// 3. Operator-string sanitisation on `username` (refuses control
//    characters / NUL / DEL / bidi-overrides — the same contract
//    every other operator-supplied string in this module follows).
// 4. Length cap + hostile-byte refusal on the **sensitive**
//    `credential`. The credential value is never echoed into the
//    rejection message — only its field name and the policy that
//    refused it.
// 5. Fail-closed refusal of `IceCredentialType::Oauth` until the
//    OAuth credential pipeline lands; the wire understands the
//    discriminator but the driver does not yet honour it.
// ---------------------------------------------------------------------------

/// Allow-list of URL schemes an `IceServer.urls` entry may use.
/// Anything else (`http://`, `file://`, scheme-relative `//host`,
/// or a bare host:port without a scheme) is treated as a probe and
/// refused.
const ICE_URL_SCHEMES: [&str; 4] = ["stun:", "stuns:", "turn:", "turns:"];

/// Returns the matching scheme prefix when `url` starts with one of
/// [`ICE_URL_SCHEMES`], or `None` otherwise.
fn ice_url_scheme(url: &str) -> Option<&'static str> {
    ICE_URL_SCHEMES
        .iter()
        .copied()
        .find(|scheme| url.len() > scheme.len() && url.as_bytes().starts_with(scheme.as_bytes()))
}

/// Validate a single ICE-server URL. Refuses an empty value, a
/// value exceeding [`MAX_ICE_URL_LEN`], a value that does not start
/// with one of the four allow-listed schemes, and any value
/// containing a control character / NUL / DEL / Unicode
/// bidi-override / ASCII whitespace (no legitimate ICE URL contains
/// inline whitespace and embedded controls would let a hostile
/// config split the URL across log lines).
fn validate_ice_url(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.len() > MAX_ICE_URL_LEN {
        return Err(format!("{field} exceeds {MAX_ICE_URL_LEN}-byte limit"));
    }
    if ice_url_scheme(value).is_none() {
        // Do not echo the URL — log only the field name and the
        // policy that refused it. A hostile URL can include
        // homoglyphs that confuse a downstream log renderer.
        return Err(format!(
            "{field} scheme is not in the ICE allow-list (stun:, stuns:, turn:, turns:)"
        ));
    }
    for c in value.chars() {
        if c.is_control() || c == '\u{007F}' {
            return Err(format!("{field} contains a non-printable character"));
        }
        if c.is_whitespace() {
            return Err(format!("{field} contains whitespace"));
        }
        if matches!(
            c,
            '\u{200E}' | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        ) {
            return Err(format!("{field} contains a bidi-override character"));
        }
    }
    Ok(())
}

/// Validate an `IceServer.credential`. The value is **sensitive** —
/// the rejection message MUST NOT contain it under any branch.
/// Length-capped at [`MAX_ICE_CREDENTIAL_LEN`] and refused if it
/// contains a control character / NUL / DEL / Unicode bidi-override.
fn validate_ice_credential(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.len() > MAX_ICE_CREDENTIAL_LEN {
        return Err(format!(
            "{field} exceeds {MAX_ICE_CREDENTIAL_LEN}-byte limit"
        ));
    }
    for c in value.chars() {
        if c.is_control() || c == '\u{007F}' {
            // Pin the message to a fixed shape that cannot
            // inadvertently include the credential's bytes.
            return Err(format!("{field} contains a non-printable character"));
        }
        if matches!(
            c,
            '\u{200E}' | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        ) {
            return Err(format!("{field} contains a bidi-override character"));
        }
    }
    Ok(())
}

fn validate_single_ice_server(idx: usize, server: &IceServer) -> Result<(), String> {
    let prefix = format!("ice_servers[{idx}]");

    if server.urls.is_empty() {
        return Err(format!("{prefix}.urls must contain at least one entry"));
    }
    if server.urls.len() > MAX_URLS_PER_ICE_SERVER {
        return Err(format!(
            "{prefix}.urls exceeds {MAX_URLS_PER_ICE_SERVER}-entry limit"
        ));
    }
    for (uidx, url) in server.urls.iter().enumerate() {
        validate_ice_url(&format!("{prefix}.urls[{uidx}]"), url)?;
    }

    // Per W3C `RTCIceServer`: a `turn:` / `turns:` URL requires a
    // username + credential, while a `stun:` / `stuns:` URL accepts
    // neither. Enforce that here so the driver never has to deal
    // with a malformed half-credentialled config.
    let any_turn = server
        .urls
        .iter()
        .any(|u| matches!(ice_url_scheme(u), Some("turn:") | Some("turns:")));
    let any_stun = server
        .urls
        .iter()
        .any(|u| matches!(ice_url_scheme(u), Some("stun:") | Some("stuns:")));

    if any_turn {
        let username = server
            .username
            .as_deref()
            .ok_or_else(|| format!("{prefix}.username is required for turn(s) URLs"))?;
        validate_operator_string(&format!("{prefix}.username"), username)?;
        let credential = server
            .credential
            .as_deref()
            .ok_or_else(|| format!("{prefix}.credential is required for turn(s) URLs"))?;
        validate_ice_credential(&format!("{prefix}.credential"), credential)?;
    } else if any_stun {
        if server.username.is_some() {
            return Err(format!(
                "{prefix}.username is not permitted for plain stun(s) URLs"
            ));
        }
        if server.credential.is_some() {
            return Err(format!(
                "{prefix}.credential is not permitted for plain stun(s) URLs"
            ));
        }
    }

    if matches!(server.credential_type, IceCredentialType::Oauth) {
        // The wire understands the discriminator so a future
        // deployment can opt in without a contract bump, but the
        // initial agent fails closed — see the slice R7.i contract.
        return Err(format!(
            "{prefix}.credential_type \"Oauth\" is not implemented by this agent"
        ));
    }

    Ok(())
}

/// Run the security-contract guards against an [`IceServerConfig`].
/// Called by the desktop-transport provider *before* any URL is
/// handed to the WebRTC stack's resolver.
///
/// The rejection's `session_id` field is left empty: an
/// `IceServerConfig` is delivered out-of-band to the per-session
/// negotiation so the caller pairs the rejection with the session
/// id it knows about, rather than this guard reflecting a value it
/// cannot independently validate.
pub fn check_ice_server_config(config: &IceServerConfig) -> Result<(), GuardRejection> {
    run(|_| {
        if config.ice_servers.len() > MAX_ICE_SERVERS {
            return Err(format!("ice_servers exceeds {MAX_ICE_SERVERS}-entry limit"));
        }
        for (idx, server) in config.ice_servers.iter().enumerate() {
            validate_single_ice_server(idx, server)?;
        }
        // `ice_transport_policy` is an enum with only safe variants;
        // nothing extra to validate.
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Slice R7.j — `ProvideIceServers` request guard.
//
// Wraps the slice R7.b operator-identity envelope check (cross-org,
// canonical-UUID `session_id`, sanitised operator strings) and the
// slice R7.i `IceServerConfig` check into one helper, so the
// dispatch handler can refuse a hostile request with a single call
// before the embedded URL list reaches any downstream parser.
//
// Note: the access_key field is intentionally NOT read by the guard
// — its only role is to be matched against the per-session cache
// the (future) WebRTC driver maintains. Refusing to read it here
// guarantees the rejection message can never accidentally echo the
// secret.
// ---------------------------------------------------------------------------

/// Run the security-contract guards against a
/// [`ProvideIceServersRequest`]. Refuses cross-org / hostile
/// operator strings / non-canonical session id at the same gate as
/// the four method-surface methods, then delegates the embedded
/// [`IceServerConfig`] to the same per-server checks
/// [`check_ice_server_config`] applies (URL allow-list, length caps,
/// TURN credential pairing, sensitive-credential redaction in the
/// rejection message, and `Oauth` fail-closed).
///
/// The rejection's `session_id` is set only after the request's
/// `session_id` has positively passed the canonical-UUID check —
/// same shape every other guard in this module follows. The
/// sensitive `access_key` is never read by the guard so a leaked
/// rejection message cannot disclose it.
pub fn check_provide_ice_servers(
    request: &ProvideIceServersRequest,
    expected_org_id: Option<&str>,
) -> Result<(), GuardRejection> {
    run(|s| {
        validate_session_id(&request.session_id)?;
        *s = request.session_id.clone();
        validate_org(expected_org_id, &request.org_id)?;
        validate_operator_string("viewer_connection_id", &request.viewer_connection_id)?;
        validate_operator_string("requester_name", &request.requester_name)?;
        validate_operator_string("org_name", &request.org_name)?;

        // Per-config cap — same check `check_ice_server_config`
        // applies, inlined so the rejection carries the
        // `session_id` we have just validated (the standalone
        // helper deliberately leaves it empty because it is
        // called out-of-band).
        let config = &request.ice_server_config;
        if config.ice_servers.len() > MAX_ICE_SERVERS {
            return Err(format!("ice_servers exceeds {MAX_ICE_SERVERS}-entry limit"));
        }
        for (idx, server) in config.ice_servers.iter().enumerate() {
            validate_single_ice_server(idx, server)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmremote_wire::InvokeCtrlAltDelRequest;

    const VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";
    const VALID_ORG_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    fn rc() -> RemoteControlSessionRequest {
        RemoteControlSessionRequest {
            session_id: VALID_SESSION_ID.into(),
            access_key: "secret-access-key".into(),
            user_connection_id: "viewer-1".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
        }
    }

    #[test]
    fn canonical_uuid_helper_accepts_lowercase_and_rejects_others() {
        assert!(is_canonical_lowercase_uuid(VALID_SESSION_ID));
        // Mixed case
        assert!(!is_canonical_lowercase_uuid(
            "11111111-2222-3333-4444-AAAAAAAAAAAA"
        ));
        // Braces
        assert!(!is_canonical_lowercase_uuid(
            "{11111111-2222-3333-4444-555555555555}"
        ));
        // Wrong length
        assert!(!is_canonical_lowercase_uuid("not-a-uuid"));
        // Wrong dash positions
        assert!(!is_canonical_lowercase_uuid(
            "111111112-222-3333-4444-555555555555"
        ));
        // Empty
        assert!(!is_canonical_lowercase_uuid(""));
    }

    #[test]
    fn happy_path_passes_for_remote_control() {
        check_remote_control(&rc(), Some(VALID_ORG_ID)).expect("happy path");
    }

    #[test]
    fn cross_org_request_is_refused() {
        let mut req = rc();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert_eq!(r.session_id, VALID_SESSION_ID);
        assert!(r.message.contains("organisation"), "{}", r.message);
        // Must not echo either id verbatim.
        assert!(!r.message.contains(VALID_ORG_ID));
        assert!(!r.message.contains("ffffffff"));
    }

    #[test]
    fn request_org_id_must_be_a_canonical_uuid() {
        let mut req = rc();
        req.org_id = "not-a-uuid".into();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("org_id"));
    }

    #[test]
    fn empty_org_id_is_refused_even_when_agent_has_no_expected_org() {
        let mut req = rc();
        req.org_id = String::new();
        let r = check_remote_control(&req, None).unwrap_err();
        assert!(r.message.contains("org_id"));
    }

    #[test]
    fn agent_without_expected_org_skips_cross_org_check_but_still_validates_format() {
        // No expected org → the format check still runs; a properly
        // formatted org id passes.
        check_remote_control(&rc(), None).expect("format-only path");
    }

    #[test]
    fn non_uuid_session_id_is_refused_and_does_not_echo_value() {
        let mut req = rc();
        req.session_id = "DROP TABLE sessions".into();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        // The malformed session id must NOT be echoed back to the
        // wire — that prevents log poisoning via the result frame.
        assert_eq!(r.session_id, "");
        assert!(r.message.contains("session_id"));
    }

    #[test]
    fn control_character_in_requester_name_is_refused() {
        let mut req = rc();
        req.requester_name = "Alice\u{1B}[31mRed".into(); // ANSI escape
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("requester_name"));
        assert!(r.message.contains("non-printable"));
        // Session id is valid → echoed back.
        assert_eq!(r.session_id, VALID_SESSION_ID);
    }

    #[test]
    fn embedded_nul_in_org_name_is_refused() {
        let mut req = rc();
        req.org_name = "Ac\0me".into();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("org_name"));
    }

    #[test]
    fn bidi_override_in_requester_name_is_refused() {
        let mut req = rc();
        // U+202E RIGHT-TO-LEFT OVERRIDE — the classic Trojan Source
        // attack character.
        req.requester_name = "Alice\u{202E}".into();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("bidi-override"), "{}", r.message);
    }

    #[test]
    fn empty_requester_name_is_refused() {
        let mut req = rc();
        req.requester_name = String::new();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("requester_name"));
    }

    #[test]
    fn over_length_operator_string_is_refused() {
        let mut req = rc();
        req.requester_name = "a".repeat(MAX_OPERATOR_STRING_LEN + 1);
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("requester_name"));
        assert!(r.message.contains("limit"));
        // Crucially, the rejection message itself must not contain
        // the offending value (which could be megabytes).
        assert!(!r.message.contains(&"a".repeat(64)));
    }

    #[test]
    fn restart_screen_caster_validates_each_viewer_id() {
        let req = RestartScreenCasterRequest {
            viewer_ids: vec!["v1".into(), "v\0bad".into()],
            session_id: VALID_SESSION_ID.into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: VALID_ORG_ID.into(),
        };
        let r = check_restart_screen_caster(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("viewer_ids[1]"), "{}", r.message);
    }

    #[test]
    fn restart_screen_caster_happy_path() {
        let req = RestartScreenCasterRequest {
            viewer_ids: vec!["v1".into(), "v2".into()],
            session_id: VALID_SESSION_ID.into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: VALID_ORG_ID.into(),
        };
        check_restart_screen_caster(&req, Some(VALID_ORG_ID)).expect("happy path");
    }

    #[test]
    fn change_windows_session_validates_viewer_connection_id() {
        let req = ChangeWindowsSessionRequest {
            viewer_connection_id: "viewer\u{1B}[H".into(),
            session_id: VALID_SESSION_ID.into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: VALID_ORG_ID.into(),
            target_session_id: 1,
        };
        let r = check_change_windows_session(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("viewer_connection_id"), "{}", r.message);
    }

    #[test]
    fn change_windows_session_happy_path_includes_negative_target() {
        let req = ChangeWindowsSessionRequest {
            viewer_connection_id: "v".into(),
            session_id: VALID_SESSION_ID.into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: VALID_ORG_ID.into(),
            target_session_id: -1,
        };
        check_change_windows_session(&req, Some(VALID_ORG_ID)).expect("happy path");
    }

    #[test]
    fn invoke_ctrl_alt_del_request_has_no_fields_to_validate() {
        // The unit struct exists only so the dispatcher can route by
        // type. The guards module deliberately exposes no
        // `check_invoke_ctrl_alt_del` — there is nothing to check.
        let _ = InvokeCtrlAltDelRequest;
    }

    #[test]
    fn rejection_messages_never_disclose_access_key() {
        let mut req = rc();
        req.requester_name = "\0bad".into();
        let r = check_remote_control(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(
            !r.message.contains("secret-access-key"),
            "rejection leaked access_key: {}",
            r.message
        );
    }

    #[test]
    fn into_result_produces_failure_with_session_id_and_message() {
        let g = GuardRejection {
            session_id: VALID_SESSION_ID.into(),
            message: "boom".into(),
        };
        let r = g.into_result();
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        assert_eq!(r.error_message.as_deref(), Some("boom"));
    }

    // -----------------------------------------------------------------
    // Slice R7.g — signalling DTO guard tests.
    // -----------------------------------------------------------------

    fn offer() -> SdpOffer {
        SdpOffer {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            kind: cmremote_wire::SdpKind::Offer,
            sdp: "v=0\r\n".into(),
        }
    }

    fn answer() -> SdpAnswer {
        SdpAnswer {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            kind: cmremote_wire::SdpKind::Answer,
            sdp: "v=0\r\n".into(),
        }
    }

    fn ice() -> IceCandidate {
        IceCandidate {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        }
    }

    #[test]
    fn sdp_offer_happy_path_passes() {
        check_sdp_offer(&offer(), Some(VALID_ORG_ID)).expect("happy path");
    }

    #[test]
    fn sdp_answer_happy_path_passes() {
        check_sdp_answer(&answer(), Some(VALID_ORG_ID)).expect("happy path");
    }

    #[test]
    fn ice_candidate_happy_path_passes() {
        check_ice_candidate(&ice(), Some(VALID_ORG_ID)).expect("happy path");
    }

    #[test]
    fn ice_candidate_end_of_candidates_marker_is_accepted() {
        let mut req = ice();
        req.candidate = String::new();
        req.sdp_mid = None;
        req.sdp_mline_index = None;
        check_ice_candidate(&req, Some(VALID_ORG_ID))
            .expect("end-of-candidates marker is RFC 8838-legal");
    }

    #[test]
    fn cross_org_sdp_offer_is_refused() {
        let mut req = offer();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = check_sdp_offer(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("organisation"), "{}", r.message);
    }

    #[test]
    fn empty_sdp_body_is_refused() {
        let mut req = offer();
        req.sdp = String::new();
        let r = check_sdp_offer(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("sdp"), "{}", r.message);
    }

    #[test]
    fn over_length_sdp_body_is_refused_without_echoing_body() {
        let mut req = offer();
        req.sdp = "v".repeat(cmremote_wire::MAX_SDP_BYTES + 1);
        let r = check_sdp_offer(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("sdp"), "{}", r.message);
        assert!(r.message.contains("limit"), "{}", r.message);
        // The rejection MUST NOT echo the offending body — that would
        // turn a hostile SDP into a log-amplification vector.
        assert!(!r.message.contains(&"v".repeat(64)), "{}", r.message);
    }

    #[test]
    fn over_length_ice_candidate_is_refused() {
        let mut req = ice();
        req.candidate = "c".repeat(cmremote_wire::MAX_SIGNALLING_STRING_LEN + 1);
        let r = check_ice_candidate(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("candidate"), "{}", r.message);
        assert!(r.message.contains("limit"), "{}", r.message);
    }

    #[test]
    fn malformed_session_id_in_signalling_does_not_echo_value() {
        let mut req = offer();
        req.session_id = "DROP TABLE sessions".into();
        let r = check_sdp_offer(&req, Some(VALID_ORG_ID)).unwrap_err();
        // Same invariant slice R7.b pinned for the four method-surface
        // requests: a malformed session id is never reflected back.
        assert_eq!(r.session_id, "");
        assert!(r.message.contains("session_id"), "{}", r.message);
    }

    #[test]
    fn bidi_override_in_ice_candidate_is_refused() {
        let mut req = ice();
        req.candidate = "candidate:1\u{202E}".into();
        let r = check_ice_candidate(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("bidi-override"), "{}", r.message);
    }

    #[test]
    fn bidi_override_in_sdp_mid_is_refused() {
        let mut req = ice();
        req.sdp_mid = Some("0\u{202E}".into());
        let r = check_ice_candidate(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("sdp_mid"), "{}", r.message);
    }

    // -----------------------------------------------------------------
    // Slice R7.i — ICE / TURN server configuration tests.
    // -----------------------------------------------------------------

    fn turn_server() -> IceServer {
        IceServer {
            urls: vec![
                "turn:turn.example.org:3478?transport=udp".into(),
                "turns:turn.example.org:5349?transport=tcp".into(),
            ],
            username: Some("agent-bob".into()),
            credential: Some("hunter2".into()),
            credential_type: IceCredentialType::Password,
        }
    }

    fn stun_server() -> IceServer {
        IceServer {
            urls: vec!["stun:stun.example.org:3478".into()],
            username: None,
            credential: None,
            credential_type: IceCredentialType::Password,
        }
    }

    fn ok_ice_config() -> IceServerConfig {
        IceServerConfig {
            ice_servers: vec![stun_server(), turn_server()],
            ice_transport_policy: cmremote_wire::IceTransportPolicy::All,
        }
    }

    #[test]
    fn happy_path_passes_for_ice_server_config() {
        check_ice_server_config(&ok_ice_config()).expect("happy path");
    }

    #[test]
    fn empty_ice_server_list_is_accepted_as_a_lan_only_config() {
        // An empty list is a meaningful — if narrow — config that
        // limits the WebRTC stack to host candidates. The guard
        // accepts it; the eventual driver decides whether to warn.
        let cfg = IceServerConfig {
            ice_servers: vec![],
            ice_transport_policy: cmremote_wire::IceTransportPolicy::All,
        };
        check_ice_server_config(&cfg).expect("empty list is valid");
    }

    #[test]
    fn over_max_ice_servers_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers = (0..(MAX_ICE_SERVERS + 1)).map(|_| stun_server()).collect();
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("ice_servers"), "{}", r.message);
        assert!(r.message.contains("limit"), "{}", r.message);
    }

    #[test]
    fn over_max_urls_per_server_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].urls = (0..(MAX_URLS_PER_ICE_SERVER + 1))
            .map(|i| format!("stun:stun{i}.example.org"))
            .collect();
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("urls"), "{}", r.message);
        assert!(r.message.contains("entry limit"), "{}", r.message);
    }

    #[test]
    fn empty_urls_list_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].urls.clear();
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("at least one"), "{}", r.message);
    }

    #[test]
    fn empty_url_string_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].urls[0] = "".into();
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("urls[0]"), "{}", r.message);
        assert!(r.message.contains("empty"), "{}", r.message);
    }

    #[test]
    fn over_max_url_length_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].urls[0] = format!("stun:{}", "x".repeat(MAX_ICE_URL_LEN));
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("urls[0]"), "{}", r.message);
        assert!(r.message.contains("limit"), "{}", r.message);
    }

    #[test]
    fn non_allowlisted_scheme_is_refused() {
        for bad in [
            "http://stun.example.org",
            "https://stun.example.org",
            "file:///etc/passwd",
            "//stun.example.org",
            "stun.example.org:3478", // bare host, no scheme
            "STUN:stun.example.org", // case-sensitive: only lower-case
            "ws://signal.example.org",
            "javascript:alert(1)",
        ] {
            let mut cfg = ok_ice_config();
            cfg.ice_servers[0].urls[0] = bad.into();
            let r = check_ice_server_config(&cfg).unwrap_err();
            assert!(
                r.message.contains("allow-list"),
                "expected allow-list refusal for {bad}, got {}",
                r.message
            );
            // The hostile URL itself must NOT be echoed.
            assert!(
                !r.message.contains(bad),
                "rejection echoed url: {}",
                r.message
            );
        }
    }

    #[test]
    fn url_with_embedded_control_or_whitespace_is_refused() {
        for bad in [
            "stun:stun.example.org\n:3478",
            "stun:stun.example.org\t:3478",
            "stun:stun.example.org :3478",
            "stun:stun.example.org\u{0007}",
            "stun:stun.example.org\u{007F}",
        ] {
            let mut cfg = ok_ice_config();
            cfg.ice_servers[0].urls[0] = bad.into();
            let r = check_ice_server_config(&cfg).unwrap_err();
            assert!(
                r.message.contains("non-printable") || r.message.contains("whitespace"),
                "expected refusal for {bad:?}, got {}",
                r.message
            );
        }
    }

    #[test]
    fn url_with_bidi_override_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].urls[0] = "stun:stun.example.org\u{202E}".into();
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("bidi-override"), "{}", r.message);
    }

    #[test]
    fn turn_server_without_username_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[1].username = None;
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("username"), "{}", r.message);
        assert!(r.message.contains("required"), "{}", r.message);
    }

    #[test]
    fn turn_server_without_credential_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[1].credential = None;
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("credential"), "{}", r.message);
        assert!(r.message.contains("required"), "{}", r.message);
    }

    #[test]
    fn stun_server_with_credential_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].credential = Some("oops".into());
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("not permitted"), "{}", r.message);
        // And the credential's value MUST NOT leak into the
        // rejection message.
        assert!(!r.message.contains("oops"), "{}", r.message);
    }

    #[test]
    fn stun_server_with_username_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].username = Some("oops-user".into());
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("not permitted"), "{}", r.message);
    }

    #[test]
    fn over_max_credential_length_is_refused_without_echoing_value() {
        let mut cfg = ok_ice_config();
        let huge = "z".repeat(MAX_ICE_CREDENTIAL_LEN + 1);
        cfg.ice_servers[1].credential = Some(huge.clone());
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("credential"), "{}", r.message);
        assert!(r.message.contains("limit"), "{}", r.message);
        // The credential's value MUST NOT appear in the rejection.
        assert!(!r.message.contains(&huge), "rejection echoed credential");
        assert!(!r.message.contains("zzz"), "rejection echoed credential");
    }

    #[test]
    fn empty_credential_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[1].credential = Some(String::new());
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("credential"), "{}", r.message);
        assert!(r.message.contains("empty"), "{}", r.message);
    }

    #[test]
    fn credential_with_control_or_bidi_is_refused_without_echoing_value() {
        // Use a credential body whose substrings are unlikely to
        // appear in the rejection message itself ("ter" appears in
        // "character", "hun" in "thumb", etc).
        let secret_body = "Zk9-pAyL0aD";
        for (suffix, marker) in [
            ("\u{0000}", "non-printable"),
            ("\u{007F}", "non-printable"),
            ("\u{0007}", "non-printable"),
            ("\u{202E}", "bidi-override"),
            ("\u{2066}", "bidi-override"),
        ] {
            let mut cfg = ok_ice_config();
            let bad = format!("{secret_body}{suffix}");
            cfg.ice_servers[1].credential = Some(bad.clone());
            let r = check_ice_server_config(&cfg).unwrap_err();
            assert!(
                r.message.contains(marker),
                "expected {marker} refusal, got {}",
                r.message
            );
            // None of the credential's printable bytes may leak.
            assert!(
                !r.message.contains(secret_body),
                "rejection echoed credential: {}",
                r.message
            );
        }
    }

    #[test]
    fn username_with_bidi_override_is_refused() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[1].username = Some("agent\u{202E}bob".into());
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("username"), "{}", r.message);
        assert!(r.message.contains("bidi-override"), "{}", r.message);
    }

    #[test]
    fn oauth_credential_type_fails_closed() {
        let mut cfg = ok_ice_config();
        cfg.ice_servers[1].credential_type = IceCredentialType::Oauth;
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert!(r.message.contains("Oauth"), "{}", r.message);
        assert!(r.message.contains("not implemented"), "{}", r.message);
    }

    #[test]
    fn rejection_for_ice_config_carries_empty_session_id() {
        // The guard cannot independently validate the session id —
        // it is delivered out-of-band — so the rejection's
        // session_id field is left empty by contract. Pin that.
        let mut cfg = ok_ice_config();
        cfg.ice_servers[0].urls[0] = "javascript:alert(1)".into();
        let r = check_ice_server_config(&cfg).unwrap_err();
        assert_eq!(r.session_id, "");
    }

    // -----------------------------------------------------------------
    // Slice R7.j — `ProvideIceServers` request guard tests.
    // -----------------------------------------------------------------

    fn provide_ice_servers_req() -> ProvideIceServersRequest {
        ProvideIceServersRequest {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            access_key: "secret-access-key".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            ice_server_config: ok_ice_config(),
        }
    }

    #[test]
    fn happy_path_passes_for_provide_ice_servers() {
        check_provide_ice_servers(&provide_ice_servers_req(), Some(VALID_ORG_ID))
            .expect("happy path");
    }

    #[test]
    fn provide_ice_servers_refuses_cross_org_request() {
        let mut req = provide_ice_servers_req();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("organisation"), "{}", r.message);
        // session_id failed the cross-org check after passing the
        // UUID check, so it IS echoed back — the guard ran the
        // session-id validator first.
        assert_eq!(r.session_id, VALID_SESSION_ID);
    }

    #[test]
    fn provide_ice_servers_refuses_non_canonical_session_id() {
        let mut req = provide_ice_servers_req();
        req.session_id = "NOT-A-UUID".into();
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("session_id"), "{}", r.message);
        // Malformed session id MUST NOT be reflected back into the
        // failure result — same contract as every other guard here.
        assert_eq!(r.session_id, "");
    }

    #[test]
    fn provide_ice_servers_refuses_bidi_override_in_org_name() {
        let mut req = provide_ice_servers_req();
        req.org_name = "Acme\u{202E}".into();
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("org_name"), "{}", r.message);
        assert!(r.message.contains("bidi-override"), "{}", r.message);
    }

    #[test]
    fn provide_ice_servers_refuses_control_in_viewer_connection_id() {
        let mut req = provide_ice_servers_req();
        req.viewer_connection_id = "viewer\u{0007}".into();
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("viewer_connection_id"), "{}", r.message);
        assert!(r.message.contains("non-printable"), "{}", r.message);
    }

    #[test]
    fn provide_ice_servers_refuses_hostile_ice_url() {
        let mut req = provide_ice_servers_req();
        req.ice_server_config.ice_servers[0].urls[0] = "javascript:alert(1)".into();
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("ice_servers[0]"), "{}", r.message);
        assert!(r.message.contains("scheme"), "{}", r.message);
        // The URL contents MUST NOT appear in the rejection
        // message — only the field name and the policy that
        // refused it.
        assert!(!r.message.contains("javascript"), "{}", r.message);
        assert!(!r.message.contains("alert"), "{}", r.message);
    }

    #[test]
    fn provide_ice_servers_refuses_over_max_servers() {
        let mut req = provide_ice_servers_req();
        req.ice_server_config.ice_servers =
            (0..(MAX_ICE_SERVERS + 1)).map(|_| stun_server()).collect();
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("ice_servers"), "{}", r.message);
        assert!(r.message.contains("limit"), "{}", r.message);
    }

    #[test]
    fn provide_ice_servers_refuses_oauth_credential_type() {
        let mut req = provide_ice_servers_req();
        req.ice_server_config.ice_servers[1].credential_type = IceCredentialType::Oauth;
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("Oauth"), "{}", r.message);
        assert!(r.message.contains("not implemented"), "{}", r.message);
    }

    #[test]
    fn provide_ice_servers_does_not_echo_access_key_in_any_rejection() {
        // The guard MUST NOT read `access_key` under any branch, so
        // a leaked rejection message can never disclose it. Pin
        // that across every refusal path.
        fn cross_org(r: &mut ProvideIceServersRequest) {
            r.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        }
        fn bad_session(r: &mut ProvideIceServersRequest) {
            r.session_id = "not-a-uuid".into();
        }
        fn bidi_org(r: &mut ProvideIceServersRequest) {
            r.org_name = "Acme\u{202E}".into();
        }
        fn bad_url(r: &mut ProvideIceServersRequest) {
            r.ice_server_config.ice_servers[0].urls[0] = "javascript:alert(1)".into();
        }
        fn oauth(r: &mut ProvideIceServersRequest) {
            r.ice_server_config.ice_servers[1].credential_type = IceCredentialType::Oauth;
        }
        let cases: [fn(&mut ProvideIceServersRequest); 5] =
            [cross_org, bad_session, bidi_org, bad_url, oauth];
        for mutate in cases {
            let mut req = provide_ice_servers_req();
            mutate(&mut req);
            let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
            assert!(
                !r.message.contains("secret-access-key"),
                "access_key leaked in rejection: {}",
                r.message
            );
        }
    }

    #[test]
    fn provide_ice_servers_does_not_echo_turn_credential_in_any_rejection() {
        // The TURN shared secret is sensitive too — the rejection
        // for a hostile-bytes credential MUST cite only the field
        // name, never the bytes themselves.
        let mut req = provide_ice_servers_req();
        req.ice_server_config.ice_servers[1].credential = Some("hunter2\u{202E}".into());
        let r = check_provide_ice_servers(&req, Some(VALID_ORG_ID)).unwrap_err();
        assert!(r.message.contains("credential"), "{}", r.message);
        assert!(!r.message.contains("hunter2"), "{}", r.message);
    }
}
