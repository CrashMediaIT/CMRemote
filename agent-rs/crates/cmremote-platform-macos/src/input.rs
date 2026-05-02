// Source: CMRemote, clean-room implementation.

//! macOS input and clipboard providers.

use async_trait::async_trait;
use cmremote_platform::desktop::{
    Clipboard, DesktopInputError, KeyCode, KeyboardInput, MouseButton, MouseInput, NamedKey,
    ScrollAxis,
};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Errors surfaced by macOS input providers.
#[derive(Debug, Error)]
pub enum MacOsInputError {
    /// A required executable is not available.
    #[error("missing executable: {0}")]
    MissingCommand(&'static str),
    /// A command failed.
    #[error("command failed: {0}")]
    Process(String),
    /// The input could not be mapped safely.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl From<MacOsInputError> for DesktopInputError {
    fn from(value: MacOsInputError) -> Self {
        match value {
            MacOsInputError::MissingCommand(_) => {
                DesktopInputError::NotSupported(cmremote_platform::HostOs::MacOs)
            }
            MacOsInputError::Process(e) => DesktopInputError::Io(e),
            MacOsInputError::InvalidInput(e) => DesktopInputError::InvalidParameters(e),
        }
    }
}

/// Mouse driver backed by the macOS `cliclick` utility.
#[derive(Debug, Default)]
pub struct AppleScriptMouseInput;

impl AppleScriptMouseInput {
    /// Construct after checking `cliclick` exists.
    pub fn new() -> Result<Self, MacOsInputError> {
        if !crate::command_exists("cliclick") {
            return Err(MacOsInputError::MissingCommand("cliclick"));
        }
        Ok(Self)
    }
}

#[async_trait]
impl MouseInput for AppleScriptMouseInput {
    async fn move_to(&self, x: i32, y: i32) -> Result<(), DesktopInputError> {
        if x < 0 || y < 0 {
            return Err(DesktopInputError::InvalidParameters(
                "negative coordinates".into(),
            ));
        }
        run_cliclick([format!("m:{x},{y}")]).await
    }

    async fn button_down(&self, button: MouseButton) -> Result<(), DesktopInputError> {
        run_cliclick([format!("dd:{}", cliclick_button(button))]).await
    }

    async fn button_up(&self, button: MouseButton) -> Result<(), DesktopInputError> {
        run_cliclick([format!("du:{}", cliclick_button(button))]).await
    }

    async fn scroll(&self, axis: ScrollAxis, delta: i32) -> Result<(), DesktopInputError> {
        if delta == 0 {
            return Ok(());
        }
        let units = (delta / 120).clamp(-32, 32);
        let arg = match axis {
            ScrollAxis::Vertical => format!("w:0,{units}"),
            ScrollAxis::Horizontal => format!("w:{units},0"),
        };
        run_cliclick([arg]).await
    }
}

/// Keyboard driver backed by AppleScript `System Events`.
#[derive(Debug, Default)]
pub struct AppleScriptKeyboardInput;

impl AppleScriptKeyboardInput {
    /// Construct after checking `osascript` exists.
    pub fn new() -> Result<Self, MacOsInputError> {
        if !crate::command_exists("osascript") {
            return Err(MacOsInputError::MissingCommand("osascript"));
        }
        Ok(Self)
    }
}

#[async_trait]
impl KeyboardInput for AppleScriptKeyboardInput {
    async fn key_down(&self, key: &KeyCode) -> Result<(), DesktopInputError> {
        let code = key_code(key)?;
        run_osascript(&format!(
            "tell application \"System Events\" to key down {code}"
        ))
        .await
    }

    async fn key_up(&self, key: &KeyCode) -> Result<(), DesktopInputError> {
        let code = key_code(key)?;
        run_osascript(&format!(
            "tell application \"System Events\" to key up {code}"
        ))
        .await
    }

    async fn type_text(&self, text: &str) -> Result<(), DesktopInputError> {
        validate_text(text)?;
        run_osascript(&format!(
            "tell application \"System Events\" to keystroke {}",
            applescript_string(text)
        ))
        .await
    }
}

/// Clipboard driver backed by `pbcopy` and `pbpaste`.
#[derive(Debug, Default)]
pub struct MacOsClipboard;

impl MacOsClipboard {
    /// Construct after checking for `pbcopy` and `pbpaste`.
    pub fn new() -> Result<Self, MacOsInputError> {
        if !crate::command_exists("pbcopy") {
            return Err(MacOsInputError::MissingCommand("pbcopy"));
        }
        if !crate::command_exists("pbpaste") {
            return Err(MacOsInputError::MissingCommand("pbpaste"));
        }
        Ok(Self)
    }
}

#[async_trait]
impl Clipboard for MacOsClipboard {
    async fn read_text(&self) -> Result<String, DesktopInputError> {
        let output = Command::new("pbpaste")
            .output()
            .await
            .map_err(|e| DesktopInputError::Io(format!("pbpaste spawn failed: {e}")))?;
        if !output.status.success() {
            return Err(DesktopInputError::Io(format!(
                "pbpaste exited {:?}",
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
        let mut child = Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| DesktopInputError::Io(format!("pbcopy spawn failed: {e}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| DesktopInputError::Io("pbcopy stdin unavailable".into()))?;
        stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| DesktopInputError::Io(format!("pbcopy write failed: {e}")))?;
        drop(stdin);
        let status = child
            .wait()
            .await
            .map_err(|e| DesktopInputError::Io(format!("pbcopy wait failed: {e}")))?;
        if !status.success() {
            return Err(DesktopInputError::Io(format!(
                "pbcopy exited {:?}",
                status.code()
            )));
        }
        Ok(())
    }
}

async fn run_cliclick<const N: usize>(args: [String; N]) -> Result<(), DesktopInputError> {
    let status = Command::new("cliclick")
        .args(args)
        .status()
        .await
        .map_err(|e| DesktopInputError::Io(format!("cliclick spawn failed: {e}")))?;
    if !status.success() {
        return Err(DesktopInputError::Io(format!(
            "cliclick exited {:?}",
            status.code()
        )));
    }
    Ok(())
}

async fn run_osascript(script: &str) -> Result<(), DesktopInputError> {
    let status = Command::new("osascript")
        .args(["-e", script])
        .status()
        .await
        .map_err(|e| DesktopInputError::Io(format!("osascript spawn failed: {e}")))?;
    if !status.success() {
        return Err(DesktopInputError::Io(format!(
            "osascript exited {:?}",
            status.code()
        )));
    }
    Ok(())
}

fn cliclick_button(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => ".",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::X1 => "button4",
        MouseButton::X2 => "button5",
    }
}

fn key_code(key: &KeyCode) -> Result<u16, DesktopInputError> {
    match key {
        KeyCode::Char(c) => char_key_code(*c),
        KeyCode::Named(n) => named_key_code(*n),
    }
}

fn char_key_code(c: char) -> Result<u16, DesktopInputError> {
    validate_char(c)?;
    Ok(match c.to_ascii_lowercase() {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '6' => 22,
        '5' => 23,
        '=' => 24,
        '9' => 25,
        '7' => 26,
        '-' => 27,
        '8' => 28,
        '0' => 29,
        ']' => 30,
        'o' => 31,
        'u' => 32,
        '[' => 33,
        'i' => 34,
        'p' => 35,
        '\n' | '\r' => 36,
        'l' => 37,
        'j' => 38,
        '\'' => 39,
        'k' => 40,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        'n' => 45,
        'm' => 46,
        '.' => 47,
        '`' => 50,
        ' ' => 49,
        _ => {
            return Err(DesktopInputError::InvalidParameters(
                "character cannot be mapped to a stable macOS key code".into(),
            ))
        }
    })
}

fn named_key_code(key: NamedKey) -> Result<u16, DesktopInputError> {
    Ok(match key {
        NamedKey::Enter => 36,
        NamedKey::Tab => 48,
        NamedKey::Backspace => 51,
        NamedKey::Delete => 117,
        NamedKey::Escape => 53,
        NamedKey::Space => 49,
        NamedKey::ArrowLeft => 123,
        NamedKey::ArrowRight => 124,
        NamedKey::ArrowDown => 125,
        NamedKey::ArrowUp => 126,
        NamedKey::Home => 115,
        NamedKey::End => 119,
        NamedKey::PageUp => 116,
        NamedKey::PageDown => 121,
        NamedKey::ShiftLeft | NamedKey::ShiftRight => 56,
        NamedKey::ControlLeft | NamedKey::ControlRight => 59,
        NamedKey::AltLeft | NamedKey::AltRight => 58,
        NamedKey::MetaLeft | NamedKey::MetaRight => 55,
        NamedKey::CapsLock => 57,
        NamedKey::F(n) if (1..=20).contains(&n) => 121 + u16::from(n),
        NamedKey::F(_) => {
            return Err(DesktopInputError::InvalidParameters(
                "function key out of range".into(),
            ))
        }
    })
}

fn applescript_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
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
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    {
        return Err(DesktopInputError::InvalidParameters(
            "refused control/bidi character".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_string_escapes_quotes_and_backslashes() {
        assert_eq!(applescript_string("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn key_mapping_covers_common_keys() {
        assert_eq!(char_key_code('a').unwrap(), 0);
        assert_eq!(named_key_code(NamedKey::ArrowLeft).unwrap(), 123);
        assert!(named_key_code(NamedKey::F(21)).is_err());
    }

    #[test]
    fn text_validation_rejects_bidi_controls() {
        assert!(validate_text("hello").is_ok());
        assert!(validate_text("bad\u{202e}").is_err());
    }
}
