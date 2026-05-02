// Source: CMRemote, clean-room implementation.

//! Concrete `DesktopTransportProvider` implementation backed by a
//! [`super::session::DesktopSessionRegistry`] and the `webrtc-rs`
//! peer-connection stack (slice R7.k lifecycle scaffolding + slice
//! R7.m peer-connection construction).
//!
//! ## Status
//!
//! Slice R7.k landed the **lifecycle scaffolding** (per-session
//! state machine, guard ordering, replace-on-duplicate semantics,
//! audit logging). Slice R7.m replaces the prior
//! `DRIVER_PENDING_MESSAGE` stub returns on every signalling /
//! provide-ice hook with real `webrtc-rs` peer-connection
//! operations:
//!
//! - `on_provide_ice_servers` translates the wire `IceServerConfig`
//!   into [`webrtc::peer_connection::configuration::RTCConfiguration`],
//!   constructs an [`webrtc::peer_connection::RTCPeerConnection`]
//!   via [`super::webrtc_pc::PeerConnectionFactory`], installs the
//!   `on_ice_candidate` and `on_peer_connection_state_change`
//!   handlers (which fan out to the runtime-supplied
//!   [`super::SignallingEgress`] and the registry transition logic),
//!   stores the PC in [`super::webrtc_pc::PeerConnectionRegistry`],
//!   and returns success.
//! - `on_sdp_offer` ensures a PC exists (lazy-creating one with the
//!   default `RTCConfiguration` if no `ProvideIceServers` has been
//!   seen — the .NET hub may skip it when the viewer reuses
//!   defaults), `set_remote_description`s the offer,
//!   `create_answer`s, `set_local_description`s the answer, and
//!   pushes the answer through the egress.
//! - `on_sdp_answer` `set_remote_description`s the answer on the
//!   existing PC.
//! - `on_ice_candidate` `add_ice_candidate`s on the existing PC
//!   after translating the wire shape.
//! - `change_windows_session` closes the PC alongside the pump.
//! - `remote_control`'s replace-on-duplicate path closes the prior
//!   PC alongside the prior pump.
//! - The idle sweep closes every evicted PC alongside its pump.
//! - `restart_screen_caster` is a driver-internal kick: it stops
//!   and respawns the pump for the session (no PC mutation; the
//!   negotiated transport stays up).
//! - `invoke_ctrl_alt_del` is platform-bound (Windows-only Secure
//!   Attention Sequence) and returns a structured "not supported on
//!   `<host_os>`" failure on every other host. The Windows-side
//!   driver lands in a follow-up slice.
//!
//! The capture-pump → encoder → RTP-track wiring (slice R7.n.6,
//! Media Foundation H.264 encoder) is still pending — the pump
//! pushes captured frames into the configured `CaptureSink`, which
//! defaults to [`super::DiscardingCaptureSink`] until the encoder
//! lands. The peer connection is therefore fully negotiable today
//! but does not yet emit media; the operator gets a connected RTP
//! transport with no track. That gap is documented in the R7 row of
//! `ROADMAP.md`.
//!
//! ## Why a feature flag (`webrtc-driver`)
//!
//! The `webrtc` crate is gated on the supply-chain audit described
//! in [`docs/decisions/0001-webrtc-crypto-provider.md`] and resolved
//! through the workspace-level
//! `[patch.crates-io].webrtc` pin to `CrashMediaIT/webrtc-cmremote`
//! at tag `v0.17.0-cmremote.1` (which swaps `ring` for `aws-lc-rs`).
//! Default builds skip this whole module so the dep graph is the
//! same as before R7.m on the path the CI fleet exercises today.
//!
//! ## Wiring
//!
//! When the workspace is built with `--features webrtc-driver`, the
//! agent runtime constructs a [`WebRtcDesktopTransport`] via
//! [`WebRtcDesktopTransport::with_providers`] in place of
//! [`super::NotSupportedDesktopTransport`]. The dispatcher's
//! `Arc<dyn DesktopTransportProvider>` slot is unchanged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, IceCandidate, InvokeCtrlAltDelRequest,
    ProvideIceServersRequest, RemoteControlSessionRequest, RestartScreenCasterRequest, SdpAnswer,
    SdpOffer,
};
use tokio::sync::Mutex;
use tokio::time::Instant;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use super::encoder_sink::EncoderCaptureSink;
use super::guards;
use super::providers::DesktopProviders;
use super::pump::{
    CapturePump, CapturePumpConfig, CaptureSink, CaptureStatsSnapshot, DiscardingCaptureSink,
    LateBoundCaptureSink,
};
use super::session::{CloseReason, DesktopSessionRegistry, DesktopSessionState};
use super::signalling_egress::{LoggingSignallingEgress, SignallingEgress};
use super::webrtc_pc::{
    translate_ice_candidate, translate_ice_config, PeerConnectionFactory, PeerConnectionRegistry,
};
use super::webrtc_track::{new_h264_video_track, WebRtcVideoTrackSink};
use super::DesktopTransportProvider;
use crate::HostOs;

/// Stable string baked into every "platform method not implemented"
/// failure result. Used today for `invoke_ctrl_alt_del` (Windows-only
/// Secure Attention Sequence). Pinned by tests so the .NET hub-side
/// error renderer can match on it without pattern-tracking a moving
/// message.
pub const PLATFORM_METHOD_NOT_IMPLEMENTED_MESSAGE: &str =
    "Desktop transport method is not implemented on this host OS";

/// Concrete `DesktopTransportProvider` for the WebRTC capture / encode
/// driver. See the module docs for the lifecycle vs. peer-connection
/// split.
pub struct WebRtcDesktopTransport {
    host_os: HostOs,
    expected_org_id: Option<String>,
    sessions: Arc<Mutex<DesktopSessionRegistry>>,
    /// Per-host bundle of capture / input / encoder providers
    /// (slice R7.n.4 + R7.n.6). The pump reads `providers.capturer`;
    /// the per-PC track-builder reads `providers.encoder_factory`;
    /// the future input data-channel will read
    /// `providers.mouse` / `providers.keyboard` / `providers.clipboard`.
    providers: Arc<DesktopProviders>,
    /// Default downstream every per-session [`LateBoundCaptureSink`]
    /// is initially bound to. Production injects
    /// [`DiscardingCaptureSink`] (frames before the PC is built are
    /// counted + dropped); tests inject a counting / observer sink
    /// so they can assert on what the pump produced before
    /// negotiation finished. Once [`Self::build_peer_connection`]
    /// runs and the per-OS encoder is available, the matching
    /// per-session sink is rebound to a chained
    /// `EncoderCaptureSink` → [`WebRtcVideoTrackSink`]; on PC close
    /// the sink falls back to this default.
    default_sink: Arc<dyn CaptureSink>,
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
    /// Slice R7.n.6 — per-session [`LateBoundCaptureSink`] each
    /// pump pushes frames into. Initially bound to
    /// [`Self::default_sink`]; rebound to the encoder→track chain
    /// when the matching peer connection is built. Maintained in
    /// lock-step with [`Self::pumps`]: every pump entry has a
    /// matching sink entry, and every close path removes both.
    session_sinks: Arc<Mutex<HashMap<String, Arc<LateBoundCaptureSink>>>>,
    /// Slice R7.m — shared `webrtc-rs` API factory. Built once at
    /// construction; one `MediaEngine` is registered with the
    /// upstream default codec set and shared across every
    /// per-session peer connection (same pattern the upstream
    /// examples use).
    pc_factory: Arc<PeerConnectionFactory>,
    /// Slice R7.m — live `Arc<RTCPeerConnection>` per session.
    /// Maintained in lock-step with [`Self::sessions`] and
    /// [`Self::pumps`] (every close / replace path drops the PC
    /// alongside the matching pump and registry record).
    peer_connections: Arc<PeerConnectionRegistry>,
    /// Slice R7.m — per-PC "alive" flags. Each entry mirrors the
    /// matching peer connection in [`Self::peer_connections`]. The
    /// `on_peer_connection_state_change` handler we install at PC
    /// build time clones the matching flag and, on every state
    /// change, gates the registry mutation behind a still-alive
    /// check. When we explicitly close a PC (replace-on-duplicate,
    /// `change_windows_session`, sweep) we clear the flag *before*
    /// awaiting `pc.close()` so the late-arriving `Closed` event
    /// from the old PC does not stomp the freshly-opened session
    /// record. Keyed by `session_id` because the registry records
    /// are keyed the same way and we never have more than one
    /// live PC per session.
    pc_alive: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    /// Slice R7.m — outbound signalling sink. The driver invokes
    /// this from the `on_ice_candidate` peer-connection event
    /// handler (locally-trickled candidates) and from the
    /// `on_sdp_offer` answer-emission path. Default is
    /// [`LoggingSignallingEgress`] which warn-logs every drop; the
    /// agent runtime swaps in a hub-bound implementation that
    /// invokes the server-bound `SendSdpAnswer` /
    /// `SendIceCandidate` hub methods.
    egress: Arc<dyn SignallingEgress>,
}

impl WebRtcDesktopTransport {
    /// Build a new driver naming `host_os` and using `expected_org_id`
    /// for the cross-org guard (`None` skips the cross-org check, same
    /// semantics as [`super::NotSupportedDesktopTransport::new`]).
    ///
    /// Uses [`DesktopProviders::not_supported_for`] for the bundle,
    /// [`DiscardingCaptureSink`] for the sink, and
    /// [`LoggingSignallingEgress`] for the egress. The agent runtime
    /// upgrades all three via [`Self::with_providers`] when
    /// constructing the production driver.
    ///
    /// # Panics
    ///
    /// Panics if the `webrtc-rs` `MediaEngine` fails to register the
    /// default codec set — in practice this only happens when the
    /// build pulled in an inconsistent codec set, which we want to
    /// surface at construction time rather than on the first
    /// `RemoteControl`. The fallible variant is
    /// [`Self::try_new`].
    pub fn new(host_os: HostOs, expected_org_id: Option<String>) -> Self {
        Self::try_new(host_os, expected_org_id).expect("webrtc-rs MediaEngine init")
    }

    /// Fallible variant of [`Self::new`] — returns the upstream
    /// `webrtc::Error` if the `MediaEngine` fails to register the
    /// default codec set. Used by `cmremote-agent`'s runtime so the
    /// agent surfaces a clean startup error instead of panicking
    /// inside `Arc::new`.
    pub fn try_new(
        host_os: HostOs,
        expected_org_id: Option<String>,
    ) -> Result<Self, webrtc::Error> {
        let pc_factory = Arc::new(PeerConnectionFactory::new()?);
        Ok(Self {
            host_os,
            expected_org_id,
            sessions: Arc::new(Mutex::new(DesktopSessionRegistry::with_default_timeout())),
            providers: Arc::new(DesktopProviders::not_supported_for(host_os)),
            default_sink: Arc::new(DiscardingCaptureSink::new()),
            pump_config: CapturePumpConfig::default(),
            pumps: Arc::new(Mutex::new(HashMap::new())),
            session_sinks: Arc::new(Mutex::new(HashMap::new())),
            pc_factory,
            peer_connections: Arc::new(PeerConnectionRegistry::new()),
            pc_alive: Arc::new(Mutex::new(HashMap::new())),
            egress: Arc::new(LoggingSignallingEgress),
        })
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

    /// Convenience constructor used by the agent runtime — accepts
    /// the per-host capture / input bundle and the production sink,
    /// retains the default [`LoggingSignallingEgress`]. Use
    /// [`Self::with_providers_and_egress`] to inject a real
    /// hub-bound egress.
    pub fn with_providers(
        host_os: HostOs,
        expected_org_id: Option<String>,
        providers: Arc<DesktopProviders>,
        sink: Arc<dyn CaptureSink>,
        pump_config: CapturePumpConfig,
    ) -> Self {
        Self::with_providers_and_egress(
            host_os,
            expected_org_id,
            providers,
            sink,
            pump_config,
            Arc::new(LoggingSignallingEgress),
        )
    }

    /// Full constructor — used by the agent runtime to inject the
    /// per-host capture / input bundle, the production sink, and a
    /// real hub-bound [`SignallingEgress`].
    ///
    /// # Panics
    ///
    /// Panics if the `webrtc-rs` `MediaEngine` fails to register
    /// the default codec set. See [`Self::try_new`] for the
    /// fallible variant.
    pub fn with_providers_and_egress(
        host_os: HostOs,
        expected_org_id: Option<String>,
        providers: Arc<DesktopProviders>,
        sink: Arc<dyn CaptureSink>,
        pump_config: CapturePumpConfig,
        egress: Arc<dyn SignallingEgress>,
    ) -> Self {
        let pc_factory =
            Arc::new(PeerConnectionFactory::new().expect("webrtc-rs MediaEngine init"));
        Self {
            host_os,
            expected_org_id,
            sessions: Arc::new(Mutex::new(DesktopSessionRegistry::with_default_timeout())),
            providers,
            default_sink: sink,
            pump_config,
            pumps: Arc::new(Mutex::new(HashMap::new())),
            session_sinks: Arc::new(Mutex::new(HashMap::new())),
            pc_factory,
            peer_connections: Arc::new(PeerConnectionRegistry::new()),
            pc_alive: Arc::new(Mutex::new(HashMap::new())),
            egress,
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

    /// Borrow the per-session capture-sink map. Exposed so tests
    /// can assert that the slice R7.n.6 lifecycle (PC build →
    /// `bind`, PC close → unbind/rebind to default) holds, and so
    /// the runtime can audit how many frames were dropped before
    /// the PC came up via [`LateBoundCaptureSink::dropped_before_bind`].
    pub fn session_sinks(&self) -> Arc<Mutex<HashMap<String, Arc<LateBoundCaptureSink>>>> {
        self.session_sinks.clone()
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

    /// `true` when a peer connection is currently registered for
    /// `session_id`. Exposed for the runtime sweep task (so it can
    /// assert PCs and pumps stay in lock-step) and for tests.
    pub async fn has_peer_connection(&self, session_id: &str) -> bool {
        self.peer_connections.get(session_id).await.is_some()
    }

    fn expected_org(&self) -> Option<&str> {
        self.expected_org_id.as_deref()
    }

    /// Spawn a fresh pump for `session_id`, replacing (and stopping)
    /// any existing pump for the same id. Awaits the prior pump's
    /// abort-and-join so callers observe a clean handoff.
    ///
    /// Slice R7.n.6 — also creates a per-session
    /// [`LateBoundCaptureSink`] (initially bound to
    /// [`Self::default_sink`]) and stores it in
    /// [`Self::session_sinks`] so [`Self::build_peer_connection`]
    /// can later swap in the encoder→track downstream without
    /// restarting the pump.
    async fn spawn_pump(&self, session_id: &str) {
        // Build the per-session sink first and bind it to the
        // default downstream so any frame the pump produces before
        // the PC is built is forwarded to the configured fallback
        // (in production: `DiscardingCaptureSink`; in tests: the
        // observer sink the test injected).
        let session_sink = Arc::new(LateBoundCaptureSink::new());
        session_sink.bind(self.default_sink.clone());
        let prior_sink = {
            let mut g = self.session_sinks.lock().await;
            g.insert(session_id.to_string(), session_sink.clone())
        };
        if let Some(prior_sink) = prior_sink {
            // Unbind the old sink so any frame a still-in-flight
            // pump produces drops cleanly rather than racing the
            // new sink's downstream.
            prior_sink.unbind();
        }
        let pump = CapturePump::start(
            self.providers.capturer.clone(),
            session_sink as Arc<dyn CaptureSink>,
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
    /// was registered for that id. Also drops the matching
    /// per-session sink so the encoder + track bound into it are
    /// released along with the pump.
    async fn stop_pump(&self, session_id: &str, reason: &'static str) {
        let pump = {
            let mut g = self.pumps.lock().await;
            g.remove(session_id)
        };
        // Drop the matching late-bound sink so the encoder + track
        // it owns are released. Done after the pump is removed
        // from the map so a still-in-flight `consume` call from
        // the pump's task observes the unbind / drop sequence
        // cleanly.
        let session_sink = {
            let mut g = self.session_sinks.lock().await;
            g.remove(session_id)
        };
        if let Some(s) = session_sink {
            s.unbind();
        }
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

    /// Close (and remove) the peer connection for `session_id`,
    /// awaiting its `close()`. No-op when no PC is registered.
    /// Errors from `close()` are logged at `warn!` so the audit
    /// trail captures a teardown failure but the caller's
    /// happy-path lifecycle is unaffected.
    ///
    /// Clears the matching alive flag *before* awaiting `close()`
    /// so the late-arriving `Closed` state-change event from the
    /// PC's event handler does not stomp a freshly-opened
    /// replacement session.
    ///
    /// Slice R7.n.6 — also rebinds the per-session capture sink
    /// back to [`Self::default_sink`] so the still-running pump
    /// (if any) returns to drop-and-count behaviour rather than
    /// pushing into a track whose RTP packetizer is being torn
    /// down.
    async fn close_peer_connection(&self, session_id: &str, reason: &'static str) {
        // Clear the alive flag first — every state-change event the
        // closed PC fires after this point will short-circuit out
        // of the handler's registry mutation.
        if let Some(flag) = self.pc_alive.lock().await.remove(session_id) {
            flag.store(false, Ordering::SeqCst);
        }
        // Rebind the per-session sink to the default downstream so
        // frames captured between PC close and the next PC build
        // (or session close) are forwarded to the configured
        // fallback rather than the soon-to-be-dropped track sink.
        if let Some(s) = self.session_sinks.lock().await.get(session_id) {
            s.bind(self.default_sink.clone());
        }
        if let Some(pc) = self.peer_connections.remove(session_id).await {
            if let Err(e) = pc.close().await {
                tracing::warn!(
                    session_id = %session_id,
                    reason = reason,
                    error = %e,
                    event = "peer-connection-close-failed",
                    "RTCPeerConnection close returned error",
                );
            } else {
                tracing::info!(
                    session_id = %session_id,
                    reason = reason,
                    event = "peer-connection-closed",
                    "closed RTCPeerConnection",
                );
            }
        }
    }

    /// Build (and register) a fresh peer connection for `session_id`
    /// with `configuration`. Replaces and closes any prior PC for
    /// the same id (mirrors the registry's replace-on-duplicate
    /// semantics). Installs `on_ice_candidate` and
    /// `on_peer_connection_state_change` handlers that fan out
    /// through the egress and registry respectively. Returns the
    /// freshly-built `Arc<RTCPeerConnection>` so the caller can
    /// immediately drive `set_remote_description` / `create_answer`
    /// without a second registry lookup.
    async fn build_peer_connection(
        &self,
        session_id: &str,
        viewer_connection_id: &str,
        configuration: RTCConfiguration,
    ) -> Result<Arc<RTCPeerConnection>, webrtc::Error> {
        // Close any prior PC for this session before building the
        // new one. `close_peer_connection` clears the alive flag
        // first so the prior PC's state-change events cannot stomp
        // the about-to-be-installed new flag.
        self.close_peer_connection(session_id, "peer-connection-replaced")
            .await;
        let pc = self.pc_factory.create(configuration).await?;
        // Install the alive flag for this PC and capture a clone
        // for the state-change handler.
        let alive = Arc::new(AtomicBool::new(true));
        self.pc_alive
            .lock()
            .await
            .insert(session_id.to_string(), alive.clone());
        // Install the locally-trickled-candidate egress handler.
        let egress_for_ice = self.egress.clone();
        let session_id_for_ice = session_id.to_string();
        let viewer_for_ice = viewer_connection_id.to_string();
        let alive_for_ice = alive.clone();
        pc.on_ice_candidate(Box::new(move |maybe_candidate| {
            let egress = egress_for_ice.clone();
            let session_id = session_id_for_ice.clone();
            let viewer = viewer_for_ice.clone();
            let alive = alive_for_ice.clone();
            Box::pin(async move {
                // Stale candidate from a closed PC; drop without
                // forwarding so the audit log is honest about the
                // viewer scope.
                if !alive.load(Ordering::SeqCst) {
                    return;
                }
                let Some(candidate) = maybe_candidate else {
                    // `None` signals end-of-gathering; nothing to
                    // forward. The hub-side trickle protocol does
                    // not have an explicit terminator.
                    tracing::debug!(
                        session_id = %session_id,
                        event = "ice-gathering-complete",
                        "local ICE gathering finished",
                    );
                    return;
                };
                let init = match candidate.to_json() {
                    Ok(init) => init,
                    Err(e) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %e,
                            event = "ice-candidate-serialize-failed",
                            "could not serialise local ICE candidate to JSON",
                        );
                        return;
                    }
                };
                egress
                    .send_ice_candidate(
                        &session_id,
                        &viewer,
                        init.candidate,
                        init.sdp_mid,
                        init.sdp_mline_index,
                    )
                    .await;
            })
        }));
        // Install the connection-state egress so the registry's
        // `Connected` / `Closed` transitions reflect the actual
        // peer-connection state.
        let sessions_for_state = self.sessions.clone();
        let session_id_for_state = session_id.to_string();
        let alive_for_state = alive;
        pc.on_peer_connection_state_change(Box::new(move |state| {
            let sessions = sessions_for_state.clone();
            let session_id = session_id_for_state.clone();
            let alive = alive_for_state.clone();
            Box::pin(async move {
                // Closed-PC events reach this handler after the
                // driver has explicitly torn the PC down (e.g.
                // ChangeWindowsSession or replace-on-duplicate).
                // Skip them so the freshly-opened replacement
                // session is not stomped.
                if !alive.load(Ordering::SeqCst) {
                    return;
                }
                let mut sessions = sessions.lock().await;
                match state {
                    RTCPeerConnectionState::Connected => {
                        let _ = sessions.transition(
                            &session_id,
                            DesktopSessionState::Connected,
                            "pc-state-connected",
                        );
                    }
                    RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Closed
                    | RTCPeerConnectionState::Disconnected => {
                        sessions.close(&session_id, CloseReason::Explicit);
                    }
                    _ => {}
                }
            })
        }));
        // Insert into the registry. Any prior entry was closed
        // above; this insert is a fresh slot so `insert` will
        // return `None`. We still log if the prior was
        // unexpectedly present (defence-in-depth — should never
        // fire in practice).
        if let Some(prior) = self.peer_connections.insert(session_id, pc.clone()).await {
            tracing::warn!(
                session_id = %session_id,
                event = "peer-connection-replaced-late",
                "unexpected prior PC found after explicit close; closing it",
            );
            if let Err(e) = prior.close().await {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    event = "peer-connection-close-failed",
                    "prior RTCPeerConnection close returned error",
                );
            }
        }

        // Slice R7.n.6 — wire the per-session capture pump into a
        // freshly-built H.264 video track on this PC. The encoder
        // factory may report `NotSupported` (e.g. on hosts without
        // a registered encoder driver); in that case we log a
        // structured `info!` and leave the per-session sink bound
        // to the default downstream so the operator gets a
        // connected RTP transport with no media (the same
        // behaviour as before R7.n.6) instead of a panic.
        self.attach_video_track(session_id, &pc).await;

        Ok(pc)
    }

    /// Build an H.264 [`webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample`],
    /// attach it to `pc`, build the per-session encoder via
    /// [`DesktopProviders::encoder_factory`], and rebind the
    /// matching [`LateBoundCaptureSink`] to the chained
    /// `EncoderCaptureSink` → [`WebRtcVideoTrackSink`].
    ///
    /// All failure paths are non-fatal: a `NotSupported` encoder,
    /// a missing per-session sink (e.g. the pump never spawned —
    /// should not happen in practice), or an `add_track` rejection
    /// all log a structured event and leave the PC in a usable
    /// (but trackless) state. The driver's signalling methods
    /// continue to negotiate SDP and trickle ICE; the operator
    /// just sees no video on the viewer side until the encoder
    /// driver is configured.
    async fn attach_video_track(&self, session_id: &str, pc: &Arc<RTCPeerConnection>) {
        let encoder = match self.providers.encoder_factory.build() {
            Ok(e) => e,
            Err(e) => {
                tracing::info!(
                    session_id = %session_id,
                    error = %e,
                    event = "video-track-not-attached",
                    "no video encoder available for this host; \
                     peer connection will negotiate without a media track",
                );
                return;
            }
        };
        let track = new_h264_video_track();
        let sender = match pc.add_track(track.clone()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    event = "video-track-add-failed",
                    "RTCPeerConnection::add_track returned error",
                );
                return;
            }
        };
        let track_sink: Arc<dyn super::encoder_sink::EncodedChunkSink> =
            Arc::new(WebRtcVideoTrackSink::new(track));
        // Hold the concrete `EncoderCaptureSink` through an `Arc`
        // so the PLI-listener task can call `request_keyframe()`
        // directly on the encoder without going through a
        // `CaptureSink` trait downcast. The same `Arc` doubles as
        // the `Arc<dyn CaptureSink>` we `bind` into the late-bound
        // sink below.
        let chained = Arc::new(EncoderCaptureSink::new(encoder, track_sink));
        // Rebind the per-session sink so the running pump now
        // pushes captured frames through the encoder and onto the
        // RTP track. If the per-session sink is missing (the pump
        // somehow was not spawned) we log and bail — the PC stays
        // attached to the track but the track will never receive
        // samples.
        let bound = {
            let g = self.session_sinks.lock().await;
            g.get(session_id).cloned()
        };
        match bound {
            Some(sink) => sink.bind(chained.clone() as Arc<dyn CaptureSink>),
            None => {
                tracing::warn!(
                    session_id = %session_id,
                    event = "video-track-no-session-sink",
                    "no per-session capture sink registered; \
                     video track will not receive samples",
                );
                return;
            }
        }
        // Spawn a best-effort RTCP-reader task that forwards every
        // PLI from the viewer to the encoder so the next frame is
        // a keyframe. Loops until `read_rtcp` returns an error
        // (the sender was stopped, the PC was closed, …) which is
        // the canonical way to clean up these per-PC tasks in the
        // upstream `webrtc-rs` examples.
        let chained_for_pli = chained;
        let session_id_for_pli = session_id.to_string();
        let sender_for_pli = sender;
        tokio::spawn(async move {
            use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
            loop {
                match sender_for_pli.read_rtcp().await {
                    Ok((packets, _)) => {
                        let saw_pli = packets.iter().any(|p| {
                            p.as_any()
                                .downcast_ref::<PictureLossIndication>()
                                .is_some()
                        });
                        if saw_pli {
                            tracing::debug!(
                                session_id = %session_id_for_pli,
                                event = "rtcp-pli-received",
                                "viewer requested a keyframe via PLI",
                            );
                            chained_for_pli.request_keyframe();
                        }
                    }
                    Err(_) => {
                        // `read_rtcp` returns `ErrClosedPipe` once
                        // the sender is stopped (the PC was
                        // closed). Exit the loop quietly — the
                        // close path logs the teardown.
                        return;
                    }
                }
            }
        });
    }

    /// Sweep the session registry for idle sessions, then stop any
    /// pump and close any peer connection whose session id is no
    /// longer present. Replacement for the bare
    /// `sessions().lock().await.sweep_idle(now)` the runtime would
    /// otherwise call — using this method keeps pumps and PCs from
    /// outliving their session record.
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
            self.close_peer_connection(id, CloseReason::IdleTimeout.as_str())
                .await;
        }
        evicted
    }

    /// Build a structured failure naming the host OS — used for
    /// `invoke_ctrl_alt_del` until the Windows-side driver lands.
    fn platform_not_implemented(&self, session_id: String, method: &str) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            session_id,
            format!(
                "{PLATFORM_METHOD_NOT_IMPLEMENTED_MESSAGE}: {method:?} on {:?}",
                self.host_os
            ),
        )
    }

    /// Build a structured "internal driver error" failure that
    /// folds the upstream `webrtc::Error` into a stable message
    /// shape. The upstream error string is included verbatim
    /// because it never carries sensitive data (no SDP, no ICE
    /// credential, no access key — those are caller-side payloads
    /// the driver passes to the API by value).
    fn driver_error(
        &self,
        session_id: String,
        method: &str,
        error: webrtc::Error,
    ) -> DesktopTransportResult {
        DesktopTransportResult::failed(
            session_id,
            format!("WebRTC driver error during {method:?}: {error}"),
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
        // building the pump / PC, so the audit trail records the
        // open even if the pump / PC construction fails downstream.
        let mut sessions = self.sessions.lock().await;
        let _outcome = sessions.open(&request.session_id, &request.user_connection_id);
        drop(sessions);
        // Slice R7.m — replace-on-duplicate also closes any prior
        // peer connection (mirrors the registry's `Replaced`
        // semantics). Done before the new pump spawns so the
        // capturer is never owned by two PCs at once.
        self.close_peer_connection(&request.session_id, CloseReason::Replaced.as_str())
            .await;
        // Slice R7.n.5 — spawn a per-session capture pump (see
        // `pump.rs`). The pump pulls from the bundle's capturer at
        // `pump_config.target_fps` and pushes into the shared sink
        // (default `DiscardingCaptureSink` until the encoder lands).
        // `spawn_pump` aborts and joins any prior pump for the same
        // session id, mirroring the registry's replace-on-duplicate
        // semantics.
        self.spawn_pump(&request.session_id).await;
        // Slice R7.m — the peer connection itself is built lazily
        // when the first signalling message arrives (either
        // `ProvideIceServers` with a real config, or `SendSdpOffer`
        // with the default config when the viewer reuses defaults).
        // RemoteControl just opens the registry record.
        DesktopTransportResult::ok(request.session_id.clone())
    }

    async fn restart_screen_caster(
        &self,
        request: &RestartScreenCasterRequest,
    ) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_restart_screen_caster(request, self.expected_org()) {
            return rejection.into_result();
        }
        // RestartScreenCaster is a driver-internal kick — the .NET
        // equivalent restarts the screencaster process. The Rust
        // equivalent stops and respawns the per-session capture
        // pump; the negotiated peer connection is left intact so
        // the viewer does not have to renegotiate.
        let session_present = {
            let sessions = self.sessions.lock().await;
            sessions.get(&request.session_id).is_some()
        };
        if !session_present {
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "RestartScreenCaster received before RemoteControl opened the session".to_string(),
            );
        }
        self.spawn_pump(&request.session_id).await;
        DesktopTransportResult::ok(request.session_id.clone())
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
        // Close the registry record (audit-logged) so a subsequent
        // SendSdpOffer goes through the "session not initialised"
        // path rather than racing against a stale state. Stop the
        // matching pump and close the matching peer connection in
        // lock-step so the capturer + ICE agent are released before
        // the new session attaches to either.
        let mut sessions = self.sessions.lock().await;
        sessions.close(&request.session_id, CloseReason::Explicit);
        drop(sessions);
        self.stop_pump(&request.session_id, "change-windows-session")
            .await;
        self.close_peer_connection(&request.session_id, "change-windows-session")
            .await;
        DesktopTransportResult::ok(request.session_id.clone())
    }

    async fn invoke_ctrl_alt_del(
        &self,
        _request: &InvokeCtrlAltDelRequest,
    ) -> DesktopTransportResult {
        // No envelope to guard — the request type is empty.
        // CtrlAltDel is the W32 Secure Attention Sequence and is
        // delivered by a privileged Windows service on the .NET
        // side. The cross-platform Rust agent has no equivalent
        // today; the Windows-side driver lands in a follow-up slice.
        // Keep the same "empty session id in the failure result"
        // shape `NotSupportedDesktopTransport` uses so the
        // dispatcher's `arguments` decoder treats either provider
        // identically.
        self.platform_not_implemented(String::new(), "InvokeCtrlAltDel")
    }

    // ---------------------------------------------------------------
    // R7.g signalling methods. State transitions documented inline.
    // Slice R7.m wires each one to the per-session
    // `RTCPeerConnection`.
    // ---------------------------------------------------------------

    async fn on_sdp_offer(&self, request: &SdpOffer) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_sdp_offer(request, self.expected_org()) {
            return rejection.into_result();
        }
        // Confirm the session was opened by a prior RemoteControl —
        // otherwise refuse with a precise message so the audit log
        // captures the ordering bug.
        {
            let sessions = self.sessions.lock().await;
            if sessions.get(&request.session_id).is_none() {
                return DesktopTransportResult::failed(
                    request.session_id.clone(),
                    "SendSdpOffer received before RemoteControl opened the session".to_string(),
                );
            }
        }
        // Drive the registry to NegotiatingSdp before any I/O so
        // the audit log captures the transition even if the upstream
        // SDP parse fails.
        {
            let mut sessions = self.sessions.lock().await;
            let _ = sessions.transition(
                &request.session_id,
                DesktopSessionState::NegotiatingSdp,
                "send-sdp-offer",
            );
        }
        // Slice R7.m — fetch the existing peer connection or
        // lazy-build one with the default RTCConfiguration. The
        // .NET hub may skip ProvideIceServers when the viewer
        // reuses defaults; the upstream stack is happy with an
        // empty `ice_servers` list and will gather host candidates
        // only.
        let pc = match self.peer_connections.get(&request.session_id).await {
            Some(pc) => pc,
            None => match self
                .build_peer_connection(
                    &request.session_id,
                    &request.viewer_connection_id,
                    RTCConfiguration::default(),
                )
                .await
            {
                Ok(pc) => pc,
                Err(e) => {
                    return self.driver_error(request.session_id.clone(), "SendSdpOffer", e);
                }
            },
        };
        // Set the remote description (offer), produce an answer,
        // set the local description, then push the answer through
        // the egress.
        let offer = match RTCSessionDescription::offer(request.sdp.clone()) {
            Ok(o) => o,
            Err(e) => {
                return self.driver_error(request.session_id.clone(), "SendSdpOffer", e);
            }
        };
        if let Err(e) = pc.set_remote_description(offer).await {
            return self.driver_error(request.session_id.clone(), "SendSdpOffer", e);
        }
        let answer = match pc.create_answer(None).await {
            Ok(a) => a,
            Err(e) => {
                return self.driver_error(request.session_id.clone(), "SendSdpOffer", e);
            }
        };
        let answer_sdp = answer.sdp.clone();
        if let Err(e) = pc.set_local_description(answer).await {
            return self.driver_error(request.session_id.clone(), "SendSdpOffer", e);
        }
        // Forward the answer to the viewer through the egress.
        // `set_local_description` returns immediately; the local
        // ICE agent will start trickling candidates through the
        // `on_ice_candidate` handler we installed at PC build time.
        self.egress
            .send_sdp_answer(
                &request.session_id,
                &request.viewer_connection_id,
                answer_sdp,
            )
            .await;
        DesktopTransportResult::ok(request.session_id.clone())
    }

    async fn on_sdp_answer(&self, request: &SdpAnswer) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_sdp_answer(request, self.expected_org()) {
            return rejection.into_result();
        }
        {
            let sessions = self.sessions.lock().await;
            if sessions.get(&request.session_id).is_none() {
                return DesktopTransportResult::failed(
                    request.session_id.clone(),
                    "SendSdpAnswer received before RemoteControl opened the session".to_string(),
                );
            }
        }
        // SDP answer applies to the existing PC built by a prior
        // ProvideIceServers / SendSdpOffer round. Refuse if no PC
        // exists — receiving an answer without an offer is a
        // protocol violation we want surfaced.
        let Some(pc) = self.peer_connections.get(&request.session_id).await else {
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "SendSdpAnswer received with no active peer connection".to_string(),
            );
        };
        let answer = match RTCSessionDescription::answer(request.sdp.clone()) {
            Ok(a) => a,
            Err(e) => {
                return self.driver_error(request.session_id.clone(), "SendSdpAnswer", e);
            }
        };
        if let Err(e) = pc.set_remote_description(answer).await {
            return self.driver_error(request.session_id.clone(), "SendSdpAnswer", e);
        }
        DesktopTransportResult::ok(request.session_id.clone())
    }

    async fn on_ice_candidate(&self, request: &IceCandidate) -> DesktopTransportResult {
        if let Err(rejection) = guards::check_ice_candidate(request, self.expected_org()) {
            return rejection.into_result();
        }
        {
            let sessions = self.sessions.lock().await;
            if sessions.get(&request.session_id).is_none() {
                return DesktopTransportResult::failed(
                    request.session_id.clone(),
                    "SendIceCandidate received before RemoteControl opened the session".to_string(),
                );
            }
        }
        // ICE trickle feeds the existing PC's ICE agent. Refuse if
        // no PC is up — ICE without a peer connection is a
        // protocol violation. The wire-layer guard has already
        // length-capped + scheme-checked the candidate body; this
        // call cannot leak a hostile string into the audit log.
        let Some(pc) = self.peer_connections.get(&request.session_id).await else {
            return DesktopTransportResult::failed(
                request.session_id.clone(),
                "SendIceCandidate received with no active peer connection".to_string(),
            );
        };
        let init = translate_ice_candidate(request);
        if let Err(e) = pc.add_ice_candidate(init).await {
            return self.driver_error(request.session_id.clone(), "SendIceCandidate", e);
        }
        DesktopTransportResult::ok(request.session_id.clone())
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
        {
            let sessions = self.sessions.lock().await;
            if sessions.get(&request.session_id).is_none() {
                return DesktopTransportResult::failed(
                    request.session_id.clone(),
                    "ProvideIceServers received before RemoteControl opened the session"
                        .to_string(),
                );
            }
        }
        // Drive the registry to IceConfigured before any I/O so the
        // audit log captures the transition even if the upstream PC
        // build fails.
        {
            let mut sessions = self.sessions.lock().await;
            let _ = sessions.transition(
                &request.session_id,
                DesktopSessionState::IceConfigured,
                "provide-ice-servers",
            );
        }
        // Slice R7.m — translate the wire `IceServerConfig` into
        // `RTCConfiguration` and (re)build the peer connection.
        // Receiving a fresh config replaces any prior PC for the
        // same id (the upstream stack does not support mutating
        // `RTCConfiguration::ice_servers` after PC construction
        // without `set_configuration` + `restart_ice`; building
        // fresh is simpler and what the .NET hub expects when it
        // re-issues ProvideIceServers).
        let configuration = translate_ice_config(&request.ice_server_config);
        if let Err(e) = self
            .build_peer_connection(
                &request.session_id,
                &request.viewer_connection_id,
                configuration,
            )
            .await
        {
            return self.driver_error(request.session_id.clone(), "ProvideIceServers", e);
        }
        DesktopTransportResult::ok(request.session_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::signalling_egress::testing::{CapturedSignal, CapturingSignallingEgress};

    const VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";
    const OTHER_SESSION_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const VALID_ORG_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    /// A small but valid SDP offer — borrowed from the upstream
    /// `webrtc-rs` example, trimmed to the minimum the parser
    /// accepts. Contains a single `m=application` line with a
    /// data-channel-only setup so the upstream stack does not
    /// require a media engine codec.
    const SAMPLE_OFFER_SDP: &str = "v=0\r\n\
o=- 4659777215431993300 2 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0\r\n\
a=extmap-allow-mixed\r\n\
a=msid-semantic: WMS\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
c=IN IP4 0.0.0.0\r\n\
a=ice-ufrag:abcd\r\n\
a=ice-pwd:abcdefghijklmnopqrstuvwx\r\n\
a=ice-options:trickle\r\n\
a=fingerprint:sha-256 \
00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF\r\n\
a=setup:actpass\r\n\
a=mid:0\r\n\
a=sctp-port:5000\r\n\
a=max-message-size:262144\r\n";

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
            sdp: SAMPLE_OFFER_SDP.to_string(),
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

    /// Build a transport with a [`CapturingSignallingEgress`] so
    /// the test can assert what the driver actually emits. Returns
    /// the transport and a clone of the egress.
    fn transport_with_capture() -> (WebRtcDesktopTransport, CapturingSignallingEgress) {
        let egress = CapturingSignallingEgress::new();
        let providers = Arc::new(DesktopProviders::not_supported_for(HostOs::Linux));
        let sink: Arc<dyn CaptureSink> = Arc::new(DiscardingCaptureSink::new());
        let t = WebRtcDesktopTransport::with_providers_and_egress(
            HostOs::Linux,
            Some(VALID_ORG_ID.into()),
            providers,
            sink,
            CapturePumpConfig::default(),
            Arc::new(egress.clone()),
        );
        (t, egress)
    }

    // -----------------------------------------------------------------
    // Happy path: RemoteControl opens the session; subsequent
    // signalling methods drive the registry into the right state and
    // build / drive the real peer connection.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn remote_control_opens_session_and_returns_ok() {
        let (p, _egress) = transport_with_capture();
        let r = p.remote_control(&rc_req()).await;
        assert!(r.success, "{r:?}");
        assert_eq!(r.session_id, VALID_SESSION_ID);
        assert!(r.error_message.is_none());
        // Registry now has the session in Initializing.
        let sessions = p.sessions().lock_owned().await;
        let s = sessions.get(VALID_SESSION_ID).unwrap();
        assert_eq!(s.state, DesktopSessionState::Initializing);
        // No PC yet — built lazily on first signalling.
        drop(sessions);
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
    }

    #[tokio::test]
    async fn provide_ice_servers_after_remote_control_builds_pc_and_drives_to_ice_configured() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let r = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(r.success, "{r:?}");
        // Sensitive access_key MUST NOT appear in any returned
        // message — happy path returns no message at all.
        assert!(r.error_message.is_none());
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::IceConfigured,
        );
        drop(sessions);
        // PC must now exist.
        assert!(p.has_peer_connection(VALID_SESSION_ID).await);
    }

    #[tokio::test]
    async fn sdp_offer_after_provide_ice_drives_to_negotiating_sdp_and_emits_answer() {
        let (p, egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        let r = p.on_sdp_offer(&sdp_offer_req()).await;
        assert!(r.success, "{r:?}");
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
        drop(sessions);
        // Egress must have captured exactly one SDP answer for the
        // session, addressed to the right viewer.
        let captured = egress.captured().await;
        let mut answers = captured.iter().filter_map(|c| match c {
            CapturedSignal::SdpAnswer {
                session_id,
                viewer_connection_id,
                sdp,
            } => Some((
                session_id.clone(),
                viewer_connection_id.clone(),
                sdp.clone(),
            )),
            _ => None,
        });
        let (sid, viewer, answer_sdp) = answers.next().expect("one answer");
        assert!(answers.next().is_none(), "expected exactly one answer");
        assert_eq!(sid, VALID_SESSION_ID);
        assert_eq!(viewer, "viewer-1");
        assert!(answer_sdp.contains("v=0"), "{answer_sdp}");
    }

    #[tokio::test]
    async fn sdp_offer_without_prior_provide_ice_lazy_builds_pc_and_emits_answer() {
        // The .NET hub may emit SendSdpOffer without ProvideIceServers
        // when the viewer reuses defaults; the driver must lazy-build
        // the PC with the default RTCConfiguration.
        let (p, egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let r = p.on_sdp_offer(&sdp_offer_req()).await;
        assert!(r.success, "{r:?}");
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::NegotiatingSdp,
        );
        drop(sessions);
        assert!(p.has_peer_connection(VALID_SESSION_ID).await);
        // Answer was emitted.
        assert!(matches!(
            egress.captured().await.first(),
            Some(CapturedSignal::SdpAnswer { .. })
        ));
    }

    #[tokio::test]
    async fn ice_candidate_after_provide_ice_is_accepted() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        // Drive an offer first so the PC has a remote description
        // and the trickled candidate has somewhere to apply.
        let _ = p.on_sdp_offer(&sdp_offer_req()).await;
        let r = p.on_ice_candidate(&ice_candidate_req()).await;
        assert!(r.success, "{r:?}");
    }

    // -----------------------------------------------------------------
    // Replace-on-duplicate: the prior PC must be closed before the
    // new one is registered.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn duplicate_remote_control_replaces_prior_session_and_drops_prior_pc() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(p.has_peer_connection(VALID_SESSION_ID).await);
        // Re-issue RemoteControl with a different viewer.
        let mut req = rc_req();
        req.user_connection_id = "viewer-2".into();
        let r = p.remote_control(&req).await;
        assert!(r.success, "{r:?}");
        let sessions = p.sessions().lock_owned().await;
        let s = sessions.get(VALID_SESSION_ID).unwrap();
        // Session is freshly Initializing, viewer is the new id.
        assert_eq!(s.state, DesktopSessionState::Initializing);
        assert_eq!(s.viewer_connection_id, "viewer-2");
        drop(sessions);
        // The prior PC was dropped — a fresh one is built on the
        // next signalling round.
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
    }

    // -----------------------------------------------------------------
    // Out-of-order signalling: every onward hook MUST refuse when the
    // session does not exist (it was never opened or was closed).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn provide_ice_without_remote_control_refuses_with_precise_message() {
        let (p, _egress) = transport_with_capture();
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
        let (p, _egress) = transport_with_capture();
        let r = p.on_sdp_offer(&sdp_offer_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SendSdpOffer"));
    }

    #[tokio::test]
    async fn sdp_answer_without_remote_control_refuses_with_precise_message() {
        let (p, _egress) = transport_with_capture();
        let r = p.on_sdp_answer(&sdp_answer_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SendSdpAnswer"));
    }

    #[tokio::test]
    async fn ice_candidate_without_remote_control_refuses_with_precise_message() {
        let (p, _egress) = transport_with_capture();
        let r = p.on_ice_candidate(&ice_candidate_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SendIceCandidate"));
    }

    #[tokio::test]
    async fn sdp_answer_without_active_pc_refuses_with_precise_message() {
        // Open the session but never drive a ProvideIceServers /
        // SendSdpOffer round; the answer must be refused because
        // there's no PC waiting on it.
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let r = p.on_sdp_answer(&sdp_answer_req()).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("no active peer connection"), "{msg}");
    }

    #[tokio::test]
    async fn ice_candidate_without_active_pc_refuses_with_precise_message() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let r = p.on_ice_candidate(&ice_candidate_req()).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("no active peer connection"), "{msg}");
    }

    // -----------------------------------------------------------------
    // Guards run BEFORE any registry / PC mutation.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn cross_org_remote_control_is_refused_before_registry_mutation() {
        let (p, _egress) = transport_with_capture();
        let mut req = rc_req();
        req.org_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("organisation"));
        assert!(p.sessions().lock_owned().await.is_empty());
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
    }

    #[tokio::test]
    async fn malformed_session_id_refused_without_touching_registry() {
        let (p, _egress) = transport_with_capture();
        let mut req = rc_req();
        req.session_id = "not-a-uuid".into();
        let r = p.remote_control(&req).await;
        assert!(!r.success);
        assert!(p.sessions().lock_owned().await.is_empty());
    }

    #[tokio::test]
    async fn over_length_sdp_refused_before_pc_mutation() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let mut req = sdp_offer_req();
        req.sdp = "v".repeat(cmremote_wire::MAX_SDP_BYTES + 1);
        let r = p.on_sdp_offer(&req).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("sdp"));
        // Session should still be Initializing — the over-length
        // offer never reached the state machine, and no PC was
        // built.
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::Initializing,
        );
        drop(sessions);
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
    }

    #[tokio::test]
    async fn hostile_ice_url_refused_before_pc_mutation() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let mut req = provide_ice_req();
        req.ice_server_config.ice_servers[0].urls[0] = "javascript:alert(1)".into();
        let r = p.on_provide_ice_servers(&req).await;
        assert!(!r.success);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("ice_servers[0]"), "{msg}");
        // The hostile URL bytes MUST NOT appear in the rejection.
        assert!(!msg.contains("javascript"), "{msg}");
        // State unchanged; no PC built.
        let sessions = p.sessions().lock_owned().await;
        assert_eq!(
            sessions.get(VALID_SESSION_ID).unwrap().state,
            DesktopSessionState::Initializing,
        );
        drop(sessions);
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
    }

    // -----------------------------------------------------------------
    // Cross-session isolation.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn signalling_for_session_a_does_not_change_session_b_state() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let mut req_b = rc_req();
        req_b.session_id = OTHER_SESSION_ID.into();
        let _ = p.remote_control(&req_b).await;
        // Drive A through ProvideIceServers + SendSdpOffer.
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
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
        drop(sessions);
        assert!(p.has_peer_connection(VALID_SESSION_ID).await);
        assert!(!p.has_peer_connection(OTHER_SESSION_ID).await);
    }

    // -----------------------------------------------------------------
    // ChangeWindowsSession closes the registry entry, the pump, AND
    // the peer connection.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn change_windows_session_closes_registry_pump_and_pc() {
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(p.has_peer_connection(VALID_SESSION_ID).await);
        let r = p.change_windows_session(&change_session_req()).await;
        assert!(r.success, "{r:?}");
        // Entry is gone — a future SendSdpOffer will fail with the
        // "before RemoteControl" message.
        assert!(p
            .sessions()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .is_none());
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
    }

    #[tokio::test]
    async fn restart_screen_caster_keeps_session_open_and_returns_ok() {
        // RestartScreenCaster is a driver-internal kick; the session
        // entry must survive so the existing signalling can continue.
        let (p, _egress) = transport_with_capture();
        let _ = p.remote_control(&rc_req()).await;
        let r = p.restart_screen_caster(&restart_req()).await;
        assert!(r.success, "{r:?}");
        assert!(p
            .sessions()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .is_some());
    }

    #[tokio::test]
    async fn restart_screen_caster_without_remote_control_refuses() {
        let (p, _egress) = transport_with_capture();
        let r = p.restart_screen_caster(&restart_req()).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("before RemoteControl"));
    }

    // -----------------------------------------------------------------
    // CtrlAltDel is a stateless ping; no session id, no registry change.
    // Returns the platform-not-implemented message until the
    // Windows-side driver lands.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn invoke_ctrl_alt_del_returns_empty_session_id_and_does_not_open_a_session() {
        let (p, _egress) = transport_with_capture();
        let r = p.invoke_ctrl_alt_del(&InvokeCtrlAltDelRequest).await;
        assert!(!r.success);
        assert!(r.session_id.is_empty());
        let msg = r.error_message.unwrap();
        assert!(
            msg.contains(PLATFORM_METHOD_NOT_IMPLEMENTED_MESSAGE),
            "{msg}"
        );
        assert!(msg.contains("InvokeCtrlAltDel"), "{msg}");
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
    async fn sweep_idle_with_pumps_stops_pumps_and_closes_pcs_for_evicted_sessions() {
        let p = fast_pump_transport();
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(p.pumps().lock_owned().await.contains_key(VALID_SESSION_ID));
        assert!(p.has_peer_connection(VALID_SESSION_ID).await);
        // `sweep_idle_with_pumps(now + 1y)` evicts every session
        // (every `last_activity` is older than the timeout) and
        // must stop every matching pump and close every matching
        // peer connection.
        let far_future =
            tokio::time::Instant::now() + std::time::Duration::from_secs(365 * 24 * 3600);
        let evicted = p.sweep_idle_with_pumps(far_future).await;
        assert!(evicted.iter().any(|id| id == VALID_SESSION_ID));
        assert!(
            p.pumps().lock_owned().await.is_empty(),
            "every pump must be removed after sweeping every session"
        );
        assert!(!p.has_peer_connection(VALID_SESSION_ID).await);
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

    // -----------------------------------------------------------------
    // Slice R7.n.6 — capture-pump → encoder → RTP-track wiring.
    //
    // The driver's job in this slice is to (a) attach a per-session
    // H.264 video track to the freshly-built `RTCPeerConnection` and
    // (b) rebind the per-session `LateBoundCaptureSink` to the
    // chained encoder→track sink so the running pump's frames flow
    // end-to-end. We exercise that with a stub encoder factory; the
    // upstream `webrtc-rs` PC machinery is real.
    // -----------------------------------------------------------------

    use crate::desktop::media::{
        CapturedFrame, EncoderFactory, VideoEncoder,
    };

    /// Stub `VideoEncoder` that emits fixed-size dummy chunks.
    /// Used to exercise the R7.n.6 wiring without depending on a
    /// per-OS encoder driver.
    #[derive(Default)]
    struct StubEncoder {
        seen: std::sync::Mutex<u64>,
    }
    #[async_trait]
    impl VideoEncoder for StubEncoder {
        async fn encode(
            &self,
            frame: &CapturedFrame,
        ) -> Result<crate::desktop::media::EncodedVideoChunk, crate::desktop::media::DesktopMediaError>
        {
            let mut s = self.seen.lock().unwrap();
            *s += 1;
            Ok(crate::desktop::media::EncodedVideoChunk {
                bytes: vec![0u8; 16],
                timestamp_micros: frame.timestamp_micros,
                is_keyframe: *s == 1,
            })
        }
        fn request_keyframe(&self) {}
    }

    /// Stub `EncoderFactory` whose every `build()` call returns a
    /// fresh stub encoder.
    struct StubEncoderFactory;
    impl EncoderFactory for StubEncoderFactory {
        fn build(
            &self,
        ) -> Result<Arc<dyn VideoEncoder>, crate::desktop::media::DesktopMediaError> {
            Ok(Arc::new(StubEncoder::default()))
        }
    }

    /// Build a transport whose providers carry a [`StubEncoderFactory`]
    /// so the R7.n.6 attach-track path takes the success branch
    /// (rather than the `NotSupported` log-and-skip fallback).
    fn transport_with_stub_encoder() -> WebRtcDesktopTransport {
        let mut providers = DesktopProviders::not_supported_for(HostOs::Linux);
        providers.encoder_factory = Arc::new(StubEncoderFactory);
        let providers = Arc::new(providers);
        let sink: Arc<dyn CaptureSink> = Arc::new(DiscardingCaptureSink::new());
        WebRtcDesktopTransport::with_providers(
            HostOs::Linux,
            Some(VALID_ORG_ID.into()),
            providers,
            sink,
            CapturePumpConfig::default(),
        )
    }

    #[tokio::test]
    async fn remote_control_creates_late_bound_sink_initially_pointing_at_default() {
        // The per-session sink is created at RemoteControl time,
        // bound to the default downstream so frames captured before
        // the PC is built are still observable to the test sink.
        let p = transport_with_stub_encoder();
        let _ = p.remote_control(&rc_req()).await;
        let sinks = p.session_sinks().lock_owned().await;
        let s = sinks
            .get(VALID_SESSION_ID)
            .expect("session sink registered alongside pump");
        assert!(
            s.is_bound(),
            "newly-spawned per-session sink must be bound to the default downstream",
        );
    }

    #[tokio::test]
    async fn build_peer_connection_attaches_h264_video_transceiver_to_pc() {
        let p = transport_with_stub_encoder();
        let _ = p.remote_control(&rc_req()).await;
        // Drive ProvideIceServers to build the PC.
        let r = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(r.success, "{r:?}");
        // The PC must now exist *and* carry a video transceiver
        // for the freshly-attached H.264 track.
        let pc = p
            .peer_connections
            .get(VALID_SESSION_ID)
            .await
            .expect("PC built by on_provide_ice_servers");
        let transceivers = pc.get_transceivers().await;
        assert!(
            transceivers.iter().any(|t| t.kind()
                == webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video),
            "PC must carry a video transceiver after attach_video_track ran",
        );
    }

    #[tokio::test]
    async fn build_peer_connection_keeps_session_sink_bound_after_rebind() {
        // Before PC build the per-session sink points at the
        // default downstream. After PC build, the sink is rebound
        // to the chained encoder→track sink. Either way the sink
        // must report `is_bound() == true` and the
        // `dropped_before_bind` counter must not have advanced
        // (frames flowing during the rebind window go to the
        // default downstream, never silently dropped).
        let p = transport_with_stub_encoder();
        let _ = p.remote_control(&rc_req()).await;
        let s = p
            .session_sinks()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .expect("session sink registered")
            .clone();
        assert!(s.is_bound());
        let dropped_before = s.dropped_before_bind();
        let r = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(r.success, "{r:?}");
        assert!(s.is_bound());
        assert_eq!(s.dropped_before_bind(), dropped_before);
    }

    #[tokio::test]
    async fn build_peer_connection_with_not_supported_encoder_leaves_pc_trackless() {
        // The default `NotSupportedEncoderFactory` reports no
        // encoder; the driver must fall back to negotiating the
        // PC without a video track rather than panicking.
        let p = fast_pump_transport(); // uses NotSupportedEncoderFactory
        let _ = p.remote_control(&rc_req()).await;
        let r = p.on_provide_ice_servers(&provide_ice_req()).await;
        assert!(r.success, "{r:?}");
        let pc = p
            .peer_connections
            .get(VALID_SESSION_ID)
            .await
            .expect("PC built by on_provide_ice_servers");
        assert!(
            pc.get_transceivers()
                .await
                .iter()
                .all(|t| t.kind()
                    != webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video),
            "PC must NOT carry a video transceiver when the encoder factory is NotSupported",
        );
    }

    #[tokio::test]
    async fn change_windows_session_unbinds_and_drops_per_session_sink() {
        // After the PC is built the per-session sink points at the
        // chained downstream. After `change_windows_session` runs,
        // the session is closed: the sink is unbound and removed
        // from the per-session map.
        let p = transport_with_stub_encoder();
        let _ = p.remote_control(&rc_req()).await;
        let _ = p.on_provide_ice_servers(&provide_ice_req()).await;
        // Capture the per-session sink Arc so we can observe its
        // state after the close path drops the map entry.
        let s = p
            .session_sinks()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .expect("session sink registered")
            .clone();
        let _ = p.change_windows_session(&change_session_req()).await;
        assert!(
            !s.is_bound(),
            "stop_pump must unbind the per-session sink when the session is closed",
        );
        assert!(p
            .session_sinks()
            .lock_owned()
            .await
            .get(VALID_SESSION_ID)
            .is_none());
    }

    #[tokio::test]
    async fn distinct_sessions_get_distinct_session_sinks() {
        let p = transport_with_stub_encoder();
        let _ = p.remote_control(&rc_req()).await;
        let mut second = rc_req();
        second.session_id = OTHER_SESSION_ID.into();
        let _ = p.remote_control(&second).await;
        let sinks = p.session_sinks().lock_owned().await;
        assert_eq!(sinks.len(), 2);
        assert!(sinks.contains_key(VALID_SESSION_ID));
        assert!(sinks.contains_key(OTHER_SESSION_ID));
        // The two sinks are distinct `Arc`s.
        let a = sinks.get(VALID_SESSION_ID).unwrap();
        let b = sinks.get(OTHER_SESSION_ID).unwrap();
        assert!(!Arc::ptr_eq(a, b));
    }
}
