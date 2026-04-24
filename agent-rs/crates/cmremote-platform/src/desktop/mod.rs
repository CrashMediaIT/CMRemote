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
//!    implementation must respect the security contract enforced by
//!    [`guards`].
//! 3. The agent binary's dispatch layer routes
//!    `RemoteControl` / `RestartScreenCaster` / `ChangeWindowsSession`
//!    / `InvokeCtrlAltDel` invocations to the registered provider
//!    rather than the generic `not_implemented` fall-through.
//! 4. The default [`NotSupportedDesktopTransport`] runs every request
//!    through [`guards`] first, then returns a structured failure
//!    naming the host OS — never panics. Concrete WebRTC-backed
//!    providers plug in alongside it without further wire/contract
//!    churn, and reuse the same guard helpers so the security
//!    contract is enforced uniformly across the dispatch surface.
//!
//! ## Security contract
//!
//! Every implementation MUST run a request through the matching
//! [`guards::check_remote_control`] / [`guards::check_restart_screen_caster`]
//! / [`guards::check_change_windows_session`] helper *before* reading
//! any other field — in particular before reading the sensitive
//! `access_key`. The guards refuse:
//!
//! 1. **Cross-org sessions** — request `org_id` not matching the
//!    agent's own [`cmremote_wire::ConnectionInfo::organization_id`].
//! 2. **Hostile operator strings** — display name, organisation
//!    name, viewer / user connection ids that exceed
//!    [`guards::MAX_OPERATOR_STRING_LEN`] bytes, contain non-printable
//!    code points (controls, embedded NUL, DEL), or contain Unicode
//!    bidi-override characters (the "Trojan Source" attack vector).
//! 3. **Non-canonical session ids** — anything other than a lowercase
//!    `8-4-4-4-12` UUID, which is the only shape the .NET hub ever
//!    emits.
//!
//! Implementations MUST ALSO **never echo `access_key` into logs or
//! error strings** and MUST **re-resolve every executable from the OS**
//! (the agent never execs a command string carried on the wire; the
//! provider locates the screencaster binary by a fixed local path).

use async_trait::async_trait;
use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, InvokeCtrlAltDelRequest,
    RemoteControlSessionRequest, RestartScreenCasterRequest,
};

use crate::HostOs;

pub mod guards;
pub mod media;

pub use media::{
    CapturedFrame, DesktopCapturer, DesktopMediaError, EncodedVideoChunk,
    NotSupportedDesktopCapturer, NotSupportedVideoEncoder, VideoEncoder,
};

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
/// WebRTC-backed driver is registered. Runs every request through
/// [`guards`] (so cross-org / hostile-string / non-UUID-session
/// requests are refused with a precise message) and otherwise
/// reports a structured "not supported on `<host_os>`" failure.
/// Always sets `success = false`; never panics; never echoes the
/// access key.
///
/// Mirrors [`super::packages::NotSupportedPackageProvider`] so the
/// agent's dispatch path stays uniform across the package and
/// desktop-transport surfaces.
pub struct NotSupportedDesktopTransport {
    host_os: HostOs,
    /// Agent's own organisation id, plumbed in from
    /// [`cmremote_wire::ConnectionInfo::organization_id`]. When
    /// `None` (only possible during early bootstrapping before the
    /// agent has registered with the server) the cross-org guard is
    /// skipped — the format checks still run.
    expected_org_id: Option<String>,
}

impl NotSupportedDesktopTransport {
    /// Build a provider that names `host_os` in its error message
    /// and runs the cross-org guard against `expected_org_id`.
    pub fn new(host_os: HostOs, expected_org_id: Option<String>) -> Self {
        Self {
            host_os,
            expected_org_id,
        }
    }

    /// Build a provider that names the current host's OS and runs
    /// the cross-org guard against `expected_org_id`.
    pub fn for_current_host(expected_org_id: Option<String>) -> Self {
        Self::new(HostOs::current(), expected_org_id)
    }

    fn message(&self, method: &str) -> String {
        format!(
            "Desktop transport for {method:?} is not supported on {:?}.",
            self.host_os
        )
    }

    fn expected_org(&self) -> Option<&str> {
        self.expected_org_id.as_deref()
    }
}

impl Default for NotSupportedDesktopTransport {
    fn default() -> Self {
        // No expected org id known at `Default::default()` time —
        // this constructor is only used by tests and by the
        // dispatcher's last-resort fallback.
        Self::for_current_host(None)
    }
}

#[async_trait]
impl DesktopTransportProvider for NotSupportedDesktopTransport {
    async fn remote_control(
        &self,
        request: &RemoteControlSessionRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_remote_control(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(request.session_id.clone(), self.message("RemoteControl"))
    }

    async fn restart_screen_caster(
        &self,
        request: &RestartScreenCasterRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_restart_screen_caster(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(
            request.session_id.clone(),
            self.message("RestartScreenCaster"),
        )
    }

    async fn change_windows_session(
        &self,
        request: &ChangeWindowsSessionRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_change_windows_session(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(
            request.session_id.clone(),
            self.message("ChangeWindowsSession"),
        )
    }

    async fn invoke_ctrl_alt_del(
        &self,
        _request: &InvokeCtrlAltDelRequest,
    ) -> DesktopTransportResult {
        // No fields → nothing for the guards module to validate.
        // Surface an empty session id so the server can correlate
        // against the original invocation by id rather than session.
        DesktopTransportResult::failed(String::new(), self.message("InvokeCtrlAltDel"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";
    const VALID_ORG_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    fn rc_req() -> RemoteControlSessionRequest {
        RemoteControlSessionRequest {
            session_id: VALID_SESSION_ID.into(),
            access_key: "secret".into(),
            user_connection_id: "viewer-1".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
        }
    }

    #[tokio::test]
    async fn not_supported_remote_control_is_structured_failure_naming_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.remote_control(&rc_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        let msg = r.error_message.as_deref().unwrap();
        assert!(msg.contains("RemoteControl"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn not_supported_remote_control_does_not_leak_access_key() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.remote_control(&rc_req()).await;
        let msg = r.error_message.unwrap();
        assert!(
            !msg.contains("secret"),
            "error message must not leak access_key: {msg}",
        );
    }

    #[tokio::test]
    async fn not_supported_restart_screen_caster_is_structured_failure() {
        let p = NotSupportedDesktopTransport::new(HostOs::MacOs, Some(VALID_ORG_ID.into()));
        let req = RestartScreenCasterRequest {
            viewer_ids: vec!["v1".into()],
            session_id: VALID_SESSION_ID.into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: VALID_ORG_ID.into(),
        };
        let r = p.restart_screen_caster(&req).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        assert!(r.error_message.unwrap().contains("RestartScreenCaster"));
    }

    #[tokio::test]
    async fn not_supported_change_windows_session_names_method_and_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::OtherUnix, Some(VALID_ORG_ID.into()));
        let req = ChangeWindowsSessionRequest {
            viewer_connection_id: "v".into(),
            session_id: VALID_SESSION_ID.into(),
            access_key: "k".into(),
            user_connection_id: "u".into(),
            requester_name: "r".into(),
            org_name: "o".into(),
            org_id: VALID_ORG_ID.into(),
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
        let p = NotSupportedDesktopTransport::new(HostOs::Windows, Some(VALID_ORG_ID.into()));
        let r = p.invoke_ctrl_alt_del(&InvokeCtrlAltDelRequest).await;
        assert!(!r.success);
        // No session id in the request type → empty in the result.
        assert!(r.session_id.is_empty());
        assert!(r.error_message.unwrap().contains("InvokeCtrlAltDel"));
    }

    #[tokio::test]
    async fn for_current_host_uses_compile_time_os() {
        let p = NotSupportedDesktopTransport::for_current_host(Some(VALID_ORG_ID.into()));
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
        // failure too. `Default` carries no expected org id, so the
        // cross-org guard is skipped — the format checks still run.
        let p: NotSupportedDesktopTransport = Default::default();
        let r = p.remote_control(&rc_req()).await;
        assert!(!r.success);
    }

    // -----------------------------------------------------------------
    // Slice R7.b — guards run *before* the OS-not-supported branch, so
    // a hostile request never reaches the per-OS error path. These
    // tests pin that ordering.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn cross_org_remote_control_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = rc_req();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        // Cross-org refusal naming the field, NOT the OS-not-supported
        // message — proves the guard runs first.
        assert!(msg.contains("organisation"), "{msg}");
        assert!(!msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn malformed_session_id_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = rc_req();
        req.session_id = "not-a-uuid".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        // Session id is empty in the failure result so a malformed
        // value cannot be reflected back through the audit log.
        assert!(r.session_id.is_empty());
        assert!(r.error_message.unwrap().contains("session_id"));
    }

    #[tokio::test]
    async fn control_character_in_requester_name_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = rc_req();
        req.requester_name = "Alice\u{1B}[31m".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("requester_name"), "{msg}");
        // The OS-not-supported branch must NOT have run — confirm by
        // absence of the "RemoteControl" method tag.
        assert!(!msg.contains("RemoteControl"), "{msg}");
    }

    /// Trait object safety check — `Box<dyn DesktopTransportProvider>`
    /// must compile so the runtime can store providers behind
    /// `Arc<dyn DesktopTransportProvider>`.
    #[test]
    fn trait_is_object_safe() {
        let _p: Box<dyn DesktopTransportProvider> =
            Box::new(NotSupportedDesktopTransport::for_current_host(None));
    }
}
