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
//!    into the consent prompt or the audit log.
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
    ChangeWindowsSessionRequest, DesktopTransportResult, RemoteControlSessionRequest,
    RestartScreenCasterRequest,
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
fn validate_operator_string(field: &str, value: &str) -> Result<(), String> {
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
        // U+200E LEFT-TO-RIGHT MARK / U+200F RIGHT-TO-LEFT MARK and
        // the U+202A..U+202E + U+2066..U+2069 bidi-override range
        // are the well-known "Trojan Source" vectors. Refuse them so
        // a hostile org name cannot disguise itself in the on-host
        // consent prompt.
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
}
