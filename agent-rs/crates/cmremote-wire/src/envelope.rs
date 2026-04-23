// Source: CMRemote, clean-room implementation.

//! Hub-message envelope shared by the agent and server.
//!
//! This is a minimal R0 placeholder. The full SignalR-compatible
//! framing (handshake, ping, completion, stream items) is added in
//! slice R2 alongside the WebSocket transport.

use serde::{Deserialize, Serialize};

/// Discriminator for a hub envelope.
///
/// Numeric values are reserved to match the SignalR hub protocol the
/// CMRemote server speaks today; the exact mapping is documented in
/// `docs/wire-protocol.md` and pinned by test vectors in slice R1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum HubMessageKind {
    /// `Invocation` — caller asks the peer to run a method.
    Invocation = 1,
    /// `StreamItem` — a single item from a streamed response.
    StreamItem = 2,
    /// `Completion` — terminates an invocation or stream.
    Completion = 3,
    /// `Ping` — keep-alive.
    Ping = 6,
    /// `Close` — orderly shutdown of the hub connection.
    Close = 7,
}

/// Top-level invocation envelope.
///
/// Only the fields needed by the R0 scaffold are present; this type is
/// fleshed out in slice R1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubInvocation {
    /// SignalR message type discriminator.
    #[serde(rename = "type")]
    pub kind: u8,

    /// Optional invocation id (omitted for fire-and-forget).
    #[serde(
        rename = "invocationId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub invocation_id: Option<String>,

    /// Hub method name to invoke on the peer.
    pub target: String,

    /// Positional arguments encoded as JSON values.
    #[serde(default)]
    pub arguments: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_round_trips() {
        let inv = HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("1".into()),
            target: "DeviceHeartbeat".into(),
            arguments: vec![serde_json::json!("ok")],
        };
        let s = serde_json::to_string(&inv).unwrap();
        let back: HubInvocation = serde_json::from_str(&s).unwrap();
        assert_eq!(inv, back);
        assert!(s.contains("\"type\":1"));
        assert!(s.contains("\"target\":\"DeviceHeartbeat\""));
    }

    #[test]
    fn ping_kind_is_six() {
        assert_eq!(HubMessageKind::Ping as u8, 6);
    }
}
