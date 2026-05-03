// Source: CMRemote, clean-room implementation.

//! Linux-specific desktop capture / encode / input providers for the
//! CMRemote agent.
//!
//! This crate follows the sibling `cmremote-platform-windows` crate's
//! layering: `cmremote-platform` remains the safe trait crate, while
//! this target crate owns host-specific provider construction. The
//! initial Linux implementation intentionally uses native command-line
//! integration points that are already present on common desktop
//! installations (`xwd`, `xdotool`, `wl-copy` / `wl-paste`, `xclip`,
//! and `ffmpeg`) instead of introducing a large D-Bus / PipeWire / GTK
//! dependency graph in one change. Provider construction is fail-closed:
//! if a required executable is absent, the agent runtime falls back to
//! the structured `NotSupported` bundle rather than advertising a
//! partially-working desktop driver.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod capture;
pub mod encoder;
pub mod input;
pub mod notification;
pub mod providers;

pub use capture::{LinuxCaptureError, XwdDesktopCapturer};
pub use encoder::{FfmpegH264Encoder, FfmpegH264EncoderFactory, LinuxEncoderError};
pub use input::{LinuxClipboard, LinuxInputError, XdotoolKeyboardInput, XdotoolMouseInput};
pub use notification::NotifySendSessionNotifier;
pub use providers::{LinuxDesktopProviders, LinuxProvidersError};

/// Return `true` when `program` can be found on `PATH`.
pub(crate) fn command_exists(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
}
