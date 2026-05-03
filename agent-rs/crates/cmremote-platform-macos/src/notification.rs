// Source: CMRemote, clean-room implementation.

//! macOS desktop-session notifications.

use async_trait::async_trait;
use cmremote_platform::desktop::{SessionNotification, SessionNotifier};
use tokio::process::Command;

/// Session notifier backed by `osascript display notification`.
#[derive(Debug, Default)]
pub struct MacOsSessionNotifier;

impl MacOsSessionNotifier {
    /// Construct after checking `osascript` exists.
    pub fn new() -> Option<Self> {
        crate::command_exists("osascript").then_some(Self)
    }
}

#[async_trait]
impl SessionNotifier for MacOsSessionNotifier {
    async fn session_connected(&self, notification: &SessionNotification) {
        run_osascript_notification(
            "CMRemote connected",
            &format!(
                "{} from {} is viewing this device.",
                notification.requester_name, notification.org_name
            ),
        )
        .await;
    }

    async fn session_disconnected(&self, notification: &SessionNotification, reason: &str) {
        run_osascript_notification(
            "CMRemote disconnected",
            &format!(
                "{} from {} stopped viewing this device ({reason}).",
                notification.requester_name, notification.org_name
            ),
        )
        .await;
    }
}

async fn run_osascript_notification(title: &str, body: &str) {
    let script = format!(
        "display notification {} with title {}",
        applescript_string(body),
        applescript_string(title),
    );
    match Command::new("osascript")
        .args(["-e", &script])
        .status()
        .await
    {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(
            exit_code = ?status.code(),
            event = "desktop-session-notification-failed",
            "osascript notification exited unsuccessfully",
        ),
        Err(e) => tracing::warn!(
            error = %e,
            event = "desktop-session-notification-failed",
            "failed to spawn osascript notification",
        ),
    }
}

fn applescript_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_string_escapes_quotes_and_backslashes() {
        assert_eq!(applescript_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
