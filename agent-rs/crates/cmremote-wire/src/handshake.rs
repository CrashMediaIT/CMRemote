// Source: CMRemote, clean-room implementation.

//! SignalR handshake records.
//!
//! Re-derived independently from `docs/wire-protocol.md` ➜ *Hub
//! protocol* ➜ *Handshake*. The shapes here are pinned by the
//! test-vector corpus under `docs/wire-protocol-vectors/handshake/`.
//!
//! The handshake is exchanged once per WebSocket connection,
//! immediately after the WebSocket upgrade completes. It is **not**
//! framed as a SignalR hub envelope — it is its own JSON record
//! terminated by the `0x1E` record-separator byte (see
//! [`crate::framing`]).

use serde::{Deserialize, Serialize};

/// Identifier of the hub-protocol encoding the agent intends to use
/// for the lifetime of the connection.
///
/// CMRemote supports exactly two encodings, mirroring the SignalR
/// hub protocol's `json` and `messagepack`. Mixing them on one
/// connection is a protocol violation per the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HubProtocol {
    /// Newline-record-separator-framed JSON. Preferred for
    /// development and for humans reading captures.
    Json,
    /// Length-prefixed MessagePack. Preferred in production.
    Messagepack,
}

impl HubProtocol {
    /// Lower-case wire identifier, exactly as it appears on the
    /// `protocol` field of the handshake request.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Messagepack => "messagepack",
        }
    }
}

/// Handshake request sent by the agent immediately after the
/// WebSocket upgrade.
///
/// Always followed by the record-separator byte `0x1E`; the framer
/// is the single owner of that byte so this struct stays a pure
/// JSON shape that can be unit-tested in isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// Hub-protocol encoding the agent wants to use. The server
    /// either accepts (empty response) or rejects with an error
    /// describing the expected encoding.
    pub protocol: HubProtocol,

    /// Hub-protocol version. Always `1` for this `protocolVersion`.
    pub version: u8,
}

impl HandshakeRequest {
    /// Build a handshake request for the requested protocol at the
    /// pinned `version: 1`.
    pub const fn new(protocol: HubProtocol) -> Self {
        Self {
            protocol,
            version: 1,
        }
    }
}

/// Handshake response sent by the server.
///
/// Per the SignalR hub protocol, success is `{}` (no fields) and
/// failure is `{"error": "<reason>"}`. We model success as
/// `error == None` rather than a separate variant so the on-wire
/// shape stays serde-friendly without a custom `Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HandshakeResponse {
    /// Failure reason. `None` means the handshake succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HandshakeResponse {
    /// Convenience constructor for the success case.
    pub const fn ok() -> Self {
        Self { error: None }
    }

    /// Convenience constructor for the failure case.
    pub fn rejected<S: Into<String>>(reason: S) -> Self {
        Self {
            error: Some(reason.into()),
        }
    }

    /// Returns `true` iff this response indicates a successful
    /// handshake.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialises_protocol_in_lowercase() {
        let req = HandshakeRequest::new(HubProtocol::Json);
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"protocol\":\"json\""), "got {s}");
        assert!(s.contains("\"version\":1"), "got {s}");
    }

    #[test]
    fn request_round_trips_messagepack_variant() {
        let req = HandshakeRequest::new(HubProtocol::Messagepack);
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"protocol\":\"messagepack\""), "got {s}");
        let back: HandshakeRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn response_ok_has_no_error_field_on_the_wire() {
        let r = HandshakeResponse::ok();
        let s = serde_json::to_string(&r).unwrap();
        // The empty-object response is what the SignalR spec mandates;
        // emitting `{"error": null}` would break older servers that
        // treat any presence of `error` as failure.
        assert_eq!(s, "{}");
    }

    #[test]
    fn response_rejected_round_trips() {
        let r = HandshakeResponse::rejected("protocol_version_unsupported");
        let s = serde_json::to_string(&r).unwrap();
        let back: HandshakeResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
        assert!(!back.is_ok());
        assert_eq!(back.error.as_deref(), Some("protocol_version_unsupported"));
    }

    #[test]
    fn response_empty_object_parses_as_ok() {
        let r: HandshakeResponse = serde_json::from_str("{}").unwrap();
        assert!(r.is_ok());
    }

    #[test]
    fn unknown_protocol_string_is_rejected() {
        let err = serde_json::from_str::<HandshakeRequest>(r#"{"protocol":"junk","version":1}"#)
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("variant"));
    }

    #[test]
    fn protocol_wire_identifiers_match_spec() {
        assert_eq!(HubProtocol::Json.as_wire(), "json");
        assert_eq!(HubProtocol::Messagepack.as_wire(), "messagepack");
    }
}
