// Source: CMRemote, clean-room implementation.

//! Hub-envelope dispatcher: peek the `"type"` discriminator and
//! fan the raw record out to the correct typed variant.
//!
//! Re-derived from `docs/wire-protocol.md` ➜ *Hub protocol* ➜
//! *Message-type discriminator*. The values mirror the SignalR hub
//! protocol discriminators used by the CMRemote server.

use serde::Deserialize;

use crate::{HubClose, HubCompletion, HubInvocation, HubPing, WireError};

/// A fully-decoded hub envelope, discriminated on the `"type"` field.
#[derive(Debug)]
pub enum HubEnvelope {
    /// Type 1 — server → agent method invocation.
    Invocation(HubInvocation),
    /// Type 3 — completion of a previously-issued invocation.
    Completion(HubCompletion),
    /// Type 6 — keep-alive ping.
    Ping(HubPing),
    /// Type 7 — server-initiated orderly shutdown.
    Close(HubClose),
    /// Any other type value. Carried so callers can log it; should be
    /// treated as a protocol violation per `docs/wire-protocol.md`.
    Unknown(u8),
}

/// Peek the `"type"` discriminator and deserialise the envelope.
///
/// Works for JSON-encoded records. MessagePack records must first be
/// decoded to a `serde_json::Value` or use the msgpack decode path
/// directly on the constituent types.
pub fn decode_envelope(bytes: &[u8]) -> Result<HubEnvelope, WireError> {
    #[derive(Deserialize)]
    struct TypePeeker {
        #[serde(rename = "type")]
        kind: u8,
    }

    let peeked: TypePeeker = serde_json::from_slice(bytes)?;
    match peeked.kind {
        1 => Ok(HubEnvelope::Invocation(serde_json::from_slice(bytes)?)),
        3 => Ok(HubEnvelope::Completion(serde_json::from_slice(bytes)?)),
        6 => Ok(HubEnvelope::Ping(serde_json::from_slice(bytes)?)),
        7 => Ok(HubEnvelope::Close(serde_json::from_slice(bytes)?)),
        n => Ok(HubEnvelope::Unknown(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_invocation() {
        let j = br#"{"type":1,"invocationId":"7","target":"TriggerHeartbeat","arguments":[]}"#;
        let env = decode_envelope(j).unwrap();
        assert!(matches!(env, HubEnvelope::Invocation(ref i) if i.target == "TriggerHeartbeat"));
    }

    #[test]
    fn decodes_completion_ok() {
        let j = br#"{"type":3,"invocationId":"1","result":null}"#;
        let env = decode_envelope(j).unwrap();
        assert!(matches!(env, HubEnvelope::Completion(_)));
    }

    #[test]
    fn decodes_ping() {
        let j = br#"{"type":6}"#;
        let env = decode_envelope(j).unwrap();
        assert!(matches!(env, HubEnvelope::Ping(_)));
    }

    #[test]
    fn decodes_close_with_quarantine() {
        let j = br#"{"type":7,"allowReconnect":false,"error":"quarantined"}"#;
        let env = decode_envelope(j).unwrap();
        match env {
            HubEnvelope::Close(c) => {
                assert!(!c.allow_reconnect);
                assert_eq!(c.error.as_deref(), Some("quarantined"));
            }
            _ => panic!("expected Close"),
        }
    }

    #[test]
    fn decodes_unknown_type() {
        let j = br#"{"type":42}"#;
        let env = decode_envelope(j).unwrap();
        assert!(matches!(env, HubEnvelope::Unknown(42)));
    }
}
