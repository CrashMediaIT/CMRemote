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
    ChangeWindowsSessionRequest, DesktopTransportResult, IceCandidate, InvokeCtrlAltDelRequest,
    ProvideIceServersRequest, RemoteControlSessionRequest, RestartScreenCasterRequest, SdpAnswer,
    SdpOffer,
};

use crate::HostOs;

pub mod consent;
pub mod encoder_sink;
pub mod guards;
pub mod input;
pub mod media;
pub mod nv12;
pub mod providers;
pub mod pump;
pub mod session;
pub mod signalling_egress;
#[cfg(feature = "webrtc-driver")]
pub mod webrtc;
#[cfg(feature = "webrtc-driver")]
pub(crate) mod webrtc_pc;

pub use consent::{
    ConsentDecision, ConsentPrompter, ConsentRequest, DenyAllConsentPrompter,
    DEFAULT_CONSENT_TIMEOUT,
};
pub use encoder_sink::{DiscardingEncodedChunkSink, EncodedChunkSink, EncoderCaptureSink};
pub use input::{
    Clipboard, DesktopInputError, KeyCode, KeyboardInput, MouseButton, MouseInput, NamedKey,
    NotSupportedClipboard, NotSupportedKeyboardInput, NotSupportedMouseInput, ScrollAxis,
};
pub use media::{
    CapturedFrame, DesktopCapturer, DesktopMediaError, EncodedVideoChunk,
    NotSupportedDesktopCapturer, NotSupportedVideoEncoder, VideoEncoder,
};
pub use nv12::{bgra_to_nv12, Nv12Frame};
pub use providers::DesktopProviders;
pub use pump::{
    CapturePump, CapturePumpConfig, CaptureSink, CaptureStats, CaptureStatsSnapshot,
    DiscardingCaptureSink,
};
pub use session::{
    CloseReason, DesktopSession, DesktopSessionRegistry, DesktopSessionState, OpenOutcome,
    TransitionOutcome, DEFAULT_IDLE_TIMEOUT,
};
pub use signalling_egress::{LoggingSignallingEgress, SignallingEgress};
#[cfg(feature = "webrtc-driver")]
pub use webrtc::WebRtcDesktopTransport;

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

    // ---------------------------------------------------------------
    // Slice R7.g — signalling hooks.
    //
    // The `Send*` family of methods is the WebRTC negotiation
    // surface every concrete driver plugs into. Until a driver lands
    // (gated on the crypto-provider ADR), the default
    // `NotSupportedDesktopTransport` runs the same security guards
    // these hooks contractually require, then returns a structured
    // "not supported on <host_os>" failure. The dispatch layer
    // routes `SendSdpOffer` / `SendSdpAnswer` / `SendIceCandidate`
    // hub invocations here.
    //
    // All three methods MUST run the matching `guards::check_*_*`
    // helper *before* parsing the SDP / candidate body — same
    // contract as the four method-surface methods above. The body
    // length cap (`MAX_SDP_BYTES` for SDP, `MAX_SIGNALLING_STRING_LEN`
    // for ICE lines) is enforced inside the guard helper, so an
    // over-length payload never reaches the driver.
    // ---------------------------------------------------------------

    /// Service a `SendSdpOffer(viewerConnectionId, sessionId, …, sdp)`
    /// invocation — the viewer is opening (or re-opening) the WebRTC
    /// negotiation. Default-impl provided so existing driver crates
    /// (and downstream forks) compile without a churn step; concrete
    /// drivers MUST override it.
    async fn on_sdp_offer(&self, request: &SdpOffer) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            request.session_id.clone(),
            "Desktop transport for \"SendSdpOffer\" is not implemented by this provider."
                .to_string(),
        )
    }

    /// Service a `SendSdpAnswer(…)` invocation — the viewer is
    /// accepting an agent-initiated renegotiation.
    async fn on_sdp_answer(&self, request: &SdpAnswer) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            request.session_id.clone(),
            "Desktop transport for \"SendSdpAnswer\" is not implemented by this provider."
                .to_string(),
        )
    }

    /// Service a `SendIceCandidate(…)` invocation — a trickled ICE
    /// candidate from the viewer.
    async fn on_ice_candidate(&self, request: &IceCandidate) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            request.session_id.clone(),
            "Desktop transport for \"SendIceCandidate\" is not implemented by this provider."
                .to_string(),
        )
    }

    // ---------------------------------------------------------------
    // Slice R7.j — `ProvideIceServers` hook.
    //
    // Receives the per-session ICE / TURN configuration the .NET hub
    // delivers before the viewer starts trickling candidates. The
    // future WebRTC driver consumes the embedded `IceServerConfig`
    // as its `RTCConfiguration::ice_servers` /
    // `ice_transport_policy`. Until a driver lands, the default
    // `NotSupportedDesktopTransport` runs the slice R7.b envelope
    // guards plus the slice R7.i config guards before returning a
    // structured "not supported on <host_os>" failure.
    //
    // Implementations MUST run `guards::check_provide_ice_servers`
    // *before* reading the embedded config — same guard-first
    // contract as the four method-surface methods above.
    // ---------------------------------------------------------------

    /// Service a `ProvideIceServers(iceServerConfig, sessionId, …)`
    /// invocation. Default-impl provided so existing driver crates
    /// (and downstream forks) compile without a churn step;
    /// concrete drivers MUST override it.
    async fn on_provide_ice_servers(
        &self,
        request: &ProvideIceServersRequest,
    ) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            request.session_id.clone(),
            "Desktop transport for \"ProvideIceServers\" is not implemented by this provider."
                .to_string(),
        )
    }
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

    // -----------------------------------------------------------------
    // Slice R7.g — signalling hooks. Same guard-first ordering as the
    // four method-surface methods: a hostile request never reaches
    // the per-OS error path. Concrete WebRTC drivers will override
    // these methods with the actual peer-connection plumbing.
    // -----------------------------------------------------------------

    async fn on_sdp_offer(&self, request: &SdpOffer) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_sdp_offer(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(request.session_id.clone(), self.message("SendSdpOffer"))
    }

    async fn on_sdp_answer(&self, request: &SdpAnswer) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_sdp_answer(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(request.session_id.clone(), self.message("SendSdpAnswer"))
    }

    async fn on_ice_candidate(&self, request: &IceCandidate) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_ice_candidate(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(request.session_id.clone(), self.message("SendIceCandidate"))
    }

    // -----------------------------------------------------------------
    // Slice R7.j — `ProvideIceServers` hook stub. Same guard-first
    // ordering as every other method here: a hostile request never
    // reaches the per-OS error path, the sensitive `access_key` is
    // never read, and the embedded `IceServerConfig` is validated by
    // the same per-server checks `check_ice_server_config` applies.
    // -----------------------------------------------------------------

    async fn on_provide_ice_servers(
        &self,
        request: &ProvideIceServersRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_provide_ice_servers(request, self.expected_org()) {
            return rejection.into_result();
        }
        DesktopTransportResult::failed(
            request.session_id.clone(),
            self.message("ProvideIceServers"),
        )
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

    // -----------------------------------------------------------------
    // Slice R7.g — signalling-hook stub behaviour. Mirror the
    // method-surface tests above: guards run *before* the
    // OS-not-supported branch.
    // -----------------------------------------------------------------

    fn sdp_offer_req() -> SdpOffer {
        SdpOffer {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            kind: cmremote_wire::SdpKind::Offer,
            sdp: "v=0\r\n".into(),
        }
    }

    fn sdp_answer_req() -> SdpAnswer {
        SdpAnswer {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            kind: cmremote_wire::SdpKind::Answer,
            sdp: "v=0\r\n".into(),
        }
    }

    fn ice_req() -> IceCandidate {
        IceCandidate {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        }
    }

    #[tokio::test]
    async fn not_supported_sdp_offer_is_structured_failure_naming_method_and_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_sdp_offer(&sdp_offer_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("SendSdpOffer"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn not_supported_sdp_answer_is_structured_failure_naming_method_and_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::MacOs, Some(VALID_ORG_ID.into()));
        let r = p.on_sdp_answer(&sdp_answer_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("SendSdpAnswer"), "{msg}");
        assert!(msg.contains("MacOs"), "{msg}");
    }

    #[tokio::test]
    async fn not_supported_ice_candidate_is_structured_failure_naming_method_and_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::Windows, Some(VALID_ORG_ID.into()));
        let r = p.on_ice_candidate(&ice_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("SendIceCandidate"), "{msg}");
        assert!(msg.contains("Windows"), "{msg}");
    }

    #[tokio::test]
    async fn cross_org_sdp_offer_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = sdp_offer_req();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = p.on_sdp_offer(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        // Cross-org refusal — proves the guard runs *before* the
        // OS-not-supported branch.
        assert!(msg.contains("organisation"), "{msg}");
        assert!(!msg.contains("Linux"), "{msg}");
        assert!(!msg.contains("SendSdpOffer"), "{msg}");
    }

    #[tokio::test]
    async fn over_length_sdp_in_offer_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = sdp_offer_req();
        req.sdp = "v".repeat(cmremote_wire::MAX_SDP_BYTES + 1);
        let r = p.on_sdp_offer(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("sdp"), "{msg}");
        assert!(msg.contains("limit"), "{msg}");
        // Body must NOT be echoed.
        assert!(!msg.contains(&"v".repeat(64)), "{msg}");
    }

    #[tokio::test]
    async fn end_of_candidates_marker_passes_guards_and_surfaces_os_not_supported() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = ice_req();
        req.candidate = String::new();
        req.sdp_mid = None;
        req.sdp_mline_index = None;
        let r = p.on_ice_candidate(&req).await;
        // Guards accept the marker; the stub then fails closed with
        // the OS-not-supported message — the *expected* shape until
        // a concrete WebRTC driver registers.
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("SendIceCandidate"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
    }

    // -----------------------------------------------------------------
    // Slice R7.j — `ProvideIceServers` stub behaviour. Mirror the
    // signalling-hook tests above: guards run *before* the OS-not-
    // supported branch, the sensitive `access_key` is never echoed
    // into the failure message, and a hostile embedded ICE config
    // is refused at the same gate as a hostile envelope.
    // -----------------------------------------------------------------

    fn provide_ice_servers_req() -> cmremote_wire::ProvideIceServersRequest {
        cmremote_wire::ProvideIceServersRequest {
            viewer_connection_id: "viewer-1".into(),
            session_id: VALID_SESSION_ID.into(),
            access_key: "secret-access-key".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: VALID_ORG_ID.into(),
            ice_server_config: cmremote_wire::IceServerConfig {
                ice_servers: vec![cmremote_wire::IceServer {
                    urls: vec!["stun:stun.example.org:3478".into()],
                    username: None,
                    credential: None,
                    credential_type: cmremote_wire::IceCredentialType::Password,
                }],
                ice_transport_policy: cmremote_wire::IceTransportPolicy::All,
            },
        }
    }

    #[tokio::test]
    async fn not_supported_provide_ice_servers_is_structured_failure_naming_method_and_os() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_provide_ice_servers(&provide_ice_servers_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ProvideIceServers"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn not_supported_provide_ice_servers_does_not_leak_access_key() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_provide_ice_servers(&provide_ice_servers_req()).await;
        let msg = r.error_message.unwrap();
        assert!(
            !msg.contains("secret-access-key"),
            "access_key leaked into result: {msg}",
        );
    }

    #[tokio::test]
    async fn cross_org_provide_ice_servers_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = provide_ice_servers_req();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = p.on_provide_ice_servers(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("organisation"), "{msg}");
        // OS-not-supported branch did not run — the cross-org guard
        // refused the request first.
        assert!(!msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn hostile_ice_url_in_provide_ice_servers_is_refused_before_os_check() {
        let p = NotSupportedDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = provide_ice_servers_req();
        req.ice_server_config.ice_servers[0].urls[0] = "javascript:alert(1)".into();
        let r = p.on_provide_ice_servers(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ice_servers[0]"), "{msg}");
        assert!(msg.contains("scheme"), "{msg}");
        // The URL contents MUST NOT appear in the rejection message.
        assert!(!msg.contains("javascript"), "{msg}");
    }

    /// The default trait method (i.e. without the `NotSupported*`
    /// override) carries a fixed "not implemented by this provider"
    /// message — pin that so a future driver crate that forgets to
    /// override the hook surfaces a clear error rather than silently
    /// succeeding.
    #[tokio::test]
    async fn default_trait_provide_ice_servers_returns_not_implemented_message() {
        struct Bare;
        #[async_trait]
        impl DesktopTransportProvider for Bare {
            async fn remote_control(
                &self,
                _: &RemoteControlSessionRequest,
            ) -> DesktopTransportResult {
                DesktopTransportResult::failed(String::new(), "n/a".to_string())
            }
            async fn restart_screen_caster(
                &self,
                _: &RestartScreenCasterRequest,
            ) -> DesktopTransportResult {
                DesktopTransportResult::failed(String::new(), "n/a".to_string())
            }
            async fn change_windows_session(
                &self,
                _: &ChangeWindowsSessionRequest,
            ) -> DesktopTransportResult {
                DesktopTransportResult::failed(String::new(), "n/a".to_string())
            }
            async fn invoke_ctrl_alt_del(
                &self,
                _: &InvokeCtrlAltDelRequest,
            ) -> DesktopTransportResult {
                DesktopTransportResult::failed(String::new(), "n/a".to_string())
            }
        }
        let r = Bare
            .on_provide_ice_servers(&provide_ice_servers_req())
            .await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ProvideIceServers"), "{msg}");
        assert!(msg.contains("not implemented"), "{msg}");
    }
}
