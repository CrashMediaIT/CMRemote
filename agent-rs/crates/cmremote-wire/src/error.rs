// Source: CMRemote, clean-room implementation.

//! Errors raised by the wire layer.

use thiserror::Error;

/// Errors produced when validating or (de)serializing wire types.
#[derive(Debug, Error)]
pub enum WireError {
    /// A required field was missing or otherwise invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(&'static str),

    /// Underlying JSON (de)serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// MessagePack encode failure.
    #[error("messagepack encode error: {0}")]
    MsgPackEncode(#[from] rmp_serde::encode::Error),

    /// MessagePack decode failure.
    #[error("messagepack decode error: {0}")]
    MsgPackDecode(#[from] rmp_serde::decode::Error),
}
