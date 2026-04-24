// Source: CMRemote, clean-room implementation.

//! Signalling DTOs for the desktop-transport WebRTC peer connection
//! (slice R7.g — *contract only*; the WebRTC backend is gated on the
//! [crypto-provider ADR](../../../../../docs/decisions/0001-webrtc-crypto-provider.md)
//! and intentionally not part of this slice).
//!
//! ## Why this exists now
//!
//! The .NET `IDesktopHubClient` carries every viewer-bound message
//! through a single opaque wrapper
//! (`SendDtoToClient(byte[] dtoWrapper, string viewerConnectionId)`).
//! When the agent grows a real WebRTC peer connection it needs three
//! concrete shapes inside that wrapper:
//!
//! 1. an SDP offer (viewer → agent, opening the negotiation),
//! 2. an SDP answer (viewer → agent, accepting an agent-side
//!    renegotiation),
//! 3. trickled ICE candidates (viewer → agent, late-arriving
//!    transport candidates).
//!
//! Slice R7.g freezes these three shapes — and matching test vectors
//! — so the .NET side can land its half against a stable contract
//! while the agent-side WebRTC backend is still gated on the
//! crypto-provider ADR. The shapes are PascalCase to match the rest
//! of the desktop-transport surface, so the same SignalR hub can
//! drive either agent fleet without a contract bump.
//!
//! ## Security contract
//!
//! Every signalling DTO carries the same operator-identity strings
//! ([`SessionId`](SdpOffer::session_id),
//! [`OrgId`](SdpOffer::org_id),
//! [`ViewerConnectionId`](SdpOffer::viewer_connection_id)) the four
//! existing desktop-transport methods carry. The agent-side guards
//! ([`cmremote_platform::desktop::guards`]) reuse the same checks
//! against these fields *before* parsing the SDP body, so:
//!
//! - a cross-org signalling message is refused at the same gate as a
//!   cross-org `RemoteControl` request,
//! - a non-canonical-UUID `session_id` cannot be reflected back into
//!   the audit log,
//! - a hostile `viewer_connection_id` (controls, bidi-overrides,
//!   over-length) is rejected before a downstream WebRTC parser ever
//!   sees it.
//!
//! In addition the SDP body itself is length-capped at
//! [`MAX_SDP_BYTES`] (16 KiB — well above the largest legitimate
//! offer / answer the .NET viewer ever produces, and far below the
//! point where a malformed body could be used as an amplification
//! vector). ICE candidate strings are length-capped at
//! [`MAX_SIGNALLING_STRING_LEN`] (1 KiB).
//!
//! The wire layer **only** caps string lengths and refuses obvious
//! shape violations; semantic validation of the SDP payload (offer /
//! answer / fingerprint mismatch, codec list, …) happens in the
//! WebRTC driver once it lands.

use serde::{Deserialize, Serialize};

/// Maximum byte length permitted for an inline SDP blob (offer or
/// answer). 16 KiB is comfortably above the largest legitimate SDP
/// the browser-side WebRTC stack produces (a maximally-decorated
/// offer with every codec / extension / FEC profile sits around
/// 4–6 KiB) and well below the point where a malformed body could
/// be used as a memory-exhaustion vector.
pub const MAX_SDP_BYTES: usize = 16 * 1024;

/// Maximum byte length permitted for any other signalling string —
/// candidate line, sdp-mid, viewer-connection-id, etc. 1 KiB is the
/// upper bound any RFC-compliant ICE candidate line ever needs.
pub const MAX_SIGNALLING_STRING_LEN: usize = 1024;

/// Discriminates an [`SdpOffer`] from an [`SdpAnswer`] when the
/// operator UI / audit log needs to render either. We carry the kind
/// explicitly on the wire (rather than relying on the DTO's Rust
/// type) so a future renegotiation message can reuse the same shape
/// with a different `Kind` field without a contract bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum SdpKind {
    /// `type: "offer"` per W3C `RTCSdpType`.
    #[default]
    Offer,
    /// `type: "answer"` per W3C `RTCSdpType`.
    Answer,
}

/// `SendSdpOffer(viewerConnectionId, sessionId, …, sdp)` — the viewer
/// is opening (or re-opening) the WebRTC negotiation.
///
/// The `sdp` field carries the raw SDP text. The agent's WebRTC
/// driver MUST treat it as untrusted UTF-8 — never as a shell
/// argument, file path, or HTML fragment — and MUST reject it if it
/// exceeds [`MAX_SDP_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SdpOffer {
    /// SignalR connection id of the viewer initiating the offer.
    pub viewer_connection_id: String,
    /// Server-issued session UUID — same identity as
    /// [`crate::RemoteControlSessionRequest::session_id`].
    pub session_id: String,
    /// Operator display name, surfaced in the consent prompt.
    pub requester_name: String,
    /// Operator organisation name.
    pub org_name: String,
    /// Operator organisation UUID — the agent's cross-org guard
    /// compares this against
    /// [`crate::ConnectionInfo::organization_id`].
    pub org_id: String,
    /// Discriminator — always [`SdpKind::Offer`] on this DTO; carried
    /// explicitly so the wire form is self-describing. Required on
    /// the wire — a missing `Kind` is a malformed payload.
    pub kind: SdpKind,
    /// Raw SDP blob produced by the viewer's `RTCPeerConnection`.
    pub sdp: String,
}

impl Default for SdpOffer {
    fn default() -> Self {
        Self {
            viewer_connection_id: String::new(),
            session_id: String::new(),
            requester_name: String::new(),
            org_name: String::new(),
            org_id: String::new(),
            kind: SdpKind::Offer,
            sdp: String::new(),
        }
    }
}

/// `SendSdpAnswer(viewerConnectionId, sessionId, …, sdp)` — the
/// viewer is accepting an agent-initiated renegotiation. Same shape
/// as [`SdpOffer`] with [`SdpKind::Answer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SdpAnswer {
    /// SignalR connection id of the viewer.
    pub viewer_connection_id: String,
    /// Session UUID.
    pub session_id: String,
    /// Operator display name.
    pub requester_name: String,
    /// Operator organisation name.
    pub org_name: String,
    /// Operator organisation UUID.
    pub org_id: String,
    /// Discriminator — always [`SdpKind::Answer`] on this DTO.
    /// Required on the wire — a missing `Kind` is a malformed
    /// payload.
    pub kind: SdpKind,
    /// Raw SDP blob.
    pub sdp: String,
}

impl Default for SdpAnswer {
    fn default() -> Self {
        Self {
            viewer_connection_id: String::new(),
            session_id: String::new(),
            requester_name: String::new(),
            org_name: String::new(),
            org_id: String::new(),
            kind: SdpKind::Answer,
            sdp: String::new(),
        }
    }
}

/// `SendIceCandidate(viewerConnectionId, sessionId, …, candidate,
/// sdpMid, sdpMLineIndex)` — a trickled ICE candidate from the
/// viewer.
///
/// The fields mirror W3C `RTCIceCandidateInit`. `sdp_mid` and
/// `sdp_mline_index` may be absent (or both `None`) when the viewer
/// signals an end-of-candidates marker; the wire form preserves
/// that by serialising them as JSON `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct IceCandidate {
    /// SignalR connection id of the viewer.
    pub viewer_connection_id: String,
    /// Session UUID.
    pub session_id: String,
    /// Operator display name.
    pub requester_name: String,
    /// Operator organisation name.
    pub org_name: String,
    /// Operator organisation UUID.
    pub org_id: String,
    /// `candidate:` line, RFC 5245 / 8445 grammar. Empty string is
    /// the legacy end-of-candidates signal.
    pub candidate: String,
    /// `a=mid` of the m-line this candidate belongs to. Absent for
    /// the end-of-candidates marker.
    #[serde(default)]
    pub sdp_mid: Option<String>,
    /// 0-based index of the m-line this candidate belongs to.
    /// Absent for the end-of-candidates marker.
    #[serde(default)]
    pub sdp_mline_index: Option<u16>,
}

// ---------------------------------------------------------------------------
// Slice R7.i — ICE / TURN server configuration.
//
// The future WebRTC peer connection needs to know which STUN / TURN
// servers to try before it can negotiate a path through restricted
// networks. Slice R7.i ships only the wire DTOs and the per-config
// length caps; the agent-side guard helpers live in
// `cmremote_platform::desktop::guards` and the (future) WebRTC driver
// consumes both. The .NET hub will eventually deliver an
// [`IceServerConfig`] either as a field on the existing
// `RemoteControlSessionRequest` (extension under the same wire
// protocol bump rules) or via a dedicated `ProvideIceServers` hub
// method; that integration step is deliberately deferred so the
// DTO contract can land first and the .NET side can build against
// frozen vectors.
// ---------------------------------------------------------------------------

/// Maximum number of [`IceServer`] entries permitted in a single
/// [`IceServerConfig`]. 8 is comfortably above the 2–4 entries a
/// typical deployment configures (one STUN, one TURN-UDP, one
/// TURN-TCP, one TURNS-TCP) and well below a number that would let a
/// hostile config exhaust the WebRTC stack's resolver budget.
pub const MAX_ICE_SERVERS: usize = 8;

/// Maximum number of `urls` entries on a single [`IceServer`]. The
/// W3C `RTCIceServer` shape lets one entry advertise multiple
/// transports for the same logical server; 4 covers the
/// TURN-UDP / TURN-TCP / TURNS-UDP / TURNS-TCP fan-out without
/// admitting an unbounded list.
pub const MAX_URLS_PER_ICE_SERVER: usize = 4;

/// Maximum byte length of any single ICE-server URL. 512 bytes is
/// far above any legitimate `turns:host:port?transport=tcp` form and
/// well below the point where the URL parser becomes a useful
/// memory-exhaustion vector.
pub const MAX_ICE_URL_LEN: usize = 512;

/// Maximum byte length of an [`IceServer::credential`] (the TURN
/// shared secret). 512 bytes is an order of magnitude above any
/// legitimate REST-API-shared credential while still bounding the
/// secret-handling code's memory footprint.
pub const MAX_ICE_CREDENTIAL_LEN: usize = 512;

/// Type discriminator for [`IceServer::credential`] — mirrors the
/// W3C `RTCIceCredentialType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum IceCredentialType {
    /// Plain shared secret. The default and the only variant the
    /// agent's WebRTC driver will initially honour.
    #[default]
    Password,
    /// OAuth-style credential (RFC 7635). Reserved on the wire so
    /// future deployments can opt in without a contract bump; the
    /// initial agent will surface
    /// [`crate::DesktopTransportResult::failed`] for any session
    /// that uses this credential type.
    Oauth,
}

/// ICE-transport policy hint — mirrors the W3C
/// `RTCIceTransportPolicy` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum IceTransportPolicy {
    /// Try every gathered candidate (host, server-reflexive, relay).
    /// The default — same shape browsers use.
    #[default]
    All,
    /// Only use relay (TURN) candidates. Useful for restricted
    /// network environments where direct paths are blocked or
    /// undesirable for compliance reasons.
    Relay,
}

/// A single ICE / STUN / TURN server entry. Mirrors the W3C
/// `RTCIceServer` shape; PascalCase wire field names match the
/// `.NET` hub.
///
/// `credential` is **sensitive**. The wire layer round-trips it as
/// part of the contract, but every downstream code path — logging,
/// `Debug`, error redactor, audit log — MUST refuse to render its
/// value. The agent-side guard helper
/// (`cmremote_platform::desktop::guards::check_ice_server_config`)
/// validates length and refuses hostile bytes without ever echoing
/// the credential into a rejection message.
///
/// `urls` MUST contain at least one entry; an empty list is a
/// malformed payload that the agent-side guard refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct IceServer {
    /// One or more `stun:` / `stuns:` / `turn:` / `turns:` URLs that
    /// describe the same logical server. Each URL is length-capped
    /// at [`MAX_ICE_URL_LEN`]; the list itself is bounded by
    /// [`MAX_URLS_PER_ICE_SERVER`].
    pub urls: Vec<String>,
    /// TURN username when [`Self::credential_type`] is
    /// [`IceCredentialType::Password`]. Absent for plain `stun:`.
    /// Sanitised by the same operator-string contract the desktop
    /// envelope guards apply.
    #[serde(default)]
    pub username: Option<String>,
    /// **Sensitive.** TURN credential (shared secret or REST-issued
    /// password). Length-capped at [`MAX_ICE_CREDENTIAL_LEN`].
    /// Implementations MUST NOT log, print, or echo this value.
    #[serde(default)]
    pub credential: Option<String>,
    /// How [`Self::credential`] should be interpreted. Defaults to
    /// [`IceCredentialType::Password`].
    #[serde(default)]
    pub credential_type: IceCredentialType,
}

/// Full ICE configuration delivered to the agent's WebRTC peer
/// connection — mirrors the subset of W3C `RTCConfiguration` the
/// agent honours. Bounded by [`MAX_ICE_SERVERS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct IceServerConfig {
    /// Servers tried in declaration order. May be empty — in which
    /// case the WebRTC driver only attempts host candidates and
    /// will work only on the same LAN as the viewer.
    pub ice_servers: Vec<IceServer>,
    /// Transport-policy hint. Defaults to [`IceTransportPolicy::All`].
    #[serde(default)]
    pub ice_transport_policy: IceTransportPolicy,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_msgpack, to_msgpack};

    fn offer() -> SdpOffer {
        SdpOffer {
            viewer_connection_id: "viewer-1".into(),
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            kind: SdpKind::Offer,
            sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n".into(),
        }
    }

    fn answer() -> SdpAnswer {
        SdpAnswer {
            viewer_connection_id: "viewer-1".into(),
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            kind: SdpKind::Answer,
            sdp: "v=0\r\n".into(),
        }
    }

    fn candidate() -> IceCandidate {
        IceCandidate {
            viewer_connection_id: "viewer-1".into(),
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        }
    }

    #[test]
    fn sdp_offer_round_trip_pascal_case() {
        let req = offer();
        let json = serde_json::to_string(&req).unwrap();
        // PascalCase field names must survive — this is the
        // contract with the .NET hub.
        assert!(
            json.contains("\"ViewerConnectionId\":\"viewer-1\""),
            "{json}"
        );
        assert!(json.contains("\"SessionId\":"), "{json}");
        assert!(json.contains("\"OrgId\":"), "{json}");
        assert!(json.contains("\"Kind\":\"Offer\""), "{json}");
        assert!(json.contains("\"Sdp\":"), "{json}");
        let back: SdpOffer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn sdp_offer_round_trip_through_msgpack() {
        let req = offer();
        let bytes = to_msgpack(&req).unwrap();
        let back: SdpOffer = from_msgpack(&bytes).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn sdp_answer_round_trip_pascal_case_and_msgpack() {
        let req = answer();
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"Kind\":\"Answer\""), "{json}");
        let back: SdpAnswer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
        let back2: SdpAnswer = from_msgpack(&to_msgpack(&req).unwrap()).unwrap();
        assert_eq!(back2, req);
    }

    #[test]
    fn ice_candidate_round_trip_includes_sdp_mid_and_mline_index() {
        let req = candidate();
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"Candidate\":"), "{json}");
        assert!(json.contains("\"SdpMid\":\"0\""), "{json}");
        assert!(json.contains("\"SdpMlineIndex\":0"), "{json}");
        let back: IceCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
        let back2: IceCandidate = from_msgpack(&to_msgpack(&req).unwrap()).unwrap();
        assert_eq!(back2, req);
    }

    #[test]
    fn ice_candidate_end_of_candidates_marker_serialises_with_null_mid_and_index() {
        let mut req = candidate();
        req.candidate = String::new();
        req.sdp_mid = None;
        req.sdp_mline_index = None;
        let json = serde_json::to_string(&req).unwrap();
        // `Option::None` round-trips as JSON `null` so the .NET side
        // can detect the end-of-candidates marker by absence of a
        // value rather than by comparing against an empty string.
        assert!(json.contains("\"SdpMid\":null"), "{json}");
        assert!(json.contains("\"SdpMlineIndex\":null"), "{json}");
        let back: IceCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn defaults_fail_closed_with_empty_strings() {
        // Default values produce a struct the agent's guards refuse
        // (empty `session_id` is not a canonical UUID), so a malformed
        // payload that omits required fields surfaces as a structured
        // failure rather than panicking inside the dispatcher.
        let r: SdpOffer = Default::default();
        assert!(r.session_id.is_empty());
        assert!(r.sdp.is_empty());
        assert_eq!(r.kind, SdpKind::Offer);

        let r: SdpAnswer = Default::default();
        assert_eq!(r.kind, SdpKind::Answer);

        let r: IceCandidate = Default::default();
        assert!(r.candidate.is_empty());
        assert!(r.sdp_mid.is_none());
        assert!(r.sdp_mline_index.is_none());
    }

    #[test]
    fn sdp_kind_serialises_as_pascal_case_string() {
        // Pin the wire encoding of `SdpKind` so the .NET side can
        // round-trip it as a plain `string`.
        assert_eq!(serde_json::to_string(&SdpKind::Offer).unwrap(), "\"Offer\"");
        assert_eq!(
            serde_json::to_string(&SdpKind::Answer).unwrap(),
            "\"Answer\""
        );
    }

    #[test]
    fn missing_kind_field_is_a_decode_error() {
        // The discriminator is required on the wire — there is no
        // sensible default for a peer that omits it (an `SdpAnswer`
        // payload that silently became an `SdpOffer` would be a
        // genuine bug). Pin the fail-closed behaviour so the .NET
        // side can rely on it.
        let json = r#"{
            "ViewerConnectionId": "v",
            "SessionId": "s",
            "RequesterName": "r",
            "OrgName": "o",
            "OrgId": "g",
            "Sdp": "v=0\r\n"
        }"#;
        assert!(serde_json::from_str::<SdpOffer>(json).is_err());
        assert!(serde_json::from_str::<SdpAnswer>(json).is_err());
    }

    #[test]
    fn cap_constants_have_expected_values() {
        // Pin the constants — slice R7.g freezes these so the
        // agent-side guards can rely on them without re-deriving.
        assert_eq!(MAX_SDP_BYTES, 16 * 1024);
        assert_eq!(MAX_SIGNALLING_STRING_LEN, 1024);
    }

    // -----------------------------------------------------------------
    // Slice R7.i — ICE / TURN server configuration tests.
    // -----------------------------------------------------------------

    fn ice_config() -> IceServerConfig {
        IceServerConfig {
            ice_servers: vec![
                IceServer {
                    urls: vec!["stun:stun.example.org:3478".into()],
                    username: None,
                    credential: None,
                    credential_type: IceCredentialType::Password,
                },
                IceServer {
                    urls: vec![
                        "turn:turn.example.org:3478?transport=udp".into(),
                        "turns:turn.example.org:5349?transport=tcp".into(),
                    ],
                    username: Some("agent-bob".into()),
                    credential: Some("hunter2".into()),
                    credential_type: IceCredentialType::Password,
                },
            ],
            ice_transport_policy: IceTransportPolicy::All,
        }
    }

    #[test]
    fn ice_server_config_round_trip_pascal_case() {
        let cfg = ice_config();
        let json = serde_json::to_string(&cfg).unwrap();
        // PascalCase wire field names must survive — the .NET hub
        // is the consumer and matches on these exact keys.
        assert!(json.contains("\"IceServers\":"), "{json}");
        assert!(json.contains("\"IceTransportPolicy\":\"All\""), "{json}");
        assert!(json.contains("\"Urls\":"), "{json}");
        assert!(json.contains("\"Username\":\"agent-bob\""), "{json}");
        // The credential is on the wire — the wire layer's job is
        // only to round-trip it; the *agent* logging layer is what
        // refuses to render it. We pin its presence here so the
        // contract with the .NET hub is explicit.
        assert!(json.contains("\"Credential\":\"hunter2\""), "{json}");
        assert!(json.contains("\"CredentialType\":\"Password\""), "{json}");
        let back: IceServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn ice_server_config_round_trip_through_msgpack() {
        let cfg = ice_config();
        let bytes = to_msgpack(&cfg).unwrap();
        let back: IceServerConfig = from_msgpack(&bytes).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn ice_server_config_relay_only_round_trips() {
        let cfg = IceServerConfig {
            ice_servers: vec![IceServer {
                urls: vec!["turns:relay.example.org:5349?transport=tcp".into()],
                username: Some("agent-bob".into()),
                credential: Some("hunter2".into()),
                credential_type: IceCredentialType::Password,
            }],
            ice_transport_policy: IceTransportPolicy::Relay,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"IceTransportPolicy\":\"Relay\""), "{json}");
        let back: IceServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn ice_server_omits_optional_fields_when_absent() {
        let s = IceServer {
            urls: vec!["stun:stun.example.org".into()],
            username: None,
            credential: None,
            credential_type: IceCredentialType::Password,
        };
        let json = serde_json::to_string(&s).unwrap();
        // Optional credentials are emitted as JSON `null` (matching
        // the existing `IceCandidate` SdpMid / SdpMlineIndex
        // convention) so the .NET side detects absence by the field
        // being null rather than missing — pin both shapes.
        assert!(json.contains("\"Username\":null"), "{json}");
        assert!(json.contains("\"Credential\":null"), "{json}");
        let back: IceServer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn defaults_fail_closed_with_empty_collections() {
        let cfg: IceServerConfig = Default::default();
        assert!(cfg.ice_servers.is_empty());
        assert_eq!(cfg.ice_transport_policy, IceTransportPolicy::All);

        let s: IceServer = Default::default();
        assert!(s.urls.is_empty());
        assert!(s.username.is_none());
        assert!(s.credential.is_none());
        assert_eq!(s.credential_type, IceCredentialType::Password);
    }

    #[test]
    fn ice_credential_type_pascal_case() {
        assert_eq!(
            serde_json::to_string(&IceCredentialType::Password).unwrap(),
            "\"Password\""
        );
        assert_eq!(
            serde_json::to_string(&IceCredentialType::Oauth).unwrap(),
            "\"Oauth\""
        );
    }

    #[test]
    fn ice_transport_policy_pascal_case() {
        assert_eq!(
            serde_json::to_string(&IceTransportPolicy::All).unwrap(),
            "\"All\""
        );
        assert_eq!(
            serde_json::to_string(&IceTransportPolicy::Relay).unwrap(),
            "\"Relay\""
        );
    }

    #[test]
    fn ice_cap_constants_have_expected_values() {
        // Pin the per-config caps — slice R7.i freezes these so the
        // agent-side guards can rely on them without re-deriving.
        assert_eq!(MAX_ICE_SERVERS, 8);
        assert_eq!(MAX_URLS_PER_ICE_SERVER, 4);
        assert_eq!(MAX_ICE_URL_LEN, 512);
        assert_eq!(MAX_ICE_CREDENTIAL_LEN, 512);
    }
}
