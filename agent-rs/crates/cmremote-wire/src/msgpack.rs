// Source: CMRemote, clean-room implementation.

//! MessagePack codec for the wire-protocol types.
//!
//! The SignalR hub protocol the CMRemote server speaks supports both a
//! JSON and a MessagePack transport. Slice R1a ships the JSON half; this
//! module is slice **R1b** — it adds the MessagePack half so that slice
//! R2 can negotiate either transport on the WebSocket.
//!
//! # Conventions
//!
//! - We use [`rmp_serde`] in its **named-field** mode
//!   ([`rmp_serde::Serializer::with_struct_map`]). This matches the
//!   SignalR MessagePack hub protocol and the corpus under
//!   `docs/wire-protocol-vectors/`.
//! - Encoded output is pure bytes and must not contain any
//!   implementation-dependent pointer / timestamp values — MessagePack
//!   is deterministic for our types, so byte-for-byte equality round
//!   trips are a guarantee we exploit in [`tests/vectors.rs`].
//! - Error handling funnels through [`WireError`] so callers do not
//!   need to depend on `rmp_serde` directly.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::WireError;

/// Encode `value` as MessagePack bytes using named struct fields.
///
/// See the module-level docs for why named fields (rather than the
/// tuple-packed default) are required.
pub fn to_msgpack<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    let mut buf = Vec::new();
    let mut ser = rmp_serde::Serializer::new(&mut buf).with_struct_map();
    value.serialize(&mut ser)?;
    Ok(buf)
}

/// Decode MessagePack bytes produced by [`to_msgpack`] (or a peer
/// speaking the same named-field convention) into `T`.
pub fn from_msgpack<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    rmp_serde::from_slice(bytes).map_err(WireError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionInfo, HubClose, HubCompletion, HubInvocation, HubPing};

    #[test]
    fn connection_info_round_trips_through_msgpack() {
        let info = ConnectionInfo {
            device_id: "d".into(),
            host: Some("https://example.com".into()),
            organization_id: Some("org".into()),
            server_verification_token: Some("tok".into()),
        };
        let bytes = to_msgpack(&info).expect("encode");
        let back: ConnectionInfo = from_msgpack(&bytes).expect("decode");
        assert_eq!(info, back);
        // Re-encoding the decoded value yields the same bytes (determinism).
        let bytes2 = to_msgpack(&back).expect("encode");
        assert_eq!(bytes, bytes2);
    }

    #[test]
    fn hub_invocation_round_trips_through_msgpack() {
        let inv = HubInvocation {
            kind: 1,
            invocation_id: Some("abc".into()),
            target: "InstallPackage".into(),
            arguments: vec![serde_json::json!("pkg-1"), serde_json::json!(42)],
        };
        let bytes = to_msgpack(&inv).expect("encode");
        let back: HubInvocation = from_msgpack(&bytes).expect("decode");
        assert_eq!(inv, back);
    }

    #[test]
    fn hub_completion_round_trips_through_msgpack() {
        let comp = HubCompletion {
            kind: 3,
            invocation_id: "abc".into(),
            result: Some(serde_json::json!({ "ok": true })),
            error: None,
        };
        let bytes = to_msgpack(&comp).expect("encode");
        let back: HubCompletion = from_msgpack(&bytes).expect("decode");
        assert_eq!(comp, back);
    }

    #[test]
    fn hub_ping_round_trips_through_msgpack() {
        let ping = HubPing::new();
        let bytes = to_msgpack(&ping).expect("encode");
        let back: HubPing = from_msgpack(&bytes).expect("decode");
        assert_eq!(ping, back);
    }

    #[test]
    fn hub_close_round_trips_through_msgpack() {
        let close = HubClose {
            kind: 7,
            error: Some("shutting down".into()),
            allow_reconnect: false,
        };
        let bytes = to_msgpack(&close).expect("encode");
        let back: HubClose = from_msgpack(&bytes).expect("decode");
        assert_eq!(close, back);
    }

    #[test]
    fn decode_rejects_garbage_bytes() {
        let err = from_msgpack::<HubPing>(b"\xff\xff\xff not valid msgpack").unwrap_err();
        assert!(matches!(err, WireError::MsgPackDecode(_)));
    }
}
