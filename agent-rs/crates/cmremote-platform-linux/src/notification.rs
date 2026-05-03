// Source: CMRemote, clean-room implementation.

//! Linux desktop-session notifications.

use async_trait::async_trait;
use cmremote_platform::desktop::{SessionNotification, SessionNotifier};
use tokio::process::Command;

/// Session notifier backed by `notify-send`.
#[derive(Debug, Default)]
pub struct NotifySendSessionNotifier;

impl NotifySendSessionNotifier {
    /// Construct after checking `notify-send` exists.
    pub fn new() -> Option<Self> {
        crate::command_exists("notify-send").then_some(Self)
    }
}

#[async_trait]
impl SessionNotifier for NotifySendSessionNotifier {
    async fn session_connected(&self, notification: &SessionNotification) {
        run_notify_send(
            "CMRemote connected",
            &format!(
                "{} from {} is viewing this device.",
                notification.requester_name, notification.org_name
            ),
        )
        .await;
    }

    async fn session_disconnected(&self, notification: &SessionNotification, reason: &str) {
        run_notify_send(
            "CMRemote disconnected",
            &format!(
                "{} from {} stopped viewing this device ({reason}).",
                notification.requester_name, notification.org_name
            ),
        )
        .await;
    }
}

async fn run_notify_send(summary: &str, body: &str) {
    match Command::new("notify-send")
        .args(["--app-name=CMRemote", summary, body])
        .status()
        .await
    {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(
            exit_code = ?status.code(),
            event = "desktop-session-notification-failed",
            "notify-send exited unsuccessfully",
        ),
        Err(e) => tracing::warn!(
            error = %e,
            event = "desktop-session-notification-failed",
            "failed to spawn notify-send",
        ),
    }
}
