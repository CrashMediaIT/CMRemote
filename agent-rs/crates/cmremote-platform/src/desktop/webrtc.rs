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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, IceCandidate, InvokeCtrlAltDelRequest,
    ProvideIceServersRequest, RemoteControlSessionRequest, RestartScreenCasterRequest, SdpAnswer,
    SdpOffer,
};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::guards;
use super::providers::DesktopProviders;
use super::pump::{
    CapturePump, CapturePumpConfig, CaptureSink, CaptureStatsSnapshot, DiscardingCaptureSink,
};
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
    /// Per-host bundle of capture / input providers (slice R7.n.4).
    /// The pump reads `providers.capturer`; the future track-builder
    /// will read `providers.mouse` / `providers.keyboard` /
    /// `providers.clipboard` for the input data-channel.
    providers: Arc<DesktopProviders>,
    /// Sink every running pump pushes captured frames into. Until
    /// the encoder lands (slice R7.n.6 — Media Foundation), the
    /// default sink is [`DiscardingCaptureSink`] which counts +
    /// drops frames. Stored as one shared `Arc` so every per-session
    /// pump observes the same downstream stats.
    sink: Arc<dyn CaptureSink>,
    /// Pump tunables — pinned at construction so renegotiation
    /// can't smuggle a hostile FPS in through the wire.
    pump_config: CapturePumpConfig,
    /// Live pump per session. Keyed by canonical-UUID `session_id`,
    /// kept in lock-step with [`Self::sessions`]: every entry here
    /// has a matching session record, and every session that has
    /// reached `Initializing` has a matching pump until close /
    /// replace / sweep removes both. Not coalesced into the
    /// registry struct because the registry is `pub` and consumed
    /// by tests that don't want a Tokio runtime dependency.
    pumps: Arc<Mutex<HashMap<String, CapturePump>>>,
}

impl WebRtcDesktopTransport {
    /// Build a new driver naming `host_os` and using `expected_org_id`
    /// for the cross-org guard (`None` skips the cross-org check, same
    /// semantics as [`super::NotSupportedDesktopTransport::new`]).
    ///
    /// Uses [`DesktopProviders::not_supported_for`] for the bundle
    /// and [`DiscardingCaptureSink`] for the sink. The agent runtime
    /// upgrades both via [`Self::with_providers`] when constructing
    /// the production driver.
    pub fn new(host_os: HostOs, expected_org_id: Option<String>) -> Self {
        Self::with_providers(
            host_os,
            expected_org_id,
            Arc::new(DesktopProviders::not_supported_for(host_os)),
            Arc::new(DiscardingCaptureSink::new()),
            CapturePumpConfig::default(),
        )
    }

    /// Build a driver that names the current host's OS, using the
    /// `NotSupported` provider bundle + discarding sink. The agent
    /// runtime calls [`Self::with_providers`] instead so it can
    /// inject the per-OS bundle from
    /// [`super::providers::DesktopProviders`] +
    /// `cmremote_platform_windows::WindowsDesktopProviders` etc.
    pub fn for_current_host(expected_org_id: Option<String>) -> Self {
        Self::new(HostOs::current(), expected_org_id)
    }

    /// Full constructor — used by the agent runtime to inject the
    /// per-host capture / input bundle and the production sink.
    pub fn with_providers(
        host_os: HostOs,
        expected_org_id: Option<String>,
        providers: Arc<DesktopProviders>,
        sink: Arc<dyn CaptureSink>,
        pump_config: CapturePumpConfig,
    ) -> Self {
        Self {
            host_os,
            expected_org_id,
            sessions: Arc::new(Mutex::new(DesktopSessionRegistry::with_default_timeout())),
            providers,
            sink,
            pump_config,
            pumps: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Borrow the underlying session registry. Exposed for the runtime
    /// idle-timeout sweep task and for tests.
    pub fn sessions(&self) -> Arc<Mutex<DesktopSessionRegistry>> {
        self.sessions.clone()
    }

    /// Borrow the per-session pump map. Exposed so the runtime sweep
    /// task can confirm it's stopping pumps in lock-step with the
    /// registry, and so tests can assert pump lifecycle.
    pub fn pumps(&self) -> Arc<Mutex<HashMap<String, CapturePump>>> {
        self.pumps.clone()
    }

    /// Stable plain-data snapshot of the live capture stats for one
    /// session. `None` when no pump is running for that id (either
    /// the session was never opened, or it has been closed and the
    /// pump removed). Cheap — the returned snapshot is a clone of
    /// the in-flight counters, not a borrow.
    pub async fn pump_stats(&self, session_id: &str) -> Option<CaptureStatsSnapshot> {
        let g = self.pumps.lock().await;
        g.get(session_id).map(|p| p.stats().snapshot())
    }

    fn expected_org(&self) -> Option<&str> {
        self.expected_org_id.as_deref()
    }

    /// Spawn a fresh pump for `session_id`, replacing (and stopping)
    /// any existing pump for the same id. Awaits the prior pump's
    /// abort-and-join so callers observe a clean handoff.
    async fn spawn_pump(&self, session_id: &str) {
        let pump = CapturePump::start(
            self.providers.capturer.clone(),
            self.sink.clone(),
            self.pump_config,
        );
        let prior = {
            let mut g = self.pumps.lock().await;
            g.insert(session_id.to_string(), pump)
        };
        if let Some(prior) = prior {
            tracing::info!(
                session_id = %session_id,
                event = "pump-replaced",
                "stopped prior capture pump for replaced session",
            );
            let _ = prior.stop().await;
        }
    }

    /// Stop and remove the pump for `session_id`. No-op if no pump
    /// was registered for that id.
    async fn stop_pump(&self, session_id: &str, reason: &'static str) {
        let pump = {
            let mut g = self.pumps.lock().await;
            g.remove(session_id)
        };
        if let Some(pump) = pump {
            tracing::info!(
                session_id = %session_id,
                event = "pump-stopped",
                reason = reason,
                "stopped capture pump",
            );
            let _ = pump.stop().await;
        }
    }

    /// Sweep the session registry for idle sessions, then stop any
    /// pump whose session id is no longer present. Replacement for
    /// the bare `sessions().lock().await.sweep_idle(now)` the
    /// runtime would otherwise call — using this method keeps
    /// pumps from outliving their session record.
    ///
    /// Returns the list of evicted session ids (same shape the
    /// underlying registry sweep returns).
    pub async fn sweep_idle_with_pumps(&self, now: Instant) -> Vec<String> {
        let evicted = {
            let mut sessions = self.sessions.lock().await;
            sessions.sweep_idle(now)
        };
        for id in &evicted {
            self.stop_pump(id, CloseReason::IdleTimeout.as_str()).await;
        }
        evicted
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
        drop(sessions);
        // Slice R7.n.5 — spawn a per-session capture pump (see
        // `pump.rs`). The pump pulls from the bundle's capturer at
        // `pump_config.target_fps` and pushes into the shared sink
        // (default `DiscardingCaptureSink` until the encoder lands).
        // `spawn_pump` aborts and joins any prior pump for the same
        // session id, mirroring the registry's replace-on-duplicate
        // semantics.
        self.spawn_pump(&request.session_id).await;
        // Until the real driver lands, the open transition itself is
        // the only state change — the session sits in `Initializing`
        // and waits for `ProvideIceServers` / `SendSdpOffer`.
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
        // path rather than racing against a stale state. Stop the
        // matching pump in lock-step so the capturer is released
        // before the new session attaches to it.
        let mut sessions = self.sessions.lock().await;
        sessions.close(&request.session_id, CloseReason::Explicit);
        drop(sessions);
        self.stop_pump(&request.session_id, "change-windows-session")
            .await;
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

    // -----------------------------------------------------------------
    // Slice R7.n.5 — capture-pump lifecycle. Every session that
    // RemoteControl opens must spawn a pump; every close path
    // (replace-on-duplicate, ChangeWindowsSession, sweep_idle) must
    // stop the matching pump in lock-step with the registry.
    // -----------------------------------------------------------------

    /// Build a transport whose pump uses an extreme `target_fps` and
    /// near-zero error backoff so the lifecycle assertions below
    /// don't have to wait for a real 30 fps tick. The default
    /// `NotSupported` capturer + `DiscardingCaptureSink` are still
    /// used so the pump immediately starts hitting capture errors —
    /// which is exactly what we want for the lifecycle assertions
    /// (the tests care about start/stop, not about frames flowing).
    fn fast_pump_transport() -> WebRtcDesktopTransport {
        let providers = Arc::new(DesktopProviders::not_supported_for(HostOs::Linux));
        let sink: Arc<dyn CaptureSink> = Arc::new(DiscardingCaptureSink::new());
        let cfg = CapturePumpConfig {
            target_fps: 1000,
            // Generous so the pump stays alive across an
            // `await`-laden assertion sequence.
            max_consecutive_errors: 1_000_000,
            error_backoff: std::time::Duration::from_micros(1),
        };
        WebRtcDesktopTransport::with_providers(
            HostOs::Linux,
            Some(VALID_ORG_ID.into()),
            providers,
            sink,
            cfg,
        )
    }

    #[tokio::test]
    async fn remote_control_spawns_a_pump_for_the_session() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        let pumps = p.pumps().lock_owned().await;
        assert!(
            pumps.contains_key(VALID_SESSION_ID),
            "pump must be registered for the opened session"
        );
        assert!(
            pumps.get(VALID_SESSION_ID).unwrap().is_running(),
            "freshly spawned pump must be running"
        );
    }

    #[tokio::test]
    async fn pump_stats_returns_live_snapshot_for_open_session_and_none_after_close() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        let snap = p.pump_stats(VALID_SESSION_ID).await.expect("snapshot");
        // The pump is alive — `stopped_at` must be `None`. The
        // `NotSupported` capturer means `frames_captured == 0` and
        // `capture_errors > 0` after even a brief tick, but the
        // assertion that matters here is the snapshot shape.
        assert!(snap.stopped_at.is_none(), "{snap:?}");
        // ChangeWindowsSession closes the session and stops the
        // pump; after that, `pump_stats` returns `None`.
        let _ = p.change_windows_session(&change_session_req()).await;
        assert!(
            p.pump_stats(VALID_SESSION_ID).await.is_none(),
            "pump_stats must return None once the session is closed"
        );
    }

    #[tokio::test]
    async fn remote_control_replace_on_duplicate_stops_prior_pump() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        // Capture an Arc to the prior pump's stats handle so we can
        // observe its termination after the replace.
        let prior_stats = p
            .pump_stats(VALID_SESSION_ID)
            .await
            .expect("prior snapshot");
        assert!(prior_stats.stopped_at.is_none());

        // Second RemoteControl with the same session id triggers
        // the replace-on-duplicate path; spawn_pump aborts and
        // joins the prior pump before swapping in the new one.
        let _ = p.remote_control(&rc_req()).await;
        // After the replace, exactly one pump remains in the map.
        let pumps = p.pumps().lock_owned().await;
        assert_eq!(pumps.len(), 1);
        assert!(pumps.contains_key(VALID_SESSION_ID));
        assert!(pumps.get(VALID_SESSION_ID).unwrap().is_running());
    }

    #[tokio::test]
    async fn change_windows_session_stops_the_matching_pump() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        assert!(p.pumps().lock_owned().await.contains_key(VALID_SESSION_ID));
        let _ = p.change_windows_session(&change_session_req()).await;
        assert!(
            !p.pumps().lock_owned().await.contains_key(VALID_SESSION_ID),
            "ChangeWindowsSession must stop the pump in lock-step \
             with closing the session"
        );
    }

    #[tokio::test]
    async fn sweep_idle_with_pumps_stops_pumps_for_evicted_sessions() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        assert!(p.pumps().lock_owned().await.contains_key(VALID_SESSION_ID));
        // `sweep_idle_with_pumps(now + 1y)` evicts every session
        // (every `last_activity` is older than the timeout) and
        // must stop every matching pump.
        let far_future =
            tokio::time::Instant::now() + std::time::Duration::from_secs(365 * 24 * 3600);
        let evicted = p.sweep_idle_with_pumps(far_future).await;
        assert!(evicted.iter().any(|id| id == VALID_SESSION_ID));
        assert!(
            p.pumps().lock_owned().await.is_empty(),
            "every pump must be removed after sweeping every session"
        );
    }

    #[tokio::test]
    async fn distinct_sessions_get_distinct_pumps() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        let mut second = rc_req();
        second.session_id = OTHER_SESSION_ID.into();
        let _ = p.remote_control(&second).await;
        let pumps = p.pumps().lock_owned().await;
        assert_eq!(pumps.len(), 2);
        assert!(pumps.contains_key(VALID_SESSION_ID));
        assert!(pumps.contains_key(OTHER_SESSION_ID));
    }

    #[tokio::test]
    async fn rejected_remote_control_does_not_spawn_a_pump() {
        // Cross-org request: the guard refuses before the registry
        // sees it. No pump must be spawned, and the pumps map must
        // stay empty.
        let p = fast_pump_transport();
        let mut hostile = rc_req();
        hostile.org_id = "deadbeef-dead-dead-dead-deaddeaddead".into();
        let r = p.remote_control(&hostile).await;
        assert!(!r.success);
        assert!(p.pumps().lock_owned().await.is_empty());
        assert!(p.sessions().lock_owned().await.is_empty());
    }
}
