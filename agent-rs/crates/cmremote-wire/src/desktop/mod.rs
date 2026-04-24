// Source: CMRemote, clean-room implementation.

//! DTOs for the desktop-transport hub methods (slice R7 — *initial*
//! wire surface).
//!
//! Re-derived from `Shared/Interfaces/IAgentHubClient.cs` ➜ the four
//! desktop-transport methods (`RemoteControl`, `RestartScreenCaster`,
//! `ChangeWindowsSession`, `InvokeCtrlAltDel`). Shapes are re-authored
//! independently from the spec; nothing is copied verbatim. The .NET
//! signatures take a flat positional argument list — we mirror that
//! by deserialising the SignalR `arguments` array directly into the
//! struct fields, with **PascalCase** wire names matching the names
//! the .NET hub uses on the wire so the same server can drive either
//! agent fleet without a contract bump.
//!
//! Slice R7 ships **only** the wire surface, the
//! [`crate::package::PackageProvider`]-style "fail closed" defaults, and
//! the JSON + MessagePack round-trip tests. The
//! [`cmremote_platform`]'s `DesktopTransportProvider` trait, the
//! `NotSupportedDesktopTransport` stub, and the agent-side dispatch
//! routing land in the same PR; the WebRTC / capture / encode driver
//! follows in a later slice as outlined in the roadmap.
//!
//! ## Security contract
//!
//! All four request types carry **operator identity strings** (display
//! name, organisation) that the .NET viewer surfaces verbatim. The
//! agent must treat every string field as untrusted UTF-8 — never as
//! a shell argument, file path, or HTML fragment. The desktop driver
//! is responsible for length-capping and printable-only validation
//! before any value reaches a child process or a UI rendering layer.

use serde::{Deserialize, Serialize};

pub mod signalling;

pub use signalling::{
    IceCandidate, IceCredentialType, IceServer, IceServerConfig, IceTransportPolicy,
    ProvideIceServersRequest, SdpAnswer, SdpKind, SdpOffer, MAX_ICE_CREDENTIAL_LEN,
    MAX_ICE_SERVERS, MAX_ICE_URL_LEN, MAX_SDP_BYTES, MAX_SIGNALLING_STRING_LEN,
    MAX_URLS_PER_ICE_SERVER,
};

/// Request payload for the `RemoteControl(sessionId, accessKey, …)`
/// hub method. Fields mirror the .NET signature in
/// `IAgentHubClient.RemoteControl` exactly.
///
/// The agent uses `session_id` + `access_key` to authenticate against
/// the desktop hub, then opens a viewer-bound peer connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteControlSessionRequest {
    /// Server-issued session UUID. Mirrors the .NET `Guid sessionId`
    /// parameter; carried as a string so deserialisation does not
    /// silently reject a non-canonical UUID rendering.
    pub session_id: String,
    /// One-shot access key the agent presents on the desktop hub.
    /// Sensitive — the agent MUST NOT log this value.
    pub access_key: String,
    /// SignalR connection id of the viewer that initiated the session.
    pub user_connection_id: String,
    /// Display name of the operator initiating the session, surfaced
    /// in the on-host consent prompt.
    pub requester_name: String,
    /// Operator organisation name, surfaced in the consent prompt.
    pub org_name: String,
    /// Operator organisation UUID — the agent's local consent policy
    /// can compare this against `ConnectionInfo.organization_id` to
    /// refuse cross-org sessions.
    pub org_id: String,
}

/// Request payload for the `RestartScreenCaster(viewerIds, sessionId, …)`
/// hub method. Used to re-spawn the screencaster process for a list
/// of attached viewers (e.g. after a Windows session switch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RestartScreenCasterRequest {
    /// Viewer SignalR connection IDs that should be re-attached after
    /// the screencaster comes back up.
    pub viewer_ids: Vec<String>,
    /// Session UUID; same identity as
    /// [`RemoteControlSessionRequest::session_id`].
    pub session_id: String,
    /// Access key — sensitive, MUST NOT be logged.
    pub access_key: String,
    /// SignalR connection id of the viewer initiating the restart.
    pub user_connection_id: String,
    /// Operator display name.
    pub requester_name: String,
    /// Operator organisation name.
    pub org_name: String,
    /// Operator organisation UUID.
    pub org_id: String,
}

/// Request payload for the
/// `ChangeWindowsSession(viewerConnectionId, sessionId, accessKey, …)`
/// hub method. Asks the agent to relaunch the screencaster against a
/// specific Windows session id (winlogon, console, RDP, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ChangeWindowsSessionRequest {
    /// SignalR connection id of the viewer that asked for the switch.
    pub viewer_connection_id: String,
    /// Existing remote-control session UUID.
    pub session_id: String,
    /// Sensitive access key for the existing session.
    pub access_key: String,
    /// SignalR connection id of the user owning the session.
    pub user_connection_id: String,
    /// Operator display name.
    pub requester_name: String,
    /// Operator organisation name.
    pub org_name: String,
    /// Operator organisation UUID.
    pub org_id: String,
    /// Target Windows session id (`> 0` for an interactive session,
    /// `0` for the services session, `-1` for "agent picks one").
    pub target_session_id: i32,
}

/// Request payload for `InvokeCtrlAltDel()` — no fields. Wrapped in a
/// struct so the dispatch layer can decode-then-route by argument
/// type uniformly with the other desktop methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InvokeCtrlAltDelRequest;

/// Outcome of a desktop-transport request reported back to the server.
///
/// Mirrors the structure of [`crate::PackageInstallResult`] so an
/// operator UI can reuse the same "result + error_message" rendering
/// without conditional logic per method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct DesktopTransportResult {
    /// Echoes the request's `session_id` (or `""` for `InvokeCtrlAltDel`,
    /// which has no session id of its own).
    pub session_id: String,
    /// `true` when the desktop driver acknowledged the request and
    /// the underlying capture / WebRTC pipeline started without error.
    pub success: bool,
    /// Operator-facing failure message — provider-not-supported,
    /// session-already-active, OS error code, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl DesktopTransportResult {
    /// Build a `success = true` result for the supplied session.
    pub fn ok(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            success: true,
            error_message: None,
        }
    }

    /// Build a structured failure result. Always sets `success = false`
    /// and copies `error_message` verbatim — the caller is responsible
    /// for redacting any secret material before invoking this helper.
    pub fn failed(session_id: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            success: false,
            error_message: Some(error_message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_msgpack, to_msgpack};

    #[test]
    fn remote_control_request_round_trip_pascal_case() {
        let req = RemoteControlSessionRequest {
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            access_key: "ak".into(),
            user_connection_id: "viewer-1".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: "org-uuid".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        // Critical: PascalCase field names so the wire stays
        // byte-stable with the .NET hub.
        assert!(json.contains("\"SessionId\":\"11111111-2222-3333-4444-555555555555\""));
        assert!(json.contains("\"AccessKey\":\"ak\""));
        assert!(json.contains("\"UserConnectionId\":\"viewer-1\""));
        assert!(json.contains("\"RequesterName\":\"Alice\""));
        assert!(json.contains("\"OrgName\":\"Acme\""));
        assert!(json.contains("\"OrgId\":\"org-uuid\""));
        let back: RemoteControlSessionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn remote_control_request_round_trips_through_msgpack() {
        let req = RemoteControlSessionRequest {
            session_id: "s".into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: "i".into(),
        };
        let bytes = to_msgpack(&req).unwrap();
        let back: RemoteControlSessionRequest = from_msgpack(&bytes).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn restart_screencaster_request_round_trip() {
        let req = RestartScreenCasterRequest {
            viewer_ids: vec!["v1".into(), "v2".into()],
            session_id: "s".into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: "i".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"ViewerIds\":[\"v1\",\"v2\"]"));
        let back: RestartScreenCasterRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);

        let bytes = to_msgpack(&req).unwrap();
        let back2: RestartScreenCasterRequest = from_msgpack(&bytes).unwrap();
        assert_eq!(back2, req);
    }

    #[test]
    fn change_windows_session_request_round_trip() {
        let req = ChangeWindowsSessionRequest {
            viewer_connection_id: "v".into(),
            session_id: "s".into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: "i".into(),
            target_session_id: 3,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"TargetSessionId\":3"));
        let back: ChangeWindowsSessionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);

        let bytes = to_msgpack(&req).unwrap();
        let back2: ChangeWindowsSessionRequest = from_msgpack(&bytes).unwrap();
        assert_eq!(back2, req);
    }

    #[test]
    fn invoke_ctrl_alt_del_request_is_a_unit_struct() {
        // Serialisation of a unit struct emits `null` in JSON.
        let json = serde_json::to_string(&InvokeCtrlAltDelRequest).unwrap();
        assert_eq!(json, "null");
        let back: InvokeCtrlAltDelRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, InvokeCtrlAltDelRequest);
    }

    #[test]
    fn desktop_result_helpers_invariants() {
        let ok = DesktopTransportResult::ok("session-1");
        assert!(ok.success);
        assert!(ok.error_message.is_none());
        assert_eq!(ok.session_id, "session-1");

        let bad = DesktopTransportResult::failed("session-2", "Provider not supported.");
        assert!(!bad.success);
        assert_eq!(
            bad.error_message.as_deref(),
            Some("Provider not supported.")
        );
        assert_eq!(bad.session_id, "session-2");
    }

    #[test]
    fn desktop_result_round_trip_pascal_case_and_omits_none() {
        let r = DesktopTransportResult {
            session_id: "s".into(),
            success: true,
            error_message: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"SessionId\":\"s\""));
        assert!(json.contains("\"Success\":true"));
        // `error_message: None` must be omitted so the wire stays narrow.
        assert!(!json.contains("ErrorMessage"));
        let back: DesktopTransportResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn desktop_result_failure_round_trip_includes_error_message() {
        let r = DesktopTransportResult::failed("s", "boom");
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"Success\":false"));
        assert!(json.contains("\"ErrorMessage\":\"boom\""));
        let back: DesktopTransportResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn defaults_fail_closed_with_empty_strings() {
        // The .NET hub will never legitimately send a request with no
        // session id; the agent must reject such a request loudly.
        // Defaulting to empty strings means a malformed payload that
        // omits required fields deserialises into a struct the
        // dispatcher can identify as "not actionable" without panicking.
        let r: RemoteControlSessionRequest = Default::default();
        assert!(r.session_id.is_empty());
        assert!(r.access_key.is_empty());
        let r: ChangeWindowsSessionRequest = Default::default();
        assert!(r.session_id.is_empty());
        assert_eq!(r.target_session_id, 0);
    }
}
