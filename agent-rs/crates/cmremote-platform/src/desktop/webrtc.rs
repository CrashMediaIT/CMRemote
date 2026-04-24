// Source: CMRemote, clean-room implementation.

//! Concrete `DesktopTransportProvider` implementation backed by a
//! [`super::session::DesktopSessionRegistry`] (slice R7.k).
//!
//! ## Status
//!
//! This module ships the **lifecycle scaffolding** the eventual WebRTC
//! driver will plug into:
//!
//! - per-session state machine (via [`super::session`]),
//! - guard-first ordering on every `DesktopTransportProvider` method
//!   (R7.b cross-org / operator-string / canonical-UUID checks for the
//!   four method-surface methods, R7.g envelope checks for the three
//!   signalling methods, R7.j envelope+config checks for
//!   `ProvideIceServers`),
//! - replace-on-duplicate semantics for `RemoteControl`,
//! - audit-log events at every state transition,
//!
//! but **stubs the actual peer-connection construction** — every
//! signalling-and-onward hook returns
//! [`DesktopTransportResult::failed`] with the message `"WebRTC driver
//! pending — peer-connection construction stub"` once the guards have
//! passed and the registry has been updated.
//!
//! ## Why a feature flag (`webrtc-driver`)
//!
//! The `webrtc` crate itself is gated on the supply-chain audit
//! described in [`docs/decisions/0001-webrtc-crypto-provider.md`].
//! Until that audit (slice R7.l) lands the audit-and-fork list for
//! every `webrtc-rs` sub-crate, adding `webrtc = "0.x"` to the
//! workspace would trip `cargo deny`'s `ring` ban via transitive
//! sub-crates the spike never audited (`webrtc-ice`, `webrtc-mdns`,
//! `webrtc-interceptor`, `webrtc-data`, `webrtc-media`).
//!
//! Hiding this driver behind a default-off cargo feature lets the
//! lifecycle scaffolding land — and be reviewed and tested — without
//! touching `deny.toml`. Once R7.l ships and the per-fork wiring PRs
//! merge, R7.m flips this same code path from "stub failure" to
//! "construct an `RTCPeerConnection`" without touching the trait
//! surface or the dispatcher.
//!
//! ## Wiring
//!
//! When the workspace is built with `--features webrtc-driver`, the
//! agent runtime constructs a [`WebRtcDesktopTransport`] in place of
//! [`super::NotSupportedDesktopTransport`]. The dispatcher's
//! `Arc<dyn DesktopTransportProvider>` slot is unchanged.

use std::sync::Arc;

use async_trait::async_trait;
use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, IceCandidate, InvokeCtrlAltDelRequest,
    ProvideIceServersRequest, RemoteControlSessionRequest, RestartScreenCasterRequest, SdpAnswer,
    SdpOffer,
};
use tokio::sync::Mutex;

use super::guards;
use super::session::{CloseReason, DesktopSessionRegistry, DesktopSessionState};
use super::DesktopTransportProvider;
use crate::HostOs;

/// Stable string baked into every "driver pending" failure result.
/// Pinned by tests so the .NET hub-side error renderer (and
/// downstream `tracing` aggregations) can match on it without
/// pattern-tracking a moving message.
pub const DRIVER_PENDING_MESSAGE: &str =
    "WebRTC driver pending — peer-connection construction stub";

/// Concrete `DesktopTransportProvider` for the WebRTC capture / encode
/// driver. See the module docs for the lifecycle vs. peer-connection
/// split.
pub struct WebRtcDesktopTransport {
    host_os: HostOs,
    expected_org_id: Option<String>,
    sessions: Arc<Mutex<DesktopSessionRegistry>>,
}

impl WebRtcDesktopTransport {
    /// Build a new driver naming `host_os` and using `expected_org_id`
    /// for the cross-org guard (`None` skips the cross-org check, same
    /// semantics as [`super::NotSupportedDesktopTransport::new`]).
    pub fn new(host_os: HostOs, expected_org_id: Option<String>) -> Self {
        Self {
            host_os,
            expected_org_id,
            sessions: Arc::new(Mutex::new(DesktopSessionRegistry::with_default_timeout())),
        }
    }

    /// Build a driver that names the current host's OS.
    pub fn for_current_host(expected_org_id: Option<String>) -> Self {
        Self::new(HostOs::current(), expected_org_id)
    }

    /// Borrow the underlying session registry. Exposed for the runtime
    /// idle-timeout sweep task and for tests.
    pub fn sessions(&self) -> Arc<Mutex<DesktopSessionRegistry>> {
        self.sessions.clone()
    }

    fn expected_org(&self) -> Option<&str> {
        self.expected_org_id.as_deref()
    }

    /// Build the canonical "driver pending" failure naming the host OS
    /// — every signalling-or-onward stub returns this once the guards
    /// pass and the registry has been updated.
    fn driver_pending(&self, session_id: String) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            session_id,
            format!("{DRIVER_PENDING_MESSAGE} (host_os={:?})", self.host_os),
        )
    }
}

#[async_trait]
impl DesktopTransportProvider for WebRtcDesktopTransport {
    // ---------------------------------------------------------------
    // R7 method-surface methods.
    // ---------------------------------------------------------------

    async fn remote_control(
        &self,
        request: &RemoteControlSessionRequest,
    ) -> DesktopTransportResult {
        // Guard FIRST — the sensitive `access_key` is never read until
        // every envelope field passes validation. Same contract the
        // `NotSupportedDesktopTransport` pins.
        if let Err(rejection) = guards::check_remote_control(request, self.expected_org()) {
            return rejection.into_result();
        }
        // Guards passed → register (or replace) the session BEFORE
        // returning the driver-pending stub, so the audit trail
        // records the open even though the peer connection is not
        // yet wired.
        let mut sessions = self.sessions.lock().await;
        let _outcome = sessions.open(&request.session_id, &request.user_connection_id);
        // Until the real driver lands, the open transition itself is
        // the only state change — the session sits in `Initializing`
        // and waits for `ProvideIceServers` / `SendSdpOffer`.
        drop(sessions);
        self.driver_pending(request.session_id.clone())
    }

    async fn restart_screen_caster(
        &self,
        request: &RestartScreenCasterRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_restart_screen_caster(request, self.expected_org()) {
            return rejection.into_result();
        }
        // RestartScreenCaster is a driver-internal kick — it does not
        // open or close a session by itself; the existing session (if
        // any) keeps its state. Surface the driver-pending message so
        // the dispatcher's audit trail still records the call.
        self.driver_pending(request.session_id.clone())
    }

    async fn change_windows_session(
        &self,
        request: &ChangeWindowsSessionRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_change_windows_session(request, self.expected_org()) {
            return rejection.into_result();
        }
        // ChangeWindowsSession on Windows tears down the existing
        // capture pipeline and rebuilds it in the target session.
        // Until the real driver lands, we close the registry's record
        // for the session id (audit-logged) so a subsequent
        // SendSdpOffer goes through the "session not initialised"
        // path rather than racing against a stale state.
        let mut sessions = self.sessions.lock().await;
        sessions.close(&request.session_id, CloseReason::Explicit);
        drop(sessions);
        self.driver_pending(request.session_id.clone())
    }

    async fn invoke_ctrl_alt_del(
        &self,
        _request: &InvokeCtrlAltDelRequest,
    ) -> DesktopTransportResult {
        // No envelope to guard — the request type is empty.
        // No session id either; keep the same "empty session id in
        // the failure result" shape `NotSupportedDesktopTransport` uses
        // so the dispatcher's `arguments` decoder treats either
        // provider identically.
        self.driver_pending(String::new())
    }

    // ---------------------------------------------------------------
    // R7.g signalling methods. State transitions documented inline.
    // ---------------------------------------------------------------

    async fn on_sdp_offer(&self, request: &SdpOffer) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_sdp_offer(request, self.expected_org()) {
            return rejection.into_result();
        }
        let mut sessions = self.sessions.lock().await;
        if sessions.get(&request.session_id).is_none() {
            // No `RemoteControl` for this id — refuse with a precise
            // message so the audit log captures the ordering bug.
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "SendSdpOffer received before RemoteControl opened the session".to_string(),
            );
        }
        // Both Initializing→NegotiatingSdp and IceConfigured→
        // NegotiatingSdp are valid (the .NET hub may skip
        // ProvideIceServers when the viewer reuses defaults). Drive
        // the registry to NegotiatingSdp; an idempotent retry
        // (re-offer in the same state) is silently accepted.
        let _ = sessions.transition(
            &request.session_id,
            DesktopSessionState::NegotiatingSdp,
            "send-sdp-offer",
        );
        drop(sessions);
        self.driver_pending(request.session_id.clone())
    }

    async fn on_sdp_answer(&self, request: &SdpAnswer) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_sdp_answer(request, self.expected_org()) {
            return rejection.into_result();
        }
        let sessions = self.sessions.lock().await;
        if sessions.get(&request.session_id).is_none() {
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "SendSdpAnswer received before RemoteControl opened the session".to_string(),
            );
        }
        // SDP answer does not change registry state — the session
        // stays in NegotiatingSdp until the driver promotes it to
        // Connected. Audit logging still happens via `transition`'s
        // idempotent path the moment the driver lands.
        drop(sessions);
        self.driver_pending(request.session_id.clone())
    }

    async fn on_ice_candidate(&self, request: &IceCandidate) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_ice_candidate(request, self.expected_org()) {
            return rejection.into_result();
        }
        let sessions = self.sessions.lock().await;
        if sessions.get(&request.session_id).is_none() {
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "SendIceCandidate received before RemoteControl opened the session".to_string(),
            );
        }
        // ICE trickle does not change registry state; it feeds the
        // peer connection's ICE agent directly. No transition emitted.
        drop(sessions);
        self.driver_pending(request.session_id.clone())
    }

    // ---------------------------------------------------------------
    // R7.j ProvideIceServers method.
    // ---------------------------------------------------------------

    async fn on_provide_ice_servers(
        &self,
        request: &ProvideIceServersRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_provide_ice_servers(request, self.expected_org()) {
            return rejection.into_result();
        }
        let mut sessions = self.sessions.lock().await;
        if sessions.get(&request.session_id).is_none() {
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "ProvideIceServers received before RemoteControl opened the session".to_string(),
            );
        }
        let _ = sessions.transition(
            &request.session_id,
            DesktopSessionState::IceConfigured,
            "provide-ice-servers",
        );
        drop(sessions);
        self.driver_pending(request.session_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";
    const OTHER_SESSION_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const VALID_ORG_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    fn rc_req() -> RemoteControlSessionRequest {
        RemoteControlSessionRequest {
            session_id: VALID_SESSION_ID.to_string(),
            access_key: "secret-access-key".to_string(),
            user_connection_id: "viewer-1".to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
        }
    }

    fn restart_req() -> RestartScreenCasterRequest {
        RestartScreenCasterRequest {
            viewer_ids: vec!["viewer-1".into()],
            session_id: VALID_SESSION_ID.to_string(),
            access_key: "secret-access-key".to_string(),
            user_connection_id: "viewer-1".to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
        }
    }

    fn change_session_req() -> ChangeWindowsSessionRequest {
        ChangeWindowsSessionRequest {
            viewer_connection_id: "viewer-1".to_string(),
            session_id: VALID_SESSION_ID.to_string(),
            access_key: "secret-access-key".to_string(),
            user_connection_id: "viewer-1".to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
            target_session_id: 1,
        }
    }

    fn sdp_offer_req() -> SdpOffer {
        SdpOffer {
            viewer_connection_id: "viewer-1".to_string(),
            session_id: VALID_SESSION_ID.to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
            kind: cmremote_wire::SdpKind::Offer,
            sdp: "v=0\r\n".to_string(),
        }
    }

    fn sdp_answer_req() -> SdpAnswer {
        SdpAnswer {
            viewer_connection_id: "viewer-1".to_string(),
            session_id: VALID_SESSION_ID.to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
            kind: cmremote_wire::SdpKind::Answer,
            sdp: "v=0\r\n".to_string(),
        }
    }

    fn ice_candidate_req() -> IceCandidate {
        IceCandidate {
            viewer_connection_id: "viewer-1".to_string(),
            session_id: VALID_SESSION_ID.to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
            candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host".to_string(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        }
    }

    fn provide_ice_req() -> ProvideIceServersRequest {
        ProvideIceServersRequest {
            viewer_connection_id: "viewer-1".to_string(),
            session_id: VALID_SESSION_ID.to_string(),
            access_key: "secret-access-key".to_string(),
            requester_name: "Alice".to_string(),
            org_name: "Acme".to_string(),
            org_id: VALID_ORG_ID.to_string(),
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

    // -----------------------------------------------------------------
    // Happy path: RemoteControl opens the session; subsequent
    // signalling methods drive the registry into the right state.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn remote_control_opens_session_and_returns_driver_pending() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.remote_control(&rc_req()).await;
        assert!(!r.success);
        assert_eq!(r.session_id, VALID_SESSION_ID);
        let msg = r.error_message.unwrap();
        assert!(msg.contains(DRIVER_PENDING_MESSAGE), "{msg}");
        // Sensitive access_key MUST NOT appear.
        assert!(!msg.contains("secret-access-key"), "{msg}");
        // Registry now has the session in Initializing.
        let sessions = p.sessions().lock_owned().await;
        let s = sessions.get(VALID_SESSION_ID).unwrap();
        assert_eq!(s.state, DesktopSessionState::Initializing);
    }

    #[tokio::test]
    async fn provide_ice_servers_after_remote_control_drives_to_ice_configured() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let r = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains(DRIVER_PENDING_MESSAGE), "{msg}");
        assert!(!msg.contains("secret-access-key"), "{msg}");
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::IceConfigured,
        );
    }

    #[tokio::test]
    async fn sdp_offer_after_provide_ice_drives_to_negotiating_sdp() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        let _ = p.on_sdp_offer(&sdp_offer_req()).await;
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
    }

    #[tokio::test]
    async fn sdp_offer_without_prior_provide_ice_still_drives_to_negotiating_sdp() {
        // The .NET hub may emit SendSdpOffer without ProvideIceServers
        // when the viewer reuses defaults; the state machine accepts
        // the shortcut.
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_sdp_offer(&sdp_offer_req()).await;
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
    }

    #[tokio::test]
    async fn sdp_answer_does_not_change_registry_state() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_sdp_offer(&sdp_offer_req()).await;
        let _ = p.on_sdp_answer(&sdp_answer_req()).await;
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
    }

    #[tokio::test]
    async fn ice_candidate_does_not_change_registry_state() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_sdp_offer(&sdp_offer_req()).await;
        let _ = p.on_ice_candidate(&ice_candidate_req()).await;
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
    }

    // -----------------------------------------------------------------
    // Replace-on-duplicate.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn duplicate_remote_control_replaces_prior_session() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        // Re-issue RemoteControl with a different viewer.
        let mut req = rc_req();
        req.user_connection_id = "viewer-2".into();
        let _ = p.remote_control(&req).await;
        let sessions = p.sessions().lock_owned().await;
        let s = sessions.get(VALID_SESSION_ID).unwrap();
        // Session is freshly Initializing, viewer is the new id.
        assert_eq!(s.state, DesktopSessionState::Initializing);
        assert_eq!(s.viewer_connection_id, "viewer-2");
    }

    // -----------------------------------------------------------------
    // Out-of-order signalling: every onward hook MUST refuse when the
    // session does not exist (it was never opened or was closed).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn provide_ice_without_remote_control_refuses_with_precise_message() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ProvideIceServers"), "{msg}");
        assert!(msg.contains("RemoteControl"), "{msg}");
        assert!(!msg.contains("secret-access-key"), "{msg}");
        // Registry has not been touched.
        assert!(p.sessions().lock_owned().await.is_empty());
    }

    #[tokio::test]
    async fn sdp_offer_without_remote_control_refuses_with_precise_message() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_sdp_offer(&sdp_offer_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SendSdpOffer"));
    }

    #[tokio::test]
    async fn sdp_answer_without_remote_control_refuses_with_precise_message() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_sdp_answer(&sdp_answer_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SendSdpAnswer"));
    }

    #[tokio::test]
    async fn ice_candidate_without_remote_control_refuses_with_precise_message() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.on_ice_candidate(&ice_candidate_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SendIceCandidate"));
    }

    // -----------------------------------------------------------------
    // Guards run BEFORE any registry mutation. Pin the ordering by
    // confirming a hostile request leaves the registry empty.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn cross_org_remote_control_is_refused_before_registry_mutation() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = rc_req();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("organisation"));
        assert!(p.sessions().lock_owned().await.is_empty());
    }

    #[tokio::test]
    async fn malformed_session_id_refused_without_touching_registry() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let mut req = rc_req();
        req.session_id = "not-a-uuid".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        assert!(p.sessions().lock_owned().await.is_empty());
    }

    #[tokio::test]
    async fn over_length_sdp_refused_before_registry_mutation() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let mut req = sdp_offer_req();
        req.sdp = "v".repeat(cmremote_wire::MAX_SDP_BYTES + 1);
        let r = p.on_sdp_offer(&req).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("sdp"));
        // Session should still be Initializing — the over-length
        // offer never reached the state machine.
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::Initializing,
        );
    }

    #[tokio::test]
    async fn hostile_ice_url_refused_before_registry_mutation() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let mut req = provide_ice_req();
        req.ice_server_config.ice_servers[0].urls[0] = "javascript:alert(1)".into();
        let r = p.on_provide_ice_servers(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ice_servers[0]"), "{msg}");
        // The hostile URL bytes MUST NOT appear in the rejection.
        assert!(!msg.contains("javascript"), "{msg}");
        // State unchanged.
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::Initializing,
        );
    }

    // -----------------------------------------------------------------
    // Cross-session isolation.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn signalling_for_session_a_does_not_change_session_b_state() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let mut req_b = rc_req();
        req_b.session_id = OTHER_SESSION_ID.into();
        let _ = p.remote_control(&req_b).await;
        // Drive A to NegotiatingSdp.
        let _ = p.on_sdp_offer(&sdp_offer_req()).await;
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
        assert_eq!(
            sessions.get(OTHER_SESSION_ID).unwrap().state,
            DesktopSessionState::Initializing,
        );
    }

    // -----------------------------------------------------------------
    // ChangeWindowsSession closes the registry entry (the driver will
    // rebuild capture in the new session).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn change_windows_session_closes_registry_entry_for_session() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        let r = p.change_windows_session(&change_session_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains(DRIVER_PENDING_MESSAGE));
        // Entry is gone — a future SendSdpOffer will fail with the
        // "before RemoteControl" message.
        assert!(p
            .sessions()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .is_none());
    }

    #[tokio::test]
    async fn restart_screen_caster_does_not_remove_session() {
        // RestartScreenCaster is a driver-internal kick; the session
        // entry must survive so the existing signalling can continue.
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let _ = p.remote_control(&rc_req()).await;
        let r = p.restart_screen_caster(&restart_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains(DRIVER_PENDING_MESSAGE));
        assert!(p
            .sessions()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .is_some());
    }

    // -----------------------------------------------------------------
    // CtrlAltDel is a stateless ping; no session id, no registry change.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn invoke_ctrl_alt_del_returns_empty_session_id_and_does_not_open_a_session() {
        let p = WebRtcDesktopTransport::new(HostOs::Linux, Some(VALID_ORG_ID.into()));
        let r = p.invoke_ctrl_alt_del(&InvokeCtrlAltDelRequest).await;
        assert!(!r.success);
        assert!(r.session_id.is_empty());
        assert!(r.error_message.unwrap().contains(DRIVER_PENDING_MESSAGE));
        assert!(p.sessions().lock_owned().await.is_empty());
    }

    // -----------------------------------------------------------------
    // Trait object safety — the dispatcher stores the provider behind
    // `Arc<dyn DesktopTransportProvider>`.
    // -----------------------------------------------------------------

    #[test]
    fn webrtc_transport_is_object_safe() {
        let _p: Arc<dyn DesktopTransportProvider> = Arc::new(
            WebRtcDesktopTransport::for_current_host(Some(VALID_ORG_ID.into())),
        );
    }
}
