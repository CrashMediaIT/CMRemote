// Source: CMRemote, clean-room implementation.

//! Hub-message envelope shared by the agent and server.
//!
//! Re-derived independently from [`docs/wire-protocol.md`] ➜
//! *Hub protocol* ➜ *Envelope shapes (JSON)*. The shapes here are
//! pinned by the test-vector corpus under
//! `docs/wire-protocol-vectors/envelope/`.
//!
//! Slice R2 will add the WebSocket transport that frames these
//! envelopes; the types here remain transport-agnostic.

use serde::{Deserialize, Serialize};

/// Discriminator for a hub envelope.
///
/// Numeric values match the SignalR hub protocol the CMRemote
/// server speaks today; the exact mapping is documented in
/// `docs/wire-protocol.md` and pinned by the corpus.
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
/// `invocationId` is omitted on the wire for fire-and-forget
/// invocations (e.g. `Log`). The agent must treat an unknown
/// `target` as a protocol violation per the spec's *Method
/// surface* section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubInvocation {
    /// SignalR message type discriminator. Always `1` for an
    /// invocation; kept as `u8` so a malformed inbound frame can
    /// be inspected before being rejected.
    #[serde(rename = "type")]
    pub kind: u8,

    /// Optional invocation id (omitted for fire-and-forget).
    #[serde(
        rename = "invocationId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub invocation_id: Option<String>,

    /// Hub method name to invoke on the peer. Compared
    /// case-sensitively against an allow-list before any argument
    /// parsing.
    pub target: String,

    /// Positional arguments encoded as JSON values. Empty arrays
    /// are serialised as `[]`, never omitted, so an additive
    /// argument can be distinguished from a missing one.
    #[serde(default)]
    pub arguments: Vec<serde_json::Value>,
}

/// Completion of a previously-issued invocation.
///
/// Exactly one of `result` and `error` is meaningful per the spec.
/// The implementations enforce the mutual-exclusion rule via
/// [`HubCompletion::validate`]; deserializers are intentionally
/// permissive so that violations can be observed and rejected
/// rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubCompletion {
    /// SignalR message type discriminator. Always `3`.
    #[serde(rename = "type")]
    pub kind: u8,

    /// Invocation this completion refers to. Required.
    #[serde(rename = "invocationId")]
    pub invocation_id: String,

    /// Successful result payload. Mutually exclusive with `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Failure reason. Mutually exclusive with `result`. The
    /// agent maps a small allow-list of reason codes
    /// (`invalid_arguments`, `duplicate_invocation`,
    /// `not_implemented`, …) and treats any other value as
    /// opaque text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl HubCompletion {
    /// Returns `Err` if both `result` and `error` are populated,
    /// which is a protocol violation per the spec's *Completion*
    /// section.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.result.is_some() && self.error.is_some() {
            return Err("completion has both result and error");
        }
        Ok(())
    }

    /// Construct a successful completion.
    pub fn ok(invocation_id: impl Into<String>, result: serde_json::Value) -> Self {
        Self {
            kind: HubMessageKind::Completion as u8,
            invocation_id: invocation_id.into(),
            result: Some(result),
            error: None,
        }
    }

    /// Construct an error completion.
    pub fn err(invocation_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            kind: HubMessageKind::Completion as u8,
            invocation_id: invocation_id.into(),
            result: None,
            error: Some(error.into()),
        }
    }
}

/// Server → client orderly shutdown.
///
/// `allowReconnect` defaults to `true` if absent on the wire so
/// that a server can issue a minimal `{"type":7}` and still get
/// the standard reconnect behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubClose {
    /// SignalR message type discriminator. Always `7`.
    #[serde(rename = "type")]
    pub kind: u8,

    /// Optional human-readable reason for the close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Whether the agent is permitted to reconnect after this
    /// close. Defaults to `true` when omitted.
    #[serde(rename = "allowReconnect", default = "default_true")]
    pub allow_reconnect: bool,
}

fn default_true() -> bool {
    true
}

/// Bidirectional keep-alive envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubPing {
    /// SignalR message type discriminator. Always `6`.
    #[serde(rename = "type")]
    pub kind: u8,
}

impl HubPing {
    /// Construct a well-formed ping.
    pub const fn new() -> Self {
        Self {
            kind: HubMessageKind::Ping as u8,
        }
    }
}

impl Default for HubPing {
    fn default() -> Self {
        Self::new()
    }
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
        assert_eq!(HubPing::new().kind, 6);
    }

    #[test]
    fn completion_rejects_result_and_error_together() {
        let bad = HubCompletion {
            kind: HubMessageKind::Completion as u8,
            invocation_id: "9".into(),
            result: Some(serde_json::json!(null)),
            error: Some("oops".into()),
        };
        assert!(bad.validate().is_err());

        let ok = HubCompletion {
            kind: HubMessageKind::Completion as u8,
            invocation_id: "9".into(),
            result: Some(serde_json::json!(null)),
            error: None,
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn close_default_allow_reconnect_is_true() {
        let c: HubClose = serde_json::from_str(r#"{"type":7}"#).unwrap();
        assert!(c.allow_reconnect);
        assert!(c.error.is_none());
    }
}
