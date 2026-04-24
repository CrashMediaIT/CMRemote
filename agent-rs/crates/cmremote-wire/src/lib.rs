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
pub mod desktop;
pub mod dispatch;
pub mod envelope;
pub mod error;
pub mod framing;
pub mod handshake;
pub mod msgpack;
pub mod package;
pub mod script;

pub use connection_info::ConnectionInfo;
pub use desktop::{
    ChangeWindowsSessionRequest, DesktopTransportResult, IceCandidate, InvokeCtrlAltDelRequest,
    RemoteControlSessionRequest, RestartScreenCasterRequest, SdpAnswer, SdpKind, SdpOffer,
    MAX_SDP_BYTES, MAX_SIGNALLING_STRING_LEN,
};
pub use dispatch::{decode_envelope, decode_envelope_with, HubEnvelope};
pub use envelope::{HubClose, HubCompletion, HubInvocation, HubMessageKind, HubPing};
pub use error::WireError;
pub use framing::{
    write_json_record, write_msgpack_record, FramingError, JsonFrameReader, MsgPackFrameReader,
    MAX_RECORD_BYTES, RECORD_SEPARATOR,
};
pub use handshake::{HandshakeRequest, HandshakeResponse, HubProtocol};
pub use msgpack::{from_msgpack, to_msgpack};
pub use package::{
    PackageInstallAction, PackageInstallRequest, PackageInstallResult, PackageProvider,
};
pub use script::{ExecuteCommandArgs, ScriptResult, ScriptingShell};
