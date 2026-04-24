// Source: CMRemote, clean-room implementation.

//! Hub-envelope dispatcher: peek the `"type"` discriminator and
//! fan the raw record out to the correct typed variant.
//!
//! Re-derived from `docs/wire-protocol.md` ➜ *Hub protocol* ➜
//! *Message-type discriminator*. The values mirror the SignalR hub
//! protocol discriminators used by the CMRemote server.

use serde::Deserialize;

use crate::{
    from_msgpack, HubClose, HubCompletion, HubInvocation, HubPing, HubProtocol, WireError,
};

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

/// Peek the `"type"` discriminator and deserialise a JSON-encoded
/// hub record into a typed envelope.
///
/// Use [`decode_envelope_with`] when the encoding may vary at
/// runtime; this function exists for callers that always speak JSON
/// (for example test vectors) and want a slightly cleaner signature.
pub fn decode_envelope(bytes: &[u8]) -> Result<HubEnvelope, WireError> {
    decode_envelope_with(bytes, HubProtocol::Json)
}

/// Peek the `"type"` discriminator on a hub record encoded with the
/// negotiated [`HubProtocol`] and deserialise it into a typed envelope.
///
/// This is the function the agent's dispatch layer uses, since the
/// transport selects between JSON and MessagePack at handshake time
/// (see `transport::run_until_shutdown`).
pub fn decode_envelope_with(bytes: &[u8], encoding: HubProtocol) -> Result<HubEnvelope, WireError> {
    match encoding {
        HubProtocol::Json => decode_json(bytes),
        HubProtocol::Messagepack => decode_msgpack(bytes),
    }
}

fn decode_json(bytes: &[u8]) -> Result<HubEnvelope, WireError> {
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

fn decode_msgpack(bytes: &[u8]) -> Result<HubEnvelope, WireError> {
    // The MessagePack hub protocol uses named-field maps (see
    // `msgpack` module docs). Peek `"type"` by deserialising into a
    // permissive struct that ignores any other fields.
    #[derive(Deserialize)]
    struct TypePeeker {
        #[serde(rename = "type")]
        kind: u8,
    }

    let peeked: TypePeeker = from_msgpack(bytes)?;
    match peeked.kind {
        1 => Ok(HubEnvelope::Invocation(from_msgpack(bytes)?)),
        3 => Ok(HubEnvelope::Completion(from_msgpack(bytes)?)),
        6 => Ok(HubEnvelope::Ping(from_msgpack(bytes)?)),
        7 => Ok(HubEnvelope::Close(from_msgpack(bytes)?)),
        n => Ok(HubEnvelope::Unknown(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{to_msgpack, HubMessageKind};

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

    #[test]
    fn decodes_msgpack_invocation() {
        // Round-trip via msgpack to make sure the envelope decoder
        // matches what `to_msgpack` produces for our types.
        let inv = HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("9".into()),
            target: "TriggerHeartbeat".into(),
            arguments: vec![],
        };
        let bytes = to_msgpack(&inv).unwrap();
        let env = decode_envelope_with(&bytes, HubProtocol::Messagepack).unwrap();
        match env {
            HubEnvelope::Invocation(got) => assert_eq!(got, inv),
            other => panic!("expected Invocation, got {other:?}"),
        }
    }

    #[test]
    fn decodes_msgpack_close() {
        let c = HubClose {
            kind: HubMessageKind::Close as u8,
            error: Some("bye".into()),
            allow_reconnect: false,
        };
        let bytes = to_msgpack(&c).unwrap();
        let env = decode_envelope_with(&bytes, HubProtocol::Messagepack).unwrap();
        match env {
            HubEnvelope::Close(got) => {
                assert!(!got.allow_reconnect);
                assert_eq!(got.error.as_deref(), Some("bye"));
            }
            other => panic!("expected Close, got {other:?}"),
        }
    }
}
