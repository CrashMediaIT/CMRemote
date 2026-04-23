// Source: CMRemote, clean-room implementation.
//
// `cmremote-wire` defines the data types exchanged between the CMRemote
// agent and server. The shapes here are re-derived independently from
// `docs/wire-protocol.md`; no source from the upstream project was used.
//
// Slice R1a shipped the JSON half of the codec, the full test-vector
// corpus, and the redacting `Debug` for `ConnectionInfo`. Slice R1b
// (this module set) adds the MessagePack half so slice R2 can negotiate
// either transport on the WebSocket.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

//! Wire-level data types for the CMRemote agent.

pub mod connection_info;
pub mod envelope;
pub mod error;
pub mod msgpack;

pub use connection_info::ConnectionInfo;
pub use envelope::{HubClose, HubCompletion, HubInvocation, HubMessageKind, HubPing};
pub use error::WireError;
pub use msgpack::{from_msgpack, to_msgpack};
