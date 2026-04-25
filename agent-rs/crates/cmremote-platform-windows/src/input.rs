// Source: CMRemote, clean-room implementation.

//! Windows desktop input drivers (slice R7.n.2).
//!
//! Implements the three input traits defined in
//! [`cmremote_platform::desktop::input`]:
//!
//! - [`WindowsMouseInput`] — `SendInput` with `MOUSEEVENTF_*` flags
//!   for buttons + wheel, `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE
//!   | MOUSEEVENTF_VIRTUALDESK` for `move_to`. Caller-supplied
//!   virtual-screen pixels are translated to the documented
//!   `0..=65535` normalised range over the entire virtual desktop.
//! - [`WindowsKeyboardInput`] — `SendInput` with
//!   `KEYEVENTF_UNICODE` for [`KeyCode::Char`] (so the host keyboard
//!   layout is preserved — the viewer's layout never leaks across)
//!   and well-known virtual-key codes (`VK_RETURN`, `VK_F1`, …) for
//!   [`NamedKey`]. Refuses control / DEL / Unicode bidi-override
//!   characters per the trait's security contract.
//! - [`WindowsClipboard`] — `OpenClipboard` / `EmptyClipboard` /
//!   `SetClipboardData(CF_UNICODETEXT)` and the matching read path,
//!   with a bounded payload size and **no** payload bytes ever
//!   echoed in error messages.
//!
//! ## Threading
//!
//! Every Win32 entry point used here is blocking; the public async
//! impls offload the work to [`tokio::task::spawn_blocking`]. The
//! clipboard is process-singleton on Windows (the system message
//! pump owns it), so the impl serialises its own access through a
//! [`Mutex`] to avoid `OpenClipboard` racing against itself when
//! the agent is driven by multiple viewer connections.
//!
//! ## Security contract
//!
//! - Char input refuses ASCII C0 controls (incl. NUL), DEL, and the
//!   Unicode bidi-override range that the project-wide
//!   [`cmremote_platform::desktop::guards`] module already refuses
//!   on the wire. This is defence-in-depth: even if a hostile
//!   viewer's chars somehow bypass the wire-layer guards, this
//!   driver still refuses them with
//!   [`DesktopInputError::InvalidParameters`].
//! - Function-key indices are validated to `1..=24`; `F(0)` and
//!   `F(>24)` are refused.
//! - Coordinates are normalised against the live virtual screen at
//!   call time; a hostile (`i32::MIN`/`i32::MAX`) coordinate is
//!   clamped to `[0, 65535]` after the saturating-arithmetic
//!   conversion so the kernel never sees a wrap-around value.
//! - Clipboard write payloads larger than [`MAX_CLIPBOARD_BYTES`]
//!   are refused with [`DesktopInputError::InvalidParameters`] —
//!   not silently truncated.
//! - No method ever logs the typed text or clipboard contents.
//!   `DesktopInputError::Io` messages contain only an
//!   implementation-defined OS error code.

use std::sync::{Mutex, MutexGuard, OnceLock};

use async_trait::async_trait;
use cmremote_platform::desktop::input::{
    Clipboard, DesktopInputError, KeyCode, KeyboardInput, MouseButton, MouseInput, NamedKey,
    ScrollAxis,
};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GLOBAL_ALLOC_FLAGS, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
    VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RCONTROL, VK_RETURN,
    VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    XBUTTON1, XBUTTON2,
};

/// Maximum clipboard payload accepted by [`WindowsClipboard::write_text`].
///
/// Picked to comfortably accommodate a multi-megabyte text snippet
/// while still bounding a hostile viewer's allocation request. The
/// constant is part of the public API so dependent code can match
/// it when constructing test inputs.
pub const MAX_CLIPBOARD_BYTES: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

/// Win32 [`SendInput`]-backed mouse driver.
///
/// Holds no mutable state; every method is independent. Wrap in an
/// `Arc` if you want to share across tasks, but a fresh instance is
/// also cheap.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsMouseInput;

impl WindowsMouseInput {
    /// Construct a new driver. Provided for symmetry with
    /// [`WindowsKeyboardInput::new`] and the keyboard / clipboard
    /// drivers.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MouseInput for WindowsMouseInput {
    async fn move_to(&self, x: i32, y: i32) -> Result<(), DesktopInputError> {
        spawn_blocking_input(move || {
            let (nx, ny) = normalise_to_virtual_screen(x, y);
            let input = build_mouse_input(
                nx as i32,
                ny as i32,
                0,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            );
            send_inputs(&[input])
        })
        .await
    }

    async fn button_down(&self, button: MouseButton) -> Result<(), DesktopInputError> {
        let (flags, mouse_data) = button_down_flags(button);
        spawn_blocking_input(move || {
            let input = build_mouse_input(0, 0, mouse_data, flags);
            send_inputs(&[input])
        })
        .await
    }

    async fn button_up(&self, button: MouseButton) -> Result<(), DesktopInputError> {
        let (flags, mouse_data) = button_up_flags(button);
        spawn_blocking_input(move || {
            let input = build_mouse_input(0, 0, mouse_data, flags);
            send_inputs(&[input])
        })
        .await
    }

    async fn scroll(&self, axis: ScrollAxis, delta: i32) -> Result<(), DesktopInputError> {
        let flags = match axis {
            ScrollAxis::Vertical => MOUSEEVENTF_WHEEL,
            ScrollAxis::Horizontal => MOUSEEVENTF_HWHEEL,
        };
        spawn_blocking_input(move || {
            // `mouseData` is documented as DWORD but carries a
            // signed wheel delta (`WHEEL_DELTA = 120` per notch,
            // negative = backward / left). Reinterpret the i32
            // bit pattern as u32 — the kernel sign-extends it
            // back internally.
            let input = build_mouse_input(0, 0, delta as u32, flags);
            send_inputs(&[input])
        })
        .await
    }
}

fn button_down_flags(
    button: MouseButton,
) -> (
    windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    u32,
) {
    match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, 0),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, 0),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, 0),
        MouseButton::X1 => (MOUSEEVENTF_XDOWN, XBUTTON1 as u32),
        MouseButton::X2 => (MOUSEEVENTF_XDOWN, XBUTTON2 as u32),
    }
}

fn button_up_flags(
    button: MouseButton,
) -> (
    windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    u32,
) {
    match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTUP, 0),
        MouseButton::Right => (MOUSEEVENTF_RIGHTUP, 0),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEUP, 0),
        MouseButton::X1 => (MOUSEEVENTF_XUP, XBUTTON1 as u32),
        MouseButton::X2 => (MOUSEEVENTF_XUP, XBUTTON2 as u32),
    }
}

fn build_mouse_input(
    dx: i32,
    dy: i32,
    mouse_data: u32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Translate a virtual-screen pixel into the documented
/// `[0, 65535]` normalised range required by
/// `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
///
/// Saturates on overflow and clamps the result so a hostile
/// (`i32::MIN`/`i32::MAX`) coordinate cannot wrap past the kernel's
/// expected range.
fn normalise_to_virtual_screen(x: i32, y: i32) -> (u16, u16) {
    // SAFETY: GetSystemMetrics has no failure mode; it returns 0
    // for unknown indices and never reads memory the caller owns.
    let (origin_x, origin_y, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    // Defensive: a degenerate metrics response (0×0 virtual screen
    // — never observed in practice but possible in service contexts
    // with no attached display) maps every input to (0, 0) rather
    // than dividing by zero.
    let width = width.max(1);
    let height = height.max(1);
    let nx = normalise_axis(x, origin_x, width);
    let ny = normalise_axis(y, origin_y, height);
    (nx, ny)
}

fn normalise_axis(value: i32, origin: i32, span: i32) -> u16 {
    // (value - origin) / (span - 1) * 65535, with saturation.
    let denom = (span - 1).max(1) as i64;
    let numer = (value as i64).saturating_sub(origin as i64) * 65535;
    let scaled = numer / denom;
    scaled.clamp(0, 65535) as u16
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

/// Win32 [`SendInput`]-backed keyboard driver.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsKeyboardInput;

impl WindowsKeyboardInput {
    /// Construct a new driver.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl KeyboardInput for WindowsKeyboardInput {
    async fn key_down(&self, key: &KeyCode) -> Result<(), DesktopInputError> {
        let inputs = build_key_inputs(key, false)?;
        spawn_blocking_input(move || send_inputs(&inputs)).await
    }

    async fn key_up(&self, key: &KeyCode) -> Result<(), DesktopInputError> {
        let inputs = build_key_inputs(key, true)?;
        spawn_blocking_input(move || send_inputs(&inputs)).await
    }

    async fn type_text(&self, text: &str) -> Result<(), DesktopInputError> {
        // Validate the entire string up front — any rejection MUST
        // leave the host's input queue untouched.
        for c in text.chars() {
            validate_char(c)?;
        }
        // UTF-16 encode and emit one down + up pair per code unit.
        // `KEYEVENTF_UNICODE` requires individual code units, so
        // surrogate pairs become two events of two inputs each.
        let mut inputs = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            inputs.push(build_unicode_input(unit, false));
            inputs.push(build_unicode_input(unit, true));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        spawn_blocking_input(move || send_inputs(&inputs)).await
    }
}

fn build_key_inputs(key: &KeyCode, key_up: bool) -> Result<Vec<INPUT>, DesktopInputError> {
    match key {
        KeyCode::Char(c) => {
            validate_char(*c)?;
            // A `char` is a single Unicode scalar value; emit one
            // INPUT per UTF-16 code unit (one for BMP, two for
            // anything in the supplementary planes).
            let mut buf = [0u16; 2];
            let units = c.encode_utf16(&mut buf);
            Ok(units
                .iter()
                .map(|u| build_unicode_input(*u, key_up))
                .collect())
        }
        KeyCode::Named(named) => {
            let (vk, extended) = named_to_vk(*named)?;
            Ok(vec![build_vk_input(vk, extended, key_up)])
        }
    }
}

fn validate_char(c: char) -> Result<(), DesktopInputError> {
    // ASCII C0 (including NUL) and DEL — same set the wire-layer
    // guards refuse, applied here as defence-in-depth.
    if c.is_control() || c == '\u{007F}' {
        return Err(DesktopInputError::InvalidParameters(
            "control character".into(),
        ));
    }
    // Unicode bidi-override / isolate range used in the
    // "Trojan Source" attack.
    if matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    ) {
        return Err(DesktopInputError::InvalidParameters(
            "bidi-override character".into(),
        ));
    }
    Ok(())
}

fn build_unicode_input(unit: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn build_vk_input(vk: VIRTUAL_KEY, extended: bool, key_up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn named_to_vk(named: NamedKey) -> Result<(VIRTUAL_KEY, bool), DesktopInputError> {
    // Returns (virtual-key, is_extended). Extended keys per MSDN:
    // arrows, Ins/Del/Home/End/PgUp/PgDn (NumPad-paired keys) and
    // the right-hand modifiers.
    Ok(match named {
        NamedKey::Enter => (VK_RETURN, false),
        NamedKey::Tab => (VK_TAB, false),
        NamedKey::Backspace => (VK_BACK, false),
        NamedKey::Delete => (VK_DELETE, true),
        NamedKey::Escape => (VK_ESCAPE, false),
        NamedKey::Space => (VK_SPACE, false),
        NamedKey::ArrowLeft => (VK_LEFT, true),
        NamedKey::ArrowRight => (VK_RIGHT, true),
        NamedKey::ArrowUp => (VK_UP, true),
        NamedKey::ArrowDown => (VK_DOWN, true),
        NamedKey::Home => (VK_HOME, true),
        NamedKey::End => (VK_END, true),
        NamedKey::PageUp => (VK_PRIOR, true),
        NamedKey::PageDown => (VK_NEXT, true),
        NamedKey::ShiftLeft => (VK_LSHIFT, false),
        NamedKey::ShiftRight => (VK_RSHIFT, false),
        NamedKey::ControlLeft => (VK_LCONTROL, false),
        NamedKey::ControlRight => (VK_RCONTROL, true),
        NamedKey::AltLeft => (VK_LMENU, false),
        NamedKey::AltRight => (VK_RMENU, true),
        NamedKey::MetaLeft => (VK_LWIN, true),
        NamedKey::MetaRight => (VK_RWIN, true),
        NamedKey::CapsLock => (VK_CAPITAL, false),
        NamedKey::F(n) => {
            if !(1..=24).contains(&n) {
                return Err(DesktopInputError::InvalidParameters(format!(
                    "F-key index {n} out of range 1..=24"
                )));
            }
            // VK_F1 = 0x70; VK_F1..VK_F24 are contiguous.
            (VIRTUAL_KEY(VK_F1.0 + (n as u16 - 1)), false)
        }
    })
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// Win32 clipboard driver.
///
/// The clipboard is a process-singleton on Windows; a static
/// [`Mutex`] serialises [`OpenClipboard`]/[`CloseClipboard`] across
/// every instance of the driver in this process to avoid the
/// well-known race where two callers each call `OpenClipboard` and
/// one quietly fails.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsClipboard;

impl WindowsClipboard {
    /// Construct a new driver.
    pub fn new() -> Self {
        Self
    }
}

fn clipboard_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[async_trait]
impl Clipboard for WindowsClipboard {
    async fn read_text(&self) -> Result<String, DesktopInputError> {
        spawn_blocking_clipboard(read_text_blocking).await
    }

    async fn write_text(&self, text: &str) -> Result<(), DesktopInputError> {
        if text.len() > MAX_CLIPBOARD_BYTES {
            return Err(DesktopInputError::InvalidParameters(format!(
                "clipboard payload exceeds {MAX_CLIPBOARD_BYTES}-byte limit"
            )));
        }
        // Pre-validate: refuse bidi-override / DEL like the rest of
        // the project. We allow control characters here because
        // clipboard text legitimately contains `\n` / `\t`.
        for c in text.chars() {
            if c == '\u{007F}' || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') {
                return Err(DesktopInputError::InvalidParameters(
                    "clipboard payload contains a forbidden character".into(),
                ));
            }
        }
        // UTF-16 encode with a NUL terminator (CF_UNICODETEXT
        // requires it).
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        spawn_blocking_clipboard(move || write_text_blocking(&wide)).await
    }
}

fn read_text_blocking() -> Result<String, DesktopInputError> {
    let _guard = clipboard_lock();
    open_clipboard()?;
    let result = (|| -> Result<String, DesktopInputError> {
        // SAFETY: GetClipboardData borrows the system-owned handle
        // for the duration we hold the clipboard open. We do NOT
        // GlobalFree the returned HGLOBAL — it remains the system's.
        let handle: HANDLE = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }
            .map_err(|e| DesktopInputError::Io(format!("GetClipboardData: {}", os_code(&e))))?;
        if handle.is_invalid() {
            // Empty / non-text clipboard — trait contract is to
            // return an empty string, not an error.
            return Ok(String::new());
        }
        let hglobal = HGLOBAL(handle.0);
        // SAFETY: GlobalSize / GlobalLock require a valid HGLOBAL,
        // which is what GetClipboardData returns on success.
        let size_bytes = unsafe { GlobalSize(hglobal) };
        if size_bytes == 0 {
            return Ok(String::new());
        }
        // SAFETY: GlobalLock returns a pointer valid until the
        // matching GlobalUnlock; we never let it escape this scope.
        let ptr = unsafe { GlobalLock(hglobal) } as *const u16;
        if ptr.is_null() {
            return Err(DesktopInputError::Io("GlobalLock returned null".into()));
        }
        let unlock_result = {
            let units = size_bytes / 2;
            // SAFETY: `ptr` is valid for `units` u16s per
            // `GlobalSize`. Stop at the first NUL — CF_UNICODETEXT
            // is documented as NUL-terminated, but we defensively
            // bound by `units` in case a producer omitted it.
            let slice = unsafe { std::slice::from_raw_parts(ptr, units) };
            let nul = slice.iter().position(|&u| u == 0).unwrap_or(units);
            Ok(String::from_utf16_lossy(&slice[..nul]))
        };
        // SAFETY: Always Unlock the handle we Locked, regardless
        // of the decode result.
        let _ = unsafe { GlobalUnlock(hglobal) };
        unlock_result
    })();
    // SAFETY: CloseClipboard is the documented inverse of
    // OpenClipboard; must always run.
    let _ = unsafe { CloseClipboard() };
    result
}

fn write_text_blocking(wide: &[u16]) -> Result<(), DesktopInputError> {
    let _guard = clipboard_lock();
    open_clipboard()?;
    let result = (|| -> Result<(), DesktopInputError> {
        // SAFETY: EmptyClipboard frees the prior contents
        // (transferring ownership back to us); only valid while we
        // hold the clipboard open.
        unsafe { EmptyClipboard() }
            .map_err(|e| DesktopInputError::Io(format!("EmptyClipboard: {}", os_code(&e))))?;

        let bytes = std::mem::size_of_val(wide);
        // SAFETY: GlobalAlloc returns a HGLOBAL or null on failure.
        let hmem = unsafe { GlobalAlloc(GLOBAL_ALLOC_FLAGS(GMEM_MOVEABLE.0), bytes) }
            .map_err(|e| DesktopInputError::Io(format!("GlobalAlloc: {}", os_code(&e))))?;
        if hmem.is_invalid() {
            return Err(DesktopInputError::Io("GlobalAlloc returned null".into()));
        }

        // SAFETY: GlobalLock pins a moveable HGLOBAL and returns a
        // pointer valid until the matching Unlock. We copy the
        // entire payload, then Unlock before passing the handle to
        // SetClipboardData (per MSDN guidance).
        let dst = unsafe { GlobalLock(hmem) } as *mut u16;
        if dst.is_null() {
            // SAFETY: Free the just-allocated HGLOBAL since we
            // never transferred ownership to the system.
            let _ = unsafe { GlobalFree(Some(hmem)) };
            return Err(DesktopInputError::Io("GlobalLock returned null".into()));
        }
        // SAFETY: `dst` points to `bytes` bytes (== `wide.len()` u16s);
        // `wide` is a Rust slice, so its length matches its allocation.
        unsafe {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(hmem);
        }

        // SAFETY: SetClipboardData transfers ownership of the
        // HGLOBAL to the system on success — we MUST NOT GlobalFree
        // it afterwards. On failure ownership stays with us, so we
        // free explicitly.
        match unsafe { SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0))) } {
            Ok(_) => Ok(()),
            Err(e) => {
                // SAFETY: SetClipboardData failed, so `hmem`
                // ownership is still ours.
                let _ = unsafe { GlobalFree(Some(hmem)) };
                Err(DesktopInputError::Io(format!(
                    "SetClipboardData: {}",
                    os_code(&e)
                )))
            }
        }
    })();
    // SAFETY: Always close the clipboard, even on error.
    let _ = unsafe { CloseClipboard() };
    result
}

fn open_clipboard() -> Result<(), DesktopInputError> {
    // SAFETY: `None` is documented as "associate the open clipboard
    // with the current task" — the idiomatic way to call
    // `OpenClipboard` from a process that does not own a window
    // (which is our case, since the agent runs as a service).
    // Returns FALSE if another process is currently holding the
    // clipboard; we surface that as a structured Io error so the
    // caller can retry.
    unsafe { OpenClipboard(None) }
        .map_err(|e| DesktopInputError::Io(format!("OpenClipboard: {}", os_code(&e))))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn os_code(e: &windows::core::Error) -> String {
    // Surface the HRESULT as an opaque numeric code only — never
    // include the OS-supplied message text, since on some Windows
    // versions it can interpolate process / handle names that
    // qualify as operator-supplied data.
    format!("HRESULT 0x{:08X}", e.code().0 as u32)
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), DesktopInputError> {
    if inputs.is_empty() {
        return Ok(());
    }
    let cb_size = std::mem::size_of::<INPUT>() as i32;
    // SAFETY: SendInput reads `cInputs * cbSize` bytes from the
    // pointer; both are derived from the slice we own. The
    // function returns the number of events actually inserted; a
    // short count indicates UIPI blocking or invalid parameters.
    let n = unsafe { SendInput(inputs, cb_size) };
    if n as usize != inputs.len() {
        return Err(DesktopInputError::Io(format!(
            "SendInput inserted {n}/{} events",
            inputs.len()
        )));
    }
    Ok(())
}

async fn spawn_blocking_input<F>(f: F) -> Result<(), DesktopInputError>
where
    F: FnOnce() -> Result<(), DesktopInputError> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(DesktopInputError::Io(format!("input task panicked: {e}"))),
    }
}

async fn spawn_blocking_clipboard<F, T>(f: F) -> Result<T, DesktopInputError>
where
    F: FnOnce() -> Result<T, DesktopInputError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(DesktopInputError::Io(format!(
            "clipboard task panicked: {e}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Pure-logic tests run on every host (Linux CI included via the
// cross-target check); injection / clipboard tests are `#[ignore]`
// because they touch the live OS input queue and clipboard.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_char_refuses_nul_and_control() {
        assert!(validate_char('\0').is_err());
        assert!(validate_char('\x07').is_err()); // BEL
        assert!(validate_char('\x7f').is_err()); // DEL
        assert!(validate_char('\u{202E}').is_err()); // RTL override
        assert!(validate_char('\u{2066}').is_err()); // LRI
        assert!(validate_char('a').is_ok());
        assert!(validate_char('é').is_ok());
        assert!(validate_char('字').is_ok());
        assert!(validate_char('😀').is_ok());
    }

    #[test]
    fn named_to_vk_function_keys_are_bounded() {
        assert!(named_to_vk(NamedKey::F(0)).is_err());
        assert!(named_to_vk(NamedKey::F(25)).is_err());
        assert!(named_to_vk(NamedKey::F(1)).is_ok());
        assert!(named_to_vk(NamedKey::F(24)).is_ok());
        let (vk1, _) = named_to_vk(NamedKey::F(1)).unwrap();
        let (vk24, _) = named_to_vk(NamedKey::F(24)).unwrap();
        assert_eq!(vk1.0, VK_F1.0);
        assert_eq!(vk24.0, VK_F1.0 + 23);
    }

    #[test]
    fn named_to_vk_marks_extended_keys_correctly() {
        // Extended-key flag must be set for the keys MSDN
        // documents as extended (arrows, navigation cluster, right
        // modifiers, NumPad Enter etc.).
        for (k, expected) in [
            (NamedKey::ArrowLeft, true),
            (NamedKey::ArrowRight, true),
            (NamedKey::ArrowUp, true),
            (NamedKey::ArrowDown, true),
            (NamedKey::Home, true),
            (NamedKey::End, true),
            (NamedKey::PageUp, true),
            (NamedKey::PageDown, true),
            (NamedKey::Delete, true),
            (NamedKey::ControlRight, true),
            (NamedKey::AltRight, true),
            (NamedKey::MetaLeft, true),
            (NamedKey::MetaRight, true),
            (NamedKey::Enter, false),
            (NamedKey::Tab, false),
            (NamedKey::ShiftLeft, false),
            (NamedKey::ControlLeft, false),
            (NamedKey::AltLeft, false),
            (NamedKey::Space, false),
            (NamedKey::Escape, false),
            (NamedKey::CapsLock, false),
        ] {
            let (_, ext) = named_to_vk(k).unwrap();
            assert_eq!(ext, expected, "{:?}", k);
        }
    }

    #[test]
    fn build_key_inputs_uses_unicode_for_chars() {
        // BMP char → 1 INPUT.
        let v = build_key_inputs(&KeyCode::Char('a'), false).unwrap();
        assert_eq!(v.len(), 1);
        unsafe {
            assert_eq!(v[0].r#type, INPUT_KEYBOARD);
            assert_eq!(v[0].Anonymous.ki.wScan, b'a' as u16);
            assert!((v[0].Anonymous.ki.dwFlags & KEYEVENTF_UNICODE).0 != 0);
            assert!((v[0].Anonymous.ki.dwFlags & KEYEVENTF_KEYUP).0 == 0);
        }
        // Supplementary-plane char → surrogate pair → 2 INPUTs.
        let v = build_key_inputs(&KeyCode::Char('😀'), true).unwrap();
        assert_eq!(v.len(), 2);
        unsafe {
            for i in &v {
                assert!((i.Anonymous.ki.dwFlags & KEYEVENTF_UNICODE).0 != 0);
                assert!((i.Anonymous.ki.dwFlags & KEYEVENTF_KEYUP).0 != 0);
            }
        }
    }

    #[test]
    fn build_key_inputs_refuses_control_chars_without_emitting_events() {
        assert!(matches!(
            build_key_inputs(&KeyCode::Char('\0'), false),
            Err(DesktopInputError::InvalidParameters(_))
        ));
        assert!(matches!(
            build_key_inputs(&KeyCode::Char('\u{202E}'), false),
            Err(DesktopInputError::InvalidParameters(_))
        ));
    }

    #[test]
    fn normalise_axis_clamps_extremes() {
        // Hostile coordinates must not wrap past the kernel's
        // expected [0, 65535] range.
        assert_eq!(normalise_axis(i32::MIN, 0, 1920), 0);
        assert_eq!(normalise_axis(i32::MAX, 0, 1920), 65535);
        assert_eq!(normalise_axis(0, 0, 1920), 0);
        assert_eq!(normalise_axis(1919, 0, 1920), 65535);
        // A negative virtual-screen origin (multi-monitor with the
        // primary monitor not at the top-left) must work too.
        assert_eq!(normalise_axis(-100, -100, 200), 0);
        assert_eq!(normalise_axis(99, -100, 200), 65535);
    }

    #[test]
    fn normalise_axis_handles_degenerate_span() {
        // 0×0 virtual screen (no display) — must not panic.
        let _ = normalise_axis(0, 0, 0);
        let _ = normalise_axis(0, 0, 1);
    }

    #[test]
    fn button_flags_round_trip() {
        // Sanity: each button's down flag pairs with the matching up
        // flag and the X-button mouse_data values are the documented
        // XBUTTON1/2 codes.
        let cases = [
            (
                MouseButton::Left,
                MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP,
                0_u32,
            ),
            (
                MouseButton::Right,
                MOUSEEVENTF_RIGHTDOWN,
                MOUSEEVENTF_RIGHTUP,
                0,
            ),
            (
                MouseButton::Middle,
                MOUSEEVENTF_MIDDLEDOWN,
                MOUSEEVENTF_MIDDLEUP,
                0,
            ),
            (
                MouseButton::X1,
                MOUSEEVENTF_XDOWN,
                MOUSEEVENTF_XUP,
                XBUTTON1 as u32,
            ),
            (
                MouseButton::X2,
                MOUSEEVENTF_XDOWN,
                MOUSEEVENTF_XUP,
                XBUTTON2 as u32,
            ),
        ];
        for (b, df, uf, md) in cases {
            assert_eq!(button_down_flags(b), (df, md));
            assert_eq!(button_up_flags(b), (uf, md));
        }
    }

    #[tokio::test]
    async fn write_text_refuses_oversize_payload_without_touching_clipboard() {
        let c = WindowsClipboard::new();
        let big = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        let r = c.write_text(&big).await;
        match r {
            Err(DesktopInputError::InvalidParameters(msg)) => {
                assert!(msg.contains("limit"));
                // The payload bytes themselves must NOT appear in
                // the rejection message.
                assert!(!msg.contains("xxxxx"));
            }
            other => panic!("expected InvalidParameters, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_text_refuses_bidi_override() {
        let c = WindowsClipboard::new();
        let payload = "hello\u{202E}world";
        let r = c.write_text(payload).await;
        assert!(matches!(r, Err(DesktopInputError::InvalidParameters(_))));
    }

    /// Trait-object safety — the agent runtime stores each driver
    /// behind `Box<dyn …>`.
    #[test]
    fn drivers_are_object_safe() {
        let _m: Box<dyn MouseInput> = Box::new(WindowsMouseInput::new());
        let _k: Box<dyn KeyboardInput> = Box::new(WindowsKeyboardInput::new());
        let _c: Box<dyn Clipboard> = Box::new(WindowsClipboard::new());
    }

    // ---- Live tests: require an interactive desktop session ----

    #[tokio::test]
    #[ignore = "moves the live cursor; requires an interactive Windows desktop"]
    async fn move_to_does_not_error_on_real_desktop() {
        let m = WindowsMouseInput::new();
        m.move_to(100, 100).await.expect("move_to");
    }

    #[tokio::test]
    #[ignore = "writes to the live clipboard; requires an interactive Windows desktop"]
    async fn clipboard_round_trips_unicode() {
        let c = WindowsClipboard::new();
        let payload = "héllo 😀 字";
        c.write_text(payload).await.expect("write");
        let got = c.read_text().await.expect("read");
        assert_eq!(got, payload);
    }
}
