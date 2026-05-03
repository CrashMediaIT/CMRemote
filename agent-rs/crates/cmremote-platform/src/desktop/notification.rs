// Source: CMRemote, clean-room implementation.

//! On-host unattended-access notification surface (slice R7).
//!
//! CMRemote is an unattended-access utility: a valid operator session
//! must not block on a prompt at the controlled machine. The host still
//! needs a clear, local indication that a remote desktop session is
//! active, so this module provides the notification seam every
//! concrete platform integration uses.
//!
//! ## Layering
//!
//! 1. The wire layer never carries a local notification decision. The
//!    desktop transport validates and accepts/refuses sessions based on
//!    its existing guards, then emits host-local notifications as a
//!    side effect.
//! 2. [`SessionNotifier`] is a `Send + Sync` async trait so a real
//!    notifier can dispatch a Windows toast, Linux desktop
//!    notification, macOS notification, tray indicator, or service log
//!    entry without blocking the agent runtime.
//! 3. The default [`LoggingSessionNotifier`] never refuses a session.
//!    Notification delivery is best-effort and must not make
//!    unattended access depend on someone clicking a prompt.
//!
//! ## Security contract
//!
//! Every notifier MUST:
//!
//! 1. **Sanitise operator-supplied strings before rendering them.**
//!    [`SessionNotification::sanitised`] runs the same
//!    [`super::guards::validate_operator_string`] checks as the
//!    desktop transport's envelope guards, so a hostile org name with
//!    bidi-override characters cannot disguise itself in the local UI.
//! 2. **Fail open for session flow, but fail safe for display.** A
//!    notifier that cannot reach the host UI must log / drop the
//!    notification and return; it must not block the already-authorised
//!    unattended session.
//! 3. **Log only sanitised fields.** Never log raw inbound strings or
//!    sensitive fields such as access keys.

use async_trait::async_trait;

use super::guards::validate_operator_string;
#[cfg(test)]
use super::guards::MAX_OPERATOR_STRING_LEN;

/// Sanitised local-notification payload for one remote desktop
/// session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNotification {
    /// Canonical lowercase-UUID session id.
    pub session_id: String,
    /// Display name of the viewer requesting access.
    pub requester_name: String,
    /// Display name of the viewer's organisation.
    pub org_name: String,
    /// SignalR connection id of the viewer.
    pub viewer_connection_id: String,
}

impl SessionNotification {
    /// Build a local-notification payload from the raw fields carried
    /// by the desktop transport. Returns `Err(field_message)` if any
    /// field fails the shared operator-string contract.
    pub fn sanitised(
        session_id: impl Into<String>,
        requester_name: impl Into<String>,
        org_name: impl Into<String>,
        viewer_connection_id: impl Into<String>,
    ) -> Result<Self, String> {
        let session_id = session_id.into();
        let requester_name = requester_name.into();
        let org_name = org_name.into();
        let viewer_connection_id = viewer_connection_id.into();

        validate_operator_string("session_id", &session_id)?;
        validate_operator_string("requester_name", &requester_name)?;
        validate_operator_string("org_name", &org_name)?;
        validate_operator_string("viewer_connection_id", &viewer_connection_id)?;

        Ok(Self {
            session_id,
            requester_name,
            org_name,
            viewer_connection_id,
        })
    }
}

/// Host-local notification sink for desktop-session lifecycle events.
#[async_trait]
pub trait SessionNotifier: Send + Sync {
    /// Notify that a remote desktop session has started.
    async fn session_connected(&self, notification: &SessionNotification);

    /// Notify that a remote desktop session has ended.
    async fn session_disconnected(&self, notification: &SessionNotification, reason: &str);
}

/// Default notifier used when no native OS notification driver is
/// configured. It records structured log events and never blocks or
/// refuses the session.
#[derive(Debug, Default)]
pub struct LoggingSessionNotifier;

#[async_trait]
impl SessionNotifier for LoggingSessionNotifier {
    async fn session_connected(&self, notification: &SessionNotification) {
        tracing::info!(
            session_id = %notification.session_id,
            requester_name = %notification.requester_name,
            org_name = %notification.org_name,
            viewer_connection_id = %notification.viewer_connection_id,
            event = "desktop-session-connected",
            "remote desktop session connected",
        );
    }

    async fn session_disconnected(&self, notification: &SessionNotification, reason: &str) {
        tracing::info!(
            session_id = %notification.session_id,
            requester_name = %notification.requester_name,
            org_name = %notification.org_name,
            viewer_connection_id = %notification.viewer_connection_id,
            reason = reason,
            event = "desktop-session-disconnected",
            "remote desktop session disconnected",
        );
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod testing {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;

    /// Captured notification event used by tests.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum CapturedNotificationEvent {
        /// Connected event.
        Connected(SessionNotification),
        /// Disconnected event and reason.
        Disconnected(SessionNotification, String),
    }

    /// Test notifier that stores every event in memory.
    #[derive(Debug, Default, Clone)]
    pub(crate) struct CapturingSessionNotifier {
        events: Arc<Mutex<Vec<CapturedNotificationEvent>>>,
    }

    impl CapturingSessionNotifier {
        /// Create an empty capturing notifier.
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Return a snapshot of captured events.
        pub(crate) async fn events(&self) -> Vec<CapturedNotificationEvent> {
            self.events.lock().await.clone()
        }
    }

    #[async_trait]
    impl SessionNotifier for CapturingSessionNotifier {
        async fn session_connected(&self, notification: &SessionNotification) {
            self.events
                .lock()
                .await
                .push(CapturedNotificationEvent::Connected(notification.clone()));
        }

        async fn session_disconnected(&self, notification: &SessionNotification, reason: &str) {
            self.events
                .lock()
                .await
                .push(CapturedNotificationEvent::Disconnected(
                    notification.clone(),
                    reason.to_string(),
                ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";

    fn ok_notification() -> SessionNotification {
        SessionNotification::sanitised(VALID_SESSION_ID, "Alice", "Acme", "viewer-1").unwrap()
    }

    #[tokio::test]
    async fn logging_notifier_allows_connected_and_disconnected_events() {
        let notifier = LoggingSessionNotifier;
        let notification = ok_notification();
        notifier.session_connected(&notification).await;
        notifier
            .session_disconnected(&notification, "test-complete")
            .await;
    }

    #[test]
    fn sanitised_constructor_accepts_a_clean_notification() {
        let n = ok_notification();
        assert_eq!(n.session_id, VALID_SESSION_ID);
        assert_eq!(n.requester_name, "Alice");
        assert_eq!(n.org_name, "Acme");
        assert_eq!(n.viewer_connection_id, "viewer-1");
    }

    #[test]
    fn sanitised_constructor_refuses_bidi_override_in_org_name() {
        let err =
            SessionNotification::sanitised(VALID_SESSION_ID, "Alice", "Acme\u{202E}", "viewer-1")
                .unwrap_err();
        assert!(err.contains("org_name"));
        assert!(err.contains("bidi-override"));
    }

    #[test]
    fn sanitised_constructor_refuses_embedded_nul() {
        let err = SessionNotification::sanitised(VALID_SESSION_ID, "Alice\0", "Acme", "viewer-1")
            .unwrap_err();
        assert!(err.contains("requester_name"));
        assert!(err.contains("non-printable"));
    }

    #[test]
    fn sanitised_constructor_refuses_over_length_org_name() {
        let huge = "x".repeat(MAX_OPERATOR_STRING_LEN + 1);
        let err = SessionNotification::sanitised(VALID_SESSION_ID, "Alice", huge, "viewer-1")
            .unwrap_err();
        assert!(err.contains("org_name"));
        assert!(err.contains("limit"));
    }

    #[test]
    fn sanitised_constructor_refuses_empty_required_fields() {
        let err =
            SessionNotification::sanitised(VALID_SESSION_ID, "", "Acme", "viewer-1").unwrap_err();
        assert!(err.contains("requester_name"));
        assert!(err.contains("empty"));
    }

    #[test]
    fn sanitised_constructor_refuses_control_in_viewer_connection_id() {
        let err =
            SessionNotification::sanitised(VALID_SESSION_ID, "Alice", "Acme", "viewer-1\u{0007}")
                .unwrap_err();
        assert!(err.contains("viewer_connection_id"));
        assert!(err.contains("non-printable"));
    }

    /// Trait-object safety check — the notifier must be storable
    /// behind `Box<dyn …>` so the runtime can swap the logging default
    /// for a per-OS notification implementation at startup.
    #[test]
    fn notifier_is_object_safe() {
        let _n: Box<dyn SessionNotifier> = Box::new(LoggingSessionNotifier);
    }
}
