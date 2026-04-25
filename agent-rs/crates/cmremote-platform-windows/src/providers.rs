// Source: CMRemote, clean-room implementation.

//! Windows bundle factory for [`DesktopProviders`] (slice R7.n.4).
//!
//! Composes the four Windows desktop drivers from this crate —
//! [`WindowsDesktopCapturer`], [`WindowsMouseInput`],
//! [`WindowsKeyboardInput`], [`WindowsClipboard`] — into a single
//! [`DesktopProviders`] bundle the agent runtime can pass around as
//! a unit. Refuses construction when the host's session topology
//! makes input injection impossible (Session 0 / detached console
//! / mismatched RDP), so the runtime falls back to the
//! `NotSupported*` bundle and the operator sees a structured
//! "agent has no interactive desktop attached" failure rather than
//! `SendInput` silently swallowing every event.
//!
//! ## Layering
//!
//! ```text
//!   WindowsSessionInfo::current()
//!         │ refuse if !can_inject_input()
//!         ▼
//!   WindowsDesktopCapturer::for_primary_output()  ─┐
//!   WindowsMouseInput::new()                       │
//!   WindowsKeyboardInput::new()                    ├──► DesktopProviders
//!   WindowsClipboard::new()                        ─┘
//! ```
//!
//! ## Threading
//!
//! Construction is synchronous and cheap (the capturer's D3D11
//! device is created lazily on the first frame; the input drivers
//! and clipboard hold no resources). The returned bundle is
//! `Clone + Send + Sync`.
//!
//! ## Security
//!
//! - Only the **primary output** of the **default adapter** is
//!   captured (matches the slice R7.n.1 capturer's hard-pinned
//!   policy — no operator-supplied display id).
//! - The session-gating check is fail-closed: any error from
//!   [`WindowsSessionInfo::current`] aborts construction with
//!   [`WindowsProvidersError::Session`] rather than assuming the
//!   agent is interactive.
//! - All structured errors carry only opaque `HRESULT 0x...` codes
//!   inherited from the underlying drivers — never typed text,
//!   clipboard contents, file paths, or operator-supplied
//!   identifiers.

use std::sync::Arc;

use cmremote_platform::desktop::DesktopProviders;
use thiserror::Error;

use crate::capture::{WindowsCaptureError, WindowsDesktopCapturer};
use crate::input::{WindowsClipboard, WindowsKeyboardInput, WindowsMouseInput};
use crate::session::{WindowsSessionError, WindowsSessionInfo};

/// Errors surfaced by [`WindowsDesktopProviders::for_primary_output`].
#[derive(Debug, Error)]
pub enum WindowsProvidersError {
    /// The Windows session topology rules out input injection — the
    /// agent is in Session 0, no console session is currently
    /// attached, or the agent is in a different session than the
    /// console. The runtime should fall back to the `NotSupported`
    /// bundle so desktop-control requests surface a structured
    /// failure instead of `SendInput` silently swallowing events.
    ///
    /// The embedded [`WindowsSessionInfo`] is the snapshot that
    /// caused the refusal — useful in the agent's startup log.
    #[error("Windows session is not eligible for input injection: {0:?}")]
    NotInteractive(WindowsSessionInfo),

    /// Failed to query the Windows session topology.
    #[error(transparent)]
    Session(#[from] WindowsSessionError),

    /// Failed to construct the DXGI desktop capturer.
    #[error(transparent)]
    Capture(#[from] WindowsCaptureError),
}

/// Factory namespace for Windows desktop bundles.
///
/// Stateless — every call to [`WindowsDesktopProviders::for_primary_output`]
/// builds a fresh bundle. The factory does **not** retain any
/// references to the constructed providers.
pub struct WindowsDesktopProviders;

impl WindowsDesktopProviders {
    /// Build a [`DesktopProviders`] bundle wired to the primary
    /// output of the default adapter, with the live Windows session
    /// gating rule applied.
    ///
    /// Returns:
    /// - `Ok(bundle)` when the agent is in an interactive session
    ///   sharing the active console (i.e. `SendInput` is observable
    ///   to the operator at the host).
    /// - `Err(WindowsProvidersError::NotInteractive)` when the
    ///   session topology rules injection out (Session 0, detached
    ///   console, RDP-vs-console split) — the runtime should fall
    ///   back to [`DesktopProviders::not_supported_for_current_host`].
    /// - `Err(WindowsProvidersError::Session)` if the topology query
    ///   itself failed (rare; surfaces an opaque `HRESULT`).
    /// - `Err(WindowsProvidersError::Capture)` if the DXGI capturer
    ///   could not initialise (no D3D11 device, no primary output
    ///   on the default adapter).
    pub fn for_primary_output() -> Result<DesktopProviders, WindowsProvidersError> {
        let session = WindowsSessionInfo::current()?;
        if !session.can_inject_input() {
            return Err(WindowsProvidersError::NotInteractive(session));
        }
        Self::for_primary_output_with_session(session)
    }

    /// Build a bundle, skipping the session-gating check by accepting
    /// a caller-supplied [`WindowsSessionInfo`] snapshot.
    ///
    /// Used by [`for_primary_output`](Self::for_primary_output) once
    /// the live snapshot has cleared the gate. Public so tests can
    /// exercise the construction path with a synthetic session
    /// (the live `WindowsSessionInfo::current` requires a real
    /// Windows host).
    ///
    /// Still rejects when `!session.can_inject_input()` — the
    /// gating rule is part of the contract, not an optimisation.
    pub fn for_primary_output_with_session(
        session: WindowsSessionInfo,
    ) -> Result<DesktopProviders, WindowsProvidersError> {
        if !session.can_inject_input() {
            return Err(WindowsProvidersError::NotInteractive(session));
        }
        let capturer = WindowsDesktopCapturer::for_primary_output()?;
        Ok(DesktopProviders {
            capturer: Arc::new(capturer),
            mouse: Arc::new(WindowsMouseInput::new()),
            keyboard: Arc::new(WindowsKeyboardInput::new()),
            clipboard: Arc::new(WindowsClipboard::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_session_zero() {
        // Session 0 — services / LocalSystem; SendInput is a no-op.
        let s = WindowsSessionInfo::new(0, Some(0));
        let r = WindowsDesktopProviders::for_primary_output_with_session(s);
        match r {
            Err(WindowsProvidersError::NotInteractive(snap)) => {
                assert_eq!(snap, s);
                let msg = format!("{}", WindowsProvidersError::NotInteractive(s));
                assert!(msg.contains("not eligible"), "{msg}");
            }
            other => panic!("expected NotInteractive, got {other:?}"),
        }
    }

    #[test]
    fn rejects_detached_console() {
        let s = WindowsSessionInfo::new(2, None);
        let r = WindowsDesktopProviders::for_primary_output_with_session(s);
        assert!(matches!(r, Err(WindowsProvidersError::NotInteractive(_))));
    }

    #[test]
    fn rejects_mismatched_console() {
        // RDP session 3 while the physical console belongs to
        // session 1 — agent in session 3 must not inject because
        // the operator is looking at a different desktop.
        let s = WindowsSessionInfo::new(3, Some(1));
        let r = WindowsDesktopProviders::for_primary_output_with_session(s);
        assert!(matches!(r, Err(WindowsProvidersError::NotInteractive(_))));
    }

    #[test]
    fn error_messages_are_opaque() {
        // Structured display — must include the snapshot but no
        // operator-supplied data.
        let s = WindowsSessionInfo::new(0, Some(0));
        let msg = format!("{}", WindowsProvidersError::NotInteractive(s));
        assert!(msg.contains("Windows session"), "{msg}");
        // No path-shaped or quoted-string sub-strings that could
        // contain operator data.
        assert!(!msg.contains("\\\\"), "{msg}");
    }

    /// Live test: actually constructs the DXGI capturer + the three
    /// input drivers. Requires a real Windows host with a primary
    /// output and an interactive session, so it's `#[ignore]` for
    /// CI but runnable locally with `cargo test -- --ignored`.
    #[test]
    #[ignore = "constructs the live DXGI capturer; requires a real interactive Windows desktop"]
    fn for_primary_output_constructs_real_bundle_on_interactive_host() {
        let bundle = WindowsDesktopProviders::for_primary_output().expect("bundle");
        // Smoke-check trait-object slots so a future refactor can't
        // accidentally swap a slot for the wrong concrete type.
        let _: &dyn cmremote_platform::desktop::DesktopCapturer = &*bundle.capturer;
        let _: &dyn cmremote_platform::desktop::MouseInput = &*bundle.mouse;
        let _: &dyn cmremote_platform::desktop::KeyboardInput = &*bundle.keyboard;
        let _: &dyn cmremote_platform::desktop::Clipboard = &*bundle.clipboard;
    }
}
