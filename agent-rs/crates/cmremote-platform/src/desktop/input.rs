// Source: CMRemote, clean-room implementation.

//! Desktop input-injection trait surface (slice R7.h).
//!
//! These three traits are the seam a future WebRTC-data-channel
//! handler will use to apply viewer-side input events on the
//! controlled host. Slice R7.h ships the trait definitions, the
//! lightweight DTOs (`MouseButton`, `KeyCode`, `ScrollAxis`), the
//! [`DesktopInputError`] taxonomy, and `NotSupported*` defaults that
//! return a structured error naming the host OS — the same
//! "trait first, real driver later" pattern slices R6 (packages),
//! R7 (transport), and R7.c (capture/encode) used.
//!
//! ## Layering
//!
//! Mirrors [`super::media`]:
//!
//! 1. The wire layer never carries individual input events — they
//!    flow over the WebRTC data channel the eventual desktop driver
//!    opens. So this module ships no DTOs in `cmremote-wire`; the
//!    types here describe the *driver* contract.
//! 2. Three independently swappable traits — [`MouseInput`],
//!    [`KeyboardInput`], [`Clipboard`] — so a host can mix a
//!    hardware-accelerated mouse path (e.g. `SendInput` on Windows)
//!    with a software keyboard path (`uinput` on Linux) without any
//!    per-OS branching at the trait level.
//! 3. Default `NotSupported*` providers exist so the runtime never
//!    panics when no concrete driver is registered — instead every
//!    method returns [`DesktopInputError::NotSupported`] naming the
//!    host OS, just like [`super::media::NotSupportedDesktopCapturer`].
//!
//! ## Security contract
//!
//! Implementations MUST:
//!
//! 1. **Refuse to inject any event before the session has cleared
//!    [`super::consent`].** The default consent prompter denies every
//!    request; a concrete prompter that asks the operator only fires
//!    once per session, after the [`super::guards`] envelope checks
//!    have passed.
//! 2. **Bound burst rates.** A flood of `mouse_move` events from a
//!    hostile viewer must not lock up the host's input queue;
//!    implementations should coalesce moves to the most recent point
//!    and drop excess wheel ticks.
//! 3. **Never log the typed text or clipboard contents.** Both can
//!    contain operator-typed passwords; the only field a
//!    [`DesktopInputError::Io`] message may include is an
//!    OS-supplied error code. Refuse to surface the inbound bytes.
//! 4. **Re-resolve every key / button mapping locally.** The wire
//!    must not carry raw scan codes that bypass the host keyboard
//!    layout; the [`KeyCode`] enum is the only shape an event takes.

use async_trait::async_trait;
use thiserror::Error;

use crate::HostOs;

/// Errors surfaced by the three input-injection traits.
#[derive(Debug, Error)]
pub enum DesktopInputError {
    /// No driver implementation is registered for the current host.
    /// Returned by [`NotSupportedMouseInput`],
    /// [`NotSupportedKeyboardInput`], and [`NotSupportedClipboard`].
    #[error("desktop input is not supported on {0:?}")]
    NotSupported(HostOs),

    /// The viewer's session has not been granted on-host consent
    /// (see [`super::consent`]); injection is refused fail-closed.
    #[error("desktop input was denied by on-host consent")]
    ConsentDenied,

    /// Operating-system or driver I/O error. The string is
    /// implementation-defined and MUST NOT contain the typed text,
    /// the clipboard contents, or any operator-supplied identifier.
    /// An OS error code is permitted.
    #[error("desktop input I/O error: {0}")]
    Io(String),

    /// The driver rejected the request because the supplied
    /// parameters are out of range (e.g. `KeyCode::Char('\0')` or a
    /// move target outside the addressable virtual screen).
    #[error("desktop input parameters are invalid: {0}")]
    InvalidParameters(String),
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

/// Logical mouse button. The numeric repr matches the Win32 `XBUTTON1`
/// / `XBUTTON2` numbering so a future Windows driver can `as u32`
/// straight into `MOUSEEVENTF_*` flags without a per-button `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MouseButton {
    /// Primary button (left for right-handed users).
    Left = 1,
    /// Secondary button (right for right-handed users).
    Right = 2,
    /// Wheel button.
    Middle = 3,
    /// First extended button (typically "back").
    X1 = 4,
    /// Second extended button (typically "forward").
    X2 = 5,
}

/// Axis a wheel-scroll event affects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScrollAxis {
    /// Vertical wheel; positive deltas scroll the document up.
    Vertical,
    /// Horizontal wheel / tilt; positive deltas scroll right.
    Horizontal,
}

/// Injects pointer events on the controlled desktop.
///
/// Coordinates are absolute, in *virtual-screen* pixels (top-left
/// origin), so multi-monitor setups round-trip without per-display
/// translation in the wire layer.
#[async_trait]
pub trait MouseInput: Send + Sync {
    /// Move the cursor to the supplied virtual-screen pixel.
    /// Implementations MAY coalesce successive moves; only the most
    /// recent point is observable to the OS event queue.
    async fn move_to(&self, x: i32, y: i32) -> Result<(), DesktopInputError>;

    /// Press `button` (without releasing it).
    async fn button_down(&self, button: MouseButton) -> Result<(), DesktopInputError>;

    /// Release `button` (no-op if it was not held by this driver).
    async fn button_up(&self, button: MouseButton) -> Result<(), DesktopInputError>;

    /// Inject a wheel-tick event. `delta` is in WHEEL_DELTA units
    /// (120 per notch on Windows; the driver translates as needed
    /// for other hosts).
    async fn scroll(&self, axis: ScrollAxis, delta: i32) -> Result<(), DesktopInputError>;
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

/// Logical key the viewer wants to press.
///
/// The wire never carries hardware scan codes; instead the viewer
/// sends a logical [`KeyCode`] and the host driver re-resolves it
/// against the active keyboard layout. This means a German viewer
/// driving a US-layout host gets the layout the host expects, not
/// the layout that physically generated the event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// Single textual character. The driver MUST refuse `'\0'`,
    /// other ASCII control characters, and Unicode bidi-override
    /// code points (the same set the consent / guards modules
    /// refuse) so a hostile viewer cannot smuggle terminal escapes
    /// or invisible-formatting attacks through key injection.
    Char(char),
    /// Named non-printable key.
    Named(NamedKey),
}

/// Named non-character keys callers can inject.
///
/// Limited to keys with a stable cross-platform meaning. A driver
/// that needs a host-only key surfaces it through a higher-level
/// (driver-specific) extension so the trait surface stays portable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    /// Enter / Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Escape.
    Escape,
    /// Spacebar.
    Space,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Left Shift modifier.
    ShiftLeft,
    /// Right Shift modifier.
    ShiftRight,
    /// Left Control modifier.
    ControlLeft,
    /// Right Control modifier.
    ControlRight,
    /// Left Alt / Option modifier.
    AltLeft,
    /// Right Alt / Option modifier.
    AltRight,
    /// Left Meta / Win / Cmd modifier.
    MetaLeft,
    /// Right Meta / Win / Cmd modifier.
    MetaRight,
    /// CapsLock.
    CapsLock,
    /// Function-row key. `index` is 1-based (`F(1)` is `F1`); the
    /// driver MUST refuse out-of-range indices via
    /// [`DesktopInputError::InvalidParameters`].
    F(u8),
}

/// Injects keyboard events on the controlled desktop.
#[async_trait]
pub trait KeyboardInput: Send + Sync {
    /// Press `key` without releasing it. Modifier keys
    /// (`ShiftLeft`, `ControlLeft`, …) latch until a matching
    /// [`key_up`](Self::key_up).
    async fn key_down(&self, key: &KeyCode) -> Result<(), DesktopInputError>;

    /// Release `key` (no-op if it was not held by this driver).
    async fn key_up(&self, key: &KeyCode) -> Result<(), DesktopInputError>;

    /// Type a literal string as a sequence of character events,
    /// preserving the host's keyboard layout. The driver MUST refuse
    /// any character it would refuse via [`KeyCode::Char`] (NUL,
    /// other ASCII controls, bidi-override code points).
    async fn type_text(&self, text: &str) -> Result<(), DesktopInputError>;
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// Reads and writes the host's text clipboard on behalf of the
/// viewer. Implementations MUST treat both directions as bulk-data
/// surfaces — never log payload bytes, never include them in error
/// messages — because either direction can carry an operator-typed
/// password.
#[async_trait]
pub trait Clipboard: Send + Sync {
    /// Return the current text clipboard contents as UTF-8.
    /// Implementations that find non-text contents (image, file
    /// list) MUST return `Ok(String::new())` rather than synthesising
    /// a textual representation.
    async fn read_text(&self) -> Result<String, DesktopInputError>;

    /// Replace the host's text clipboard with `text`. The driver
    /// MAY refuse over-large payloads via
    /// [`DesktopInputError::InvalidParameters`].
    async fn write_text(&self, text: &str) -> Result<(), DesktopInputError>;
}

// ---------------------------------------------------------------------------
// `NotSupported*` defaults — one per trait. Mirror `NotSupportedDesktopCapturer`.
// ---------------------------------------------------------------------------

/// Default mouse driver returned by the runtime when no concrete
/// driver is registered. Always returns
/// [`DesktopInputError::NotSupported`] — never moves the pointer.
pub struct NotSupportedMouseInput {
    host_os: HostOs,
}

impl NotSupportedMouseInput {
    /// Build a driver that names `host_os` in its error.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }

    /// Build a driver that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedMouseInput {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl MouseInput for NotSupportedMouseInput {
    async fn move_to(&self, _x: i32, _y: i32) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
    async fn button_down(&self, _button: MouseButton) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
    async fn button_up(&self, _button: MouseButton) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
    async fn scroll(&self, _axis: ScrollAxis, _delta: i32) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
}

/// Default keyboard driver returned by the runtime when no concrete
/// driver is registered.
pub struct NotSupportedKeyboardInput {
    host_os: HostOs,
}

impl NotSupportedKeyboardInput {
    /// Build a driver that names `host_os` in its error.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }
    /// Build a driver that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedKeyboardInput {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl KeyboardInput for NotSupportedKeyboardInput {
    async fn key_down(&self, _key: &KeyCode) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
    async fn key_up(&self, _key: &KeyCode) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
    async fn type_text(&self, _text: &str) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
}

/// Default clipboard driver returned by the runtime when no concrete
/// driver is registered.
pub struct NotSupportedClipboard {
    host_os: HostOs,
}

impl NotSupportedClipboard {
    /// Build a driver that names `host_os` in its error.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }
    /// Build a driver that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedClipboard {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl Clipboard for NotSupportedClipboard {
    async fn read_text(&self) -> Result<String, DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
    async fn write_text(&self, _text: &str) -> Result<(), DesktopInputError> {
        Err(DesktopInputError::NotSupported(self.host_os))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn not_supported_mouse_returns_structured_error_naming_os() {
        let m = NotSupportedMouseInput::new(HostOs::Linux);
        let err = m.move_to(10, 20).await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("not supported"), "{s}");
        assert!(s.contains("Linux"), "{s}");
        assert!(matches!(
            m.button_down(MouseButton::Left).await.unwrap_err(),
            DesktopInputError::NotSupported(HostOs::Linux)
        ));
        assert!(matches!(
            m.button_up(MouseButton::Right).await.unwrap_err(),
            DesktopInputError::NotSupported(HostOs::Linux)
        ));
        assert!(matches!(
            m.scroll(ScrollAxis::Vertical, 120).await.unwrap_err(),
            DesktopInputError::NotSupported(HostOs::Linux)
        ));
    }

    #[tokio::test]
    async fn not_supported_keyboard_returns_structured_error_naming_os() {
        let k = NotSupportedKeyboardInput::new(HostOs::Windows);
        assert!(matches!(
            k.key_down(&KeyCode::Named(NamedKey::Enter))
                .await
                .unwrap_err(),
            DesktopInputError::NotSupported(HostOs::Windows)
        ));
        assert!(matches!(
            k.key_up(&KeyCode::Char('a')).await.unwrap_err(),
            DesktopInputError::NotSupported(HostOs::Windows)
        ));
        let s = k.type_text("hello").await.unwrap_err().to_string();
        assert!(s.contains("Windows"), "{s}");
    }

    #[tokio::test]
    async fn not_supported_clipboard_returns_structured_error_naming_os() {
        let c = NotSupportedClipboard::new(HostOs::MacOs);
        let s = c.read_text().await.unwrap_err().to_string();
        assert!(s.contains("MacOs"), "{s}");
        assert!(matches!(
            c.write_text("hi").await.unwrap_err(),
            DesktopInputError::NotSupported(HostOs::MacOs)
        ));
    }

    #[test]
    fn defaults_use_current_host() {
        let _: NotSupportedMouseInput = Default::default();
        let _: NotSupportedKeyboardInput = Default::default();
        let _: NotSupportedClipboard = Default::default();
        let _: NotSupportedMouseInput = NotSupportedMouseInput::for_current_host();
        let _: NotSupportedKeyboardInput = NotSupportedKeyboardInput::for_current_host();
        let _: NotSupportedClipboard = NotSupportedClipboard::for_current_host();
    }

    /// Trait-object safety check — every input trait must be storable
    /// behind `Box<dyn …>` so the runtime can hand them to the future
    /// WebRTC driver via dynamic dispatch.
    #[test]
    fn traits_are_object_safe() {
        let _m: Box<dyn MouseInput> = Box::new(NotSupportedMouseInput::for_current_host());
        let _k: Box<dyn KeyboardInput> = Box::new(NotSupportedKeyboardInput::for_current_host());
        let _c: Box<dyn Clipboard> = Box::new(NotSupportedClipboard::for_current_host());
    }

    #[test]
    fn mouse_button_repr_matches_documented_numbering() {
        // The numeric repr is part of the contract — a Windows
        // driver may rely on it. Pin it.
        assert_eq!(MouseButton::Left as u8, 1);
        assert_eq!(MouseButton::Right as u8, 2);
        assert_eq!(MouseButton::Middle as u8, 3);
        assert_eq!(MouseButton::X1 as u8, 4);
        assert_eq!(MouseButton::X2 as u8, 5);
    }

    #[test]
    fn key_code_round_trips_named_and_char_variants() {
        // The enum is the only shape an event takes on the wire
        // (slice R7.h does not ship a serialised form yet, but the
        // equality contract is what concrete drivers will dispatch
        // on, so pin it).
        assert_eq!(KeyCode::Char('z'), KeyCode::Char('z'));
        assert_ne!(KeyCode::Char('z'), KeyCode::Char('Z'));
        assert_eq!(
            KeyCode::Named(NamedKey::F(1)),
            KeyCode::Named(NamedKey::F(1))
        );
        assert_ne!(
            KeyCode::Named(NamedKey::F(1)),
            KeyCode::Named(NamedKey::F(2))
        );
        assert_ne!(
            KeyCode::Named(NamedKey::ShiftLeft),
            KeyCode::Named(NamedKey::ShiftRight)
        );
    }

    #[test]
    fn error_messages_do_not_require_payload_bytes() {
        // Sanity: the structured error contract pins the message to
        // a fixed shape that cannot inadvertently include a
        // typed-text or clipboard payload.
        let e = DesktopInputError::Io("EACCES".into());
        let s = e.to_string();
        assert!(s.contains("desktop input I/O error"));
        assert!(s.contains("EACCES"));
        let denied = DesktopInputError::ConsentDenied.to_string();
        assert!(denied.contains("consent"));
        let bad = DesktopInputError::InvalidParameters("F(13)".into()).to_string();
        assert!(bad.contains("invalid"));
    }
}
