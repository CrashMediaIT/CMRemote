// Source: CMRemote, clean-room implementation.

//! Internal library surface of the CMRemote agent. Exposed as a `lib`
//! target so unit and integration tests can exercise the modules
//! without going through the binary entry point.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod cli;
pub mod config;
pub mod logging;
pub mod runtime;
pub mod transport;
