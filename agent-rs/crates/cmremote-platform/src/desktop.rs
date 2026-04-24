// Source: CMRemote, clean-room implementation.

//! Desktop-transport provider trait and safety-stub default
//! (slice R7 — *initial* trait + stub; the WebRTC capture / encode
//! driver lands in a follow-up PR).
//!
//! Re-derived from `Shared/Interfaces/IAgentHubClient.cs` ➜ the four
//! desktop-transport methods (`RemoteControl`, `RestartScreenCaster`,
//! `ChangeWindowsSession`, `InvokeCtrlAltDel`) and the .NET
//! `IAppLauncher` interface that backs them.
//!
//! ## Layering
//!
//! Mirrors the pattern slice R6 used for package providers:
//!
//! 1. The wire layer ([`cmremote_wire::desktop`]) defines the
//!    PascalCase request DTOs and a generic [`DesktopTransportResult`]
//!    response shape.
//! 2. This module defines [`DesktopTransportProvider`] — a `Send +
//!    Sync` async trait with one method per .NET hub call. Every
//!    implementation must respect the security contract documented
//!    below.
//! 3. The agent binary's dispatch layer routes
//!    `RemoteControl` / `RestartScreenCaster` / `ChangeWindowsSession`
//!    / `InvokeCtrlAltDel` invocations to the registered provider
//!    rather than the generic `not_implemented` fall-through.
//! 4. The default [`NotSupportedDesktopTransport`] returns a
//!    structured failure on every call, naming the host OS in the
//!    error message — never panics. Concrete WebRTC-backed providers
//!    plug in alongside it without further wire/contract churn.
//!
//! ## Security contract
//!
//! Every implementation MUST:
//!
//! 1. **Never echo `access_key` into logs or error strings.** The
//!    field is sensitive — treat it the same as the slice R6
//!    `auth_header` value: if it has to traverse `tracing`, mark it
//!    `Sensitive` or scrub it first.
//! 2. **Treat every operator-supplied string as untrusted UTF-8.**
//!    `requester_name` / `org_name` reach the on-host consent prompt
//!    and may be rendered in a UI; clamp lengths and refuse
//!    non-printable code points before they propagate.
//! 3. **Refuse cross-org sessions.** If a request's `org_id` does
//!    not match `ConnectionInfo::organization_id`, the implementation
//!    must return [`DesktopTransportResult::failed`] with a clear
//!    message — silently servicing the request would let a server
//!    bug or compromise pivot a viewer onto a device it does not own.
//! 4. **Re-resolve every executable from the OS.** The agent never
//!    execs a command string carried on the wire; the provider must
//!    locate the screencaster binary by a fixed local path.

use async_trait::async_trait;
use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, InvokeCtrlAltDelRequest,
    RemoteControlSessionRequest, RestartScreenCasterRequest,
};

use crate::HostOs;

/// Async trait every desktop-transport backend implements.
///
/// Each method maps 1:1 to one of the four desktop hub invocations
/// the .NET server can issue. The agent's dispatcher decodes the
/// invocation into the matching request DTO and forwards it here;
/// the provider is responsible for everything from consent prompting
/// to capture / encode / WebRTC plumbing.
///
/// All methods take `&self` and return the same generic result type
/// so the agent's hub-completion layer does not need a per-method
/// branch when serialising the outcome.
#[async_trait]
pub trait DesktopTransportProvider: Send + Sync {
    /// Service a `RemoteControl(sessionId, accessKey, …)` invocation.
    async fn remote_control(&self, request: &RemoteControlSessionRequest)
        -> DesktopTransportResult;

    /// Service a `RestartScreenCaster(viewerIds, sessionId, …)`
    /// invocation.
    async fn restart_screen_caster(
        &self,
        request: &RestartScreenCasterRequest,
    ) -> DesktopTransportResult;

    /// Service a `ChangeWindowsSession(viewerConnectionId, sessionId,
    /// …, targetSessionId)` invocation.
    async fn change_windows_session(
        &self,
        request: &ChangeWindowsSessionRequest,
    ) -> DesktopTransportResult;

    /// Service an `InvokeCtrlAltDel()` invocation. The session id is
    /// not part of the request shape; implementations should return
    /// an empty session id in the result.
    async fn invoke_ctrl_alt_del(
        &self,
        request: &InvokeCtrlAltDelRequest,
    ) -> DesktopTransportResult;
}

/// Default provider returned by the runtime when no concrete
/// WebRTC-backed driver is registered. Always reports a structured
/// failure with `success = false` and an operator-facing message
/// naming the host OS — never panics.
///
/// Mirrors [`super::packages::NotSupportedPackageProvider`] so the
/// agent's dispatch path stays uniform across the package and
/// desktop-transport surfaces.
pub struct NotSupportedDesktopTransport {
    host_os: HostOs,
}

impl NotSupportedDesktopTransport {
    /// Build a provider that names `host_os` in its error message.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }

    /// Build a provider that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }

    fn message(&self, method: &str) -> String {
        format!(
            "Desktop transport for {method:?} is not supported on {:?}.",
            self.host_os
        )
    }
}

impl Default for NotSupportedDesktopTransport {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl DesktopTransportProvider for NotSupportedDesktopTransport {
    async fn remote_control(
        &self,
        request: &RemoteControlSessionRequest,
    ) -> DesktopTransportResult {
        DesktopTransportResult::failed(request.session_id.clone(), self.message("RemoteControl"))
    }

    async fn restart_screen_caster(
        &self,
        request: &RestartScreenCasterRequest,
    ) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            request.session_id.clone(),
            self.message("RestartScreenCaster"),
        )
    }

    async fn change_windows_session(
        &self,
        request: &ChangeWindowsSessionRequest,
    ) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            request.session_id.clone(),
            self.message("ChangeWindowsSession"),
        )
    }

    async fn invoke_ctrl_alt_del(
        &self,
        _request: &InvokeCtrlAltDelRequest,
    ) -> DesktopTransportResult {
        // No session id in the request — surface an empty one so the
        // server can correlate against the original invocation by id
        // rather than session.
        DesktopTransportResult::failed(String::new(), self.message("InvokeCtrlAltDel"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc_req() -> RemoteControlSessionRequest {
        RemoteControlSessionRequest {
            session_id: "session-1".into(),
            access_key: "secret".into(),
            user_connection_id: "viewer-1".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: "org-uuid".into(),
        }
    }

    #[tokio::test]
    async fn not_supported_remote_control_is_structured_failure_naming_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux);
        let r = p.remote_control(&rc_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, "session-1");
        let msg = r.error_message.as_deref().unwrap();
        assert!(msg.contains("RemoteControl"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn not_supported_remote_control_does_not_leak_access_key() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux);
        let r = p.remote_control(&rc_req()).await;
        let msg = r.error_message.unwrap();
        assert!(
            !msg.contains("secret"),
            "error message must not leak access_key: {msg}",
        );
    }

    #[tokio::test]
    async fn not_supported_restart_screen_caster_is_structured_failure() {
        let p = NotSupportedDesktopTransport::new(HostOs::MacOs);
        let req = RestartScreenCasterRequest {
            viewer_ids: vec!["v1".into()],
            session_id: "s".into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: "i".into(),
        };
        let r = p.restart_screen_caster(&req).await;
        assert!(!r.success);
        assert_eq!(r.session_id, "s");
        assert!(r.error_message.unwrap().contains("RestartScreenCaster"));
    }

    #[tokio::test]
    async fn not_supported_change_windows_session_names_method_and_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::OtherUnix);
        let req = ChangeWindowsSessionRequest {
            viewer_connection_id: "v".into(),
            session_id: "s".into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: "i".into(),
            target_session_id: 1,
        };
        let r = p.change_windows_session(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ChangeWindowsSession"), "{msg}");
        assert!(msg.contains("OtherUnix"), "{msg}");
    }

    #[tokio::test]
    async fn not_supported_invoke_ctrl_alt_del_returns_empty_session_id() {
        let p = NotSupportedDesktopTransport::new(HostOs::Windows);
        let r = p.invoke_ctrl_alt_del(&InvokeCtrlAltDelRequest).await;
        assert!(!r.success);
        // No session id in the request type → empty in the result.
        assert!(r.session_id.is_empty());
        assert!(r.error_message.unwrap().contains("InvokeCtrlAltDel"));
    }

    #[tokio::test]
    async fn for_current_host_uses_compile_time_os() {
        let p = NotSupportedDesktopTransport::for_current_host();
        let r = p.remote_control(&rc_req()).await;
        let msg = r.error_message.unwrap();
        // The message must name the current host's OS — proves the
        // helper actually picks up the compile-time target.
        assert!(msg.contains(&format!("{:?}", HostOs::current())));
    }

    #[tokio::test]
    async fn default_impl_is_for_current_host() {
        // The dispatcher constructs a `NotSupportedDesktopTransport`
        // via `Default::default()` when no concrete provider is
        // registered. Make sure that path produces a structured
        // failure too.
        let p: NotSupportedDesktopTransport = Default::default();
        let r = p.remote_control(&rc_req()).await;
        assert!(!r.success);
    }

    /// Trait object safety check — `Box<dyn DesktopTransportProvider>`
    /// must compile so the runtime can store providers behind
    /// `Arc<dyn DesktopTransportProvider>`.
    #[test]
    fn trait_is_object_safe() {
        let _p: Box<dyn DesktopTransportProvider> =
            Box::new(NotSupportedDesktopTransport::for_current_host());
    }
}
