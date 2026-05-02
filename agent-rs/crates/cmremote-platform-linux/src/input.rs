// Source: CMRemote, clean-room implementation.

//! Linux input and clipboard drivers backed by desktop command tools.

use async_trait::async_trait;
use cmremote_platform::desktop::{
    Clipboard, DesktopInputError, KeyCode, KeyboardInput, MouseButton, MouseInput, NamedKey,
    ScrollAxis,
};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Errors surfaced by Linux input providers.
#[derive(Debug, Error)]
pub enum LinuxInputError {
    /// A required executable is not available.
    #[error("missing executable: {0}")]
    MissingCommand(&'static str),
    /// A command failed.
    #[error("command failed: {0}")]
    Process(String),
    /// The requested key or text is not supported by this driver.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl From<LinuxInputError> for DesktopInputError {
    fn from(value: LinuxInputError) -> Self {
        match value {
            LinuxInputError::MissingCommand(_) => {
                DesktopInputError::NotSupported(cmremote_platform::HostOs::Linux)
            }
            LinuxInputError::Process(e) => DesktopInputError::Io(e),
            LinuxInputError::InvalidInput(e) => DesktopInputError::InvalidParameters(e),
        }
    }
}

/// Mouse driver backed by `xdotool`.
#[derive(Debug, Default)]
pub struct XdotoolMouseInput;

impl XdotoolMouseInput {
    /// Construct after checking for `xdotool`.
    pub fn new() -> Result<Self, LinuxInputError> {
        if !crate::command_exists("xdotool") {
            return Err(LinuxInputError::MissingCommand("xdotool"));
        }
        Ok(Self)
    }
}

#[async_trait]
impl MouseInput for XdotoolMouseInput {
    async fn move_to(&self, x: i32, y: i32) -> Result<(), DesktopInputError> {
        if x < 0 || y < 0 {
            return Err(DesktopInputError::InvalidParameters(
                "negative coordinates".into(),
            ));
        }
        run_xdotool(["mousemove", &x.to_string(), &y.to_string()]).await
    }

    async fn button_down(&self, button: MouseButton) -> Result<(), DesktopInputError> {
        run_xdotool(["mousedown", mouse_button_arg(button)]).await
    }

    async fn button_up(&self, button: MouseButton) -> Result<(), DesktopInputError> {
        run_xdotool(["mouseup", mouse_button_arg(button)]).await
    }

    async fn scroll(&self, axis: ScrollAxis, delta: i32) -> Result<(), DesktopInputError> {
        let button = match (axis, delta.cmp(&0)) {
            (ScrollAxis::Vertical, std::cmp::Ordering::Greater) => "4",
            (ScrollAxis::Vertical, std::cmp::Ordering::Less) => "5",
            (ScrollAxis::Horizontal, std::cmp::Ordering::Greater) => "7",
            (ScrollAxis::Horizontal, std::cmp::Ordering::Less) => "6",
            (_, std::cmp::Ordering::Equal) => return Ok(()),
        };
        let clicks = (delta.unsigned_abs() / 120).max(1).min(32);
        for _ in 0..clicks {
            run_xdotool(["click", button]).await?;
        }
        Ok(())
    }
}

/// Keyboard driver backed by `xdotool`.
#[derive(Debug, Default)]
pub struct XdotoolKeyboardInput;

impl XdotoolKeyboardInput {
    /// Construct after checking for `xdotool`.
    pub fn new() -> Result<Self, LinuxInputError> {
        if !crate::command_exists("xdotool") {
            return Err(LinuxInputError::MissingCommand("xdotool"));
        }
        Ok(Self)
    }
}

#[async_trait]
impl KeyboardInput for XdotoolKeyboardInput {
    async fn key_down(&self, key: &KeyCode) -> Result<(), DesktopInputError> {
        let key = key_name(key)?;
        run_xdotool(["keydown", key.as_str()]).await
    }

    async fn key_up(&self, key: &KeyCode) -> Result<(), DesktopInputError> {
        let key = key_name(key)?;
        run_xdotool(["keyup", key.as_str()]).await
    }

    async fn type_text(&self, text: &str) -> Result<(), DesktopInputError> {
        validate_text(text)?;
        run_xdotool(["type", "--clearmodifiers", text]).await
    }
}

/// Clipboard driver backed by `wl-copy` / `wl-paste` when present,
/// otherwise `xclip`.
#[derive(Debug, Clone)]
pub enum LinuxClipboard {
    /// Wayland wl-clipboard implementation.
    WlClipboard,
    /// X11 xclip implementation.
    Xclip,
}

impl LinuxClipboard {
    /// Pick the first available clipboard command pair.
    pub fn new() -> Result<Self, LinuxInputError> {
        if crate::command_exists("wl-copy") && crate::command_exists("wl-paste") {
            Ok(Self::WlClipboard)
        } else if crate::command_exists("xclip") {
            Ok(Self::Xclip)
        } else {
            Err(LinuxInputError::MissingCommand("wl-copy/wl-paste or xclip"))
        }
    }
}

#[async_trait]
impl Clipboard for LinuxClipboard {
    async fn read_text(&self) -> Result<String, DesktopInputError> {
        let output = match self {
            LinuxClipboard::WlClipboard => {
                Command::new("wl-paste")
                    .args(["--no-newline"])
                    .output()
                    .await
            }
            LinuxClipboard::Xclip => {
                Command::new("xclip")
                    .args(["-selection", "clipboard", "-out"])
                    .output()
                    .await
            }
        }
        .map_err(|e| DesktopInputError::Io(format!("clipboard read failed: {e}")))?;
        if !output.status.success() {
            return Err(DesktopInputError::Io(format!(
                "clipboard read command exited {:?}",
                output.status.code()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn write_text(&self, text: &str) -> Result<(), DesktopInputError> {
        if text.len() > 1024 * 1024 {
            return Err(DesktopInputError::InvalidParameters(
                "clipboard text exceeds 1 MiB".into(),
            ));
        }
        let mut child = match self {
            LinuxClipboard::WlClipboard => Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn(),
            LinuxClipboard::Xclip => Command::new("xclip")
                .args(["-selection", "clipboard", "-in"])
                .stdin(std::process::Stdio::piped())
                .spawn(),
        }
        .map_err(|e| DesktopInputError::Io(format!("clipboard write spawn failed: {e}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| DesktopInputError::Io("clipboard stdin unavailable".into()))?;
        stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| DesktopInputError::Io(format!("clipboard write failed: {e}")))?;
        drop(stdin);
        let status = child
            .wait()
            .await
            .map_err(|e| DesktopInputError::Io(format!("clipboard command wait failed: {e}")))?;
        if !status.success() {
            return Err(DesktopInputError::Io(format!(
                "clipboard write command exited {:?}",
                status.code()
            )));
        }
        Ok(())
    }
}

async fn run_xdotool<const N: usize>(args: [&str; N]) -> Result<(), DesktopInputError> {
    let status = Command::new("xdotool")
        .args(args)
        .status()
        .await
        .map_err(|e| DesktopInputError::Io(format!("xdotool spawn failed: {e}")))?;
    if !status.success() {
        return Err(DesktopInputError::Io(format!(
            "xdotool exited {:?}",
            status.code()
        )));
    }
    Ok(())
}

fn mouse_button_arg(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "1",
        MouseButton::Middle => "2",
        MouseButton::Right => "3",
        MouseButton::X1 => "8",
        MouseButton::X2 => "9",
    }
}

fn key_name(key: &KeyCode) -> Result<String, DesktopInputError> {
    match key {
        KeyCode::Char(c) => {
            validate_char(*c)?;
            Ok(c.to_string())
        }
        KeyCode::Named(n) => named_key(*n).map(str::to_owned),
    }
}

fn named_key(key: NamedKey) -> Result<&'static str, DesktopInputError> {
    Ok(match key {
        NamedKey::Enter => "Return",
        NamedKey::Tab => "Tab",
        NamedKey::Backspace => "BackSpace",
        NamedKey::Delete => "Delete",
        NamedKey::Escape => "Escape",
        NamedKey::Space => "space",
        NamedKey::ArrowLeft => "Left",
        NamedKey::ArrowRight => "Right",
        NamedKey::ArrowUp => "Up",
        NamedKey::ArrowDown => "Down",
        NamedKey::Home => "Home",
        NamedKey::End => "End",
        NamedKey::PageUp => "Page_Up",
        NamedKey::PageDown => "Page_Down",
        NamedKey::ShiftLeft => "Shift_L",
        NamedKey::ShiftRight => "Shift_R",
        NamedKey::ControlLeft => "Control_L",
        NamedKey::ControlRight => "Control_R",
        NamedKey::AltLeft => "Alt_L",
        NamedKey::AltRight => "Alt_R",
        NamedKey::MetaLeft => "Super_L",
        NamedKey::MetaRight => "Super_R",
        NamedKey::CapsLock => "Caps_Lock",
        NamedKey::F(n) if (1..=24).contains(&n) => {
            return Ok(Box::leak(format!("F{n}").into_boxed_str()))
        }
        NamedKey::F(_) => {
            return Err(DesktopInputError::InvalidParameters(
                "function key out of range".into(),
            ))
        }
    })
}

fn validate_text(text: &str) -> Result<(), DesktopInputError> {
    for c in text.chars() {
        validate_char(c)?;
    }
    Ok(())
}

fn validate_char(c: char) -> Result<(), DesktopInputError> {
    if c == '\0'
        || (c.is_ascii_control() && c != '\n' && c != '\r' && c != '\t')
        || is_bidi_override(c)
    {
        return Err(DesktopInputError::InvalidParameters(
            "refused control/bidi character".into(),
        ));
    }
    Ok(())
}

fn is_bidi_override(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_buttons_match_xdotool_numbering() {
        assert_eq!(mouse_button_arg(MouseButton::Left), "1");
        assert_eq!(mouse_button_arg(MouseButton::Right), "3");
        assert_eq!(mouse_button_arg(MouseButton::X2), "9");
    }

    #[test]
    fn named_keys_map_to_x11_names() {
        assert_eq!(named_key(NamedKey::Enter).unwrap(), "Return");
        assert_eq!(named_key(NamedKey::ArrowLeft).unwrap(), "Left");
        assert!(named_key(NamedKey::F(25)).is_err());
    }

    #[test]
    fn text_validation_rejects_bidi_controls() {
        assert!(validate_text("hello").is_ok());
        assert!(validate_text("bad\u{202e}").is_err());
    }
}
