// Source: CMRemote, clean-room implementation.

//! Windows desktop-session notifications.

use async_trait::async_trait;
use cmremote_platform::desktop::{SessionNotification, SessionNotifier};
use tokio::process::Command;

/// Session notifier backed by Windows `msg.exe`.
#[derive(Debug, Default)]
pub struct WindowsSessionNotifier;

#[async_trait]
impl SessionNotifier for WindowsSessionNotifier {
    async fn session_connected(&self, notification: &SessionNotification) {
        run_msg(&format!(
            "CMRemote connected: {} from {} is viewing this device.",
            notification.requester_name, notification.org_name
        ))
        .await;
    }

    async fn session_disconnected(&self, notification: &SessionNotification, reason: &str) {
        run_msg(&format!(
            "CMRemote disconnected: {} from {} stopped viewing this device ({reason}).",
            notification.requester_name, notification.org_name
        ))
        .await;
    }
}

async fn run_msg(message: &str) {
    match Command::new("msg").args(["*", message]).status().await {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(
            exit_code = ?status.code(),
            event = "desktop-session-notification-failed",
            "msg.exe exited unsuccessfully",
        ),
        Err(e) => tracing::warn!(
            error = %e,
            event = "desktop-session-notification-failed",
            "failed to spawn msg.exe",
        ),
    }
}
