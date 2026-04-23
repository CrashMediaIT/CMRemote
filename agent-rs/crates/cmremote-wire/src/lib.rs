// Source: CMRemote, clean-room implementation.
//
// `cmremote-wire` defines the data types exchanged between the CMRemote
// agent and server. The shapes here are re-derived independently from
// `docs/wire-protocol.md`; no source from the upstream project was used.
//
// This is the R0 scaffold: only the bootstrap `ConnectionInfo` and a
// minimal hub-message envelope are present. Slice R1 will add the full
// DTO surface and a MessagePack codec backed by a shared test-vector
// corpus.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

//! Wire-level data types for the CMRemote agent.

pub mod connection_info;
pub mod envelope;
pub mod error;

pub use connection_info::ConnectionInfo;
pub use envelope::{HubClose, HubCompletion, HubInvocation, HubMessageKind, HubPing};
pub use error::WireError;
