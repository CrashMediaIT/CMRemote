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
pub mod encoder;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub mod input;

// `session` is intentionally NOT cfg-gated to `target_os = "windows"`:
// the pure-logic helpers (`is_session_zero`, `is_in_console_session`,
// `can_inject_input`) need to be callable from cross-platform code
// (e.g. the agent runtime on Linux deciding whether to surface a
// "Windows-only" capability for a Windows agent), and their unit
// tests should run on every CI host. Only the live Win32 entry
// point [`session::WindowsSessionInfo::current`] is cfg-gated
// internally — on non-Windows targets it returns a structured
// `Io` error.
#[allow(unsafe_code)]
pub mod session;

// `providers` is the Windows bundle factory for the cross-crate
// `DesktopProviders` abstraction in `cmremote-platform`. Only
// available on Windows because it composes the DXGI capturer +
// SendInput drivers from the modules above.
#[cfg(target_os = "windows")]
pub mod providers;

#[cfg(target_os = "windows")]
pub use capture::{WindowsCaptureError, WindowsDesktopCapturer};

#[cfg(target_os = "windows")]
pub use encoder::{
    WindowsEncoderError, WindowsVideoEncoder, WindowsVideoEncoderConfig,
    WindowsVideoEncoderFactory,
};

#[cfg(target_os = "windows")]
pub use input::{WindowsClipboard, WindowsKeyboardInput, WindowsMouseInput};

pub use session::{WindowsSessionError, WindowsSessionInfo};

#[cfg(target_os = "windows")]
pub use providers::{WindowsDesktopProviders, WindowsProvidersError};
