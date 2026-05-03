// Source: CMRemote, clean-room implementation.

//! macOS-specific desktop capture / encode / input providers for the
//! CMRemote agent.
//!
//! The implementation uses macOS' built-in command surfaces
//! (`screencapture`, `osascript`, `pbcopy`, `pbpaste`) plus `ffmpeg`
//! for H.264 encoding. Construction is capability-gated so the agent
//! falls back to the structured `NotSupported` bundle if a required
//! executable is missing or the host is not macOS. No private APIs or
//! unsafe FFI are used in this crate.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod capture;
pub mod encoder;
pub mod input;
pub mod notification;
pub mod providers;

pub use capture::{BmpDesktopCapturer, MacOsCaptureError};
pub use encoder::{FfmpegH264Encoder, FfmpegH264EncoderFactory, MacOsEncoderError};
pub use input::{AppleScriptKeyboardInput, AppleScriptMouseInput, MacOsClipboard, MacOsInputError};
pub use notification::MacOsSessionNotifier;
pub use providers::{MacOsDesktopProviders, MacOsProvidersError};

/// Return `true` when `program` can be found on `PATH`.
pub(crate) fn command_exists(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
}
