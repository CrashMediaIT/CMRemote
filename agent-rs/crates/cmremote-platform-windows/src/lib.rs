// Source: CMRemote, clean-room implementation.

//! Windows-specific desktop capture / input providers for the
//! CMRemote agent (slice R7.n — Windows leg).
//!
//! ## Why a separate crate
//!
//! `cmremote-platform` carries `#![forbid(unsafe_code)]`, which is a
//! load-bearing guarantee about the trait crate (the `NotSupported*`
//! defaults, the wire DTOs, the session-state machine, the guards —
//! none of those need `unsafe` and an audit reviewer can rely on the
//! attribute to skip the whole tree). DXGI Desktop Duplication is
//! pure COM FFI through the [`windows`] crate, which **does** need
//! `unsafe`. Splitting this code into a sibling crate preserves the
//! safety invariant on the trait crate while still letting the
//! Windows agent runtime compose a real capturer with zero changes
//! to the trait surface.
//!
//! ## Layering
//!
//! 1. [`cmremote_platform::desktop::DesktopCapturer`] — the trait the
//!    capturer implements. Defined in `cmremote-platform`.
//! 2. [`WindowsDesktopCapturer`] — this crate's concrete
//!    implementation, only present on `target_os = "windows"`.
//! 3. The agent runtime (slice R7.n.2) chooses
//!    `WindowsDesktopCapturer::for_primary_output()` over
//!    [`cmremote_platform::desktop::NotSupportedDesktopCapturer`] when
//!    `cfg!(target_os = "windows")`. That wiring lives in
//!    `cmremote-agent`, not here, so this crate stays a pure
//!    capability provider.
//!
//! ## Cross-platform compilation
//!
//! On non-Windows targets the crate compiles to an empty library so
//! the workspace can depend on it unconditionally without
//! `--exclude` gymnastics in CI. The `windows` dep is itself
//! target-gated in `Cargo.toml`, so non-Windows hosts never download
//! or compile any of the generated bindings.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]
// Allow `unsafe` inside the `capture` module specifically — the
// crate-level default is still `deny`, so any `unsafe` block must
// be opt-in per item with a `// SAFETY:` justification.
#![deny(unsafe_code)]

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub mod capture;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub mod input;

#[cfg(target_os = "windows")]
pub use capture::{WindowsCaptureError, WindowsDesktopCapturer};

#[cfg(target_os = "windows")]
pub use input::{WindowsClipboard, WindowsKeyboardInput, WindowsMouseInput};
