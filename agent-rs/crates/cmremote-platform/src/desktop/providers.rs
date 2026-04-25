// Source: CMRemote, clean-room implementation.

//! Per-host bundle of desktop capability providers (slice R7.n.4).
//!
//! The agent's WebRTC desktop transport needs four collaborating
//! providers — a screen capturer, a mouse-input driver, a
//! keyboard-input driver, and a clipboard driver — all configured
//! for the same host. Slices R7.h (input traits), R7.c (capture
//! traits), and R7.n (Windows DXGI / `SendInput` impls) ship the
//! individual building blocks; this module wires them into a
//! single owned bundle that the runtime can construct once and
//! pass around as a unit.
//!
//! ## Why a struct, not a trait
//!
//! A trait would force every consumer to call four accessor
//! methods and clone four `Arc`s on every WebRTC track / data
//! channel construction. The bundle is *data* — four `Arc<dyn
//! …>` slots — so a plain struct is the simplest representation
//! and keeps `DesktopProviders` `Clone`-able and `Send + Sync`
//! without further trait gymnastics.
//!
//! ## Per-OS construction
//!
//! - On every host, [`DesktopProviders::not_supported_for_current_host`]
//!   produces a bundle whose four slots all return
//!   `NotSupported(<host_os>)` from every method. This is the
//!   *fallback* used when no concrete OS driver is available
//!   (non-Windows hosts today; Windows hosts where the agent is
//!   running in Session 0 / a non-interactive session).
//! - The Windows agent constructs a real bundle via
//!   `cmremote_platform_windows::WindowsDesktopProviders::for_primary_output`,
//!   which composes the DXGI capturer with the three `SendInput` /
//!   `CF_UNICODETEXT` drivers. Other OSes follow the same pattern
//!   when their drivers land.
//!
//! ## Security
//!
//! `DesktopProviders` is purely a container — it neither validates
//! nor mediates calls into the underlying providers. The trait
//! contracts on [`DesktopCapturer`], [`MouseInput`],
//! [`KeyboardInput`], and [`Clipboard`] are unchanged; in
//! particular the consent gate ([`super::consent`]) and the wire
//! envelope guards ([`super::guards`]) remain the responsibility
//! of the call site that *uses* a slot, not the bundle itself.

use std::sync::Arc;

use super::input::{
    Clipboard, KeyboardInput, MouseInput, NotSupportedClipboard, NotSupportedKeyboardInput,
    NotSupportedMouseInput,
};
use super::media::{DesktopCapturer, NotSupportedDesktopCapturer};
use crate::HostOs;

/// Owned bundle of the four desktop-capability providers for one
/// host.
///
/// `Clone` is intentionally derived — every field is an `Arc`, so
/// cloning a bundle is just four atomic increments and lets the
/// runtime hand a fresh bundle to each WebRTC session without
/// reconstructing the underlying drivers.
#[derive(Clone)]
pub struct DesktopProviders {
    /// Captures the host desktop as a sequence of [`super::media::CapturedFrame`]s.
    pub capturer: Arc<dyn DesktopCapturer>,
    /// Injects pointer events on the host.
    pub mouse: Arc<dyn MouseInput>,
    /// Injects keyboard events on the host.
    pub keyboard: Arc<dyn KeyboardInput>,
    /// Reads / writes the host's text clipboard.
    pub clipboard: Arc<dyn Clipboard>,
}

impl DesktopProviders {
    /// Build a bundle whose four slots all report `NotSupported`
    /// against `host_os`.
    ///
    /// Used as the fallback bundle when the runtime cannot build a
    /// real per-OS bundle (no driver registered, or — on Windows —
    /// the agent is running in a non-interactive session where
    /// `SendInput` would silently inject nothing). Calls into any
    /// slot return [`super::input::DesktopInputError::NotSupported`]
    /// or [`super::media::DesktopMediaError::NotSupported`] as
    /// appropriate.
    pub fn not_supported_for(host_os: HostOs) -> Self {
        Self {
            capturer: Arc::new(NotSupportedDesktopCapturer::new(host_os)),
            mouse: Arc::new(NotSupportedMouseInput::new(host_os)),
            keyboard: Arc::new(NotSupportedKeyboardInput::new(host_os)),
            clipboard: Arc::new(NotSupportedClipboard::new(host_os)),
        }
    }

    /// Build a bundle whose four slots all report `NotSupported`
    /// against the current host's OS — derived via
    /// [`HostOs::current`].
    pub fn not_supported_for_current_host() -> Self {
        Self::not_supported_for(HostOs::current())
    }
}

impl std::fmt::Debug for DesktopProviders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The underlying `dyn` providers don't necessarily impl
        // `Debug` (the trait bound would force every concrete
        // driver to derive it), so report only the bundle's
        // shape — never include the underlying provider's state,
        // which on a real Windows bundle would touch live D3D11 /
        // clipboard handles.
        f.debug_struct("DesktopProviders")
            .field("capturer", &"<dyn DesktopCapturer>")
            .field("mouse", &"<dyn MouseInput>")
            .field("keyboard", &"<dyn KeyboardInput>")
            .field("clipboard", &"<dyn Clipboard>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::input::{KeyCode, MouseButton, ScrollAxis};

    #[tokio::test]
    async fn not_supported_bundle_returns_structured_errors_naming_os() {
        let p = DesktopProviders::not_supported_for(HostOs::Linux);

        // Capturer
        let e = p.capturer.capture_next_frame().await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");

        // Mouse
        let e = p.mouse.move_to(0, 0).await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");
        let e = p.mouse.button_down(MouseButton::Left).await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");
        let e = p.mouse.scroll(ScrollAxis::Vertical, 120).await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");

        // Keyboard
        let e = p.keyboard.key_down(&KeyCode::Char('a')).await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");
        let e = p.keyboard.type_text("hi").await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");

        // Clipboard
        let e = p.clipboard.read_text().await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");
        let e = p.clipboard.write_text("hi").await.unwrap_err();
        assert!(e.to_string().contains("Linux"), "{e}");
    }

    #[test]
    fn for_current_host_compiles_and_runs() {
        let _ = DesktopProviders::not_supported_for_current_host();
    }

    #[test]
    fn bundle_is_clone_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DesktopProviders>();
        let p = DesktopProviders::not_supported_for(HostOs::Windows);
        let p2 = p.clone();
        // Each clone is a fresh Arc to the same underlying provider —
        // pin both the trait-object safety and the `Clone` semantics.
        let _: Arc<dyn DesktopCapturer> = p.capturer;
        let _: Arc<dyn DesktopCapturer> = p2.capturer;
    }

    #[test]
    fn debug_does_not_leak_provider_state() {
        let p = DesktopProviders::not_supported_for(HostOs::Windows);
        let s = format!("{p:?}");
        assert!(s.contains("DesktopProviders"));
        // The fallback providers carry the host OS internally; the
        // Debug impl deliberately hides every concrete field so a
        // future real-driver bundle can't accidentally leak handle
        // values / file paths through `tracing` formatting.
        assert!(!s.contains("Windows"), "{s}");
    }
}
