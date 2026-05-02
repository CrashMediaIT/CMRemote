// Source: CMRemote, clean-room implementation.

//! Peer-connection bookkeeping for the slice R7.m WebRTC driver.
//!
//! This module lives behind the `webrtc-driver` feature and is the
//! single point of contact between [`super::webrtc::WebRtcDesktopTransport`]
//! and the upstream `webrtc` crate (resolved through the
//! `[patch.crates-io]` pin to `CrashMediaIT/webrtc-cmremote` at tag
//! `v0.17.0-cmremote.1`, per ADR 0001 Option B).
//!
//! ## Why a separate module
//!
//! Keeping every `webrtc::*` import here lets the call-site in
//! `webrtc.rs` stay readable and lets reviewers audit the boundary
//! between our state machine and the upstream stack in one file.
//!
//! ## Lifecycle
//!
//! 1. [`PeerConnectionFactory::new`] builds one shared
//!    `webrtc::api::API` with `register_default_codecs`. The
//!    `MediaEngine` is registered once and shared across every
//!    per-session peer connection — same pattern the upstream
//!    `examples/` directory uses.
//! 2. For each `RemoteControl` the driver calls
//!    [`PeerConnectionFactory::create`] with the per-session
//!    [`webrtc::peer_connection::configuration::RTCConfiguration`].
//!    The factory installs the `on_ice_candidate` and
//!    `on_peer_connection_state_change` handlers that fan
//!    locally-trickled candidates and connection-state transitions
//!    out via the [`super::SignallingEgress`] / state-machine
//!    callbacks the driver provides.
//! 3. [`PeerConnectionRegistry`] tracks the live `Arc<RTCPeerConnection>`
//!    per canonical-UUID `session_id`. Replace-on-duplicate /
//!    explicit close / sweep-idle close the prior PC before
//!    dropping it from the map.

use std::sync::Arc;

use cmremote_wire::{
    IceCandidate as WireIceCandidate, IceCredentialType, IceServer as WireIceServer,
    IceServerConfig as WireIceServerConfig, IceTransportPolicy as WireIceTransportPolicy,
};
use tokio::sync::Mutex;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::{APIBuilder, API};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;
use webrtc::peer_connection::RTCPeerConnection;

/// Shared `webrtc::api::API` plus the bookkeeping every per-session
/// peer connection needs. Cheap to clone — the inner `Arc` is what
/// [`super::webrtc::WebRtcDesktopTransport`] stashes.
pub(super) struct PeerConnectionFactory {
    api: API,
}

impl PeerConnectionFactory {
    /// Build a fresh factory. The `MediaEngine` is initialised with
    /// the upstream default codec set (H.264 + Opus + VP8 + VP9 + …)
    /// so the agent's outbound video track can negotiate against
    /// every browser the .NET viewer ships in. Returns an error if
    /// `register_default_codecs` fails — in practice this only
    /// happens when the build pulled in an inconsistent codec set,
    /// which we want to surface at startup rather than on the first
    /// `RemoteControl`.
    pub(super) fn new() -> Result<Self, webrtc::Error> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;
        let api = APIBuilder::new().with_media_engine(media_engine).build();
        Ok(Self { api })
    }

    /// Create a new peer connection with `configuration`. The
    /// returned `Arc<RTCPeerConnection>` is shared with the
    /// driver's per-session map; the driver owns installing the
    /// `on_ice_candidate` / `on_peer_connection_state_change`
    /// handlers (which live in `webrtc.rs` so they can capture the
    /// per-session `Arc<dyn SignallingEgress>` and registry handle
    /// without a second indirection).
    pub(super) async fn create(
        &self,
        configuration: RTCConfiguration,
    ) -> Result<Arc<RTCPeerConnection>, webrtc::Error> {
        let pc = self.api.new_peer_connection(configuration).await?;
        Ok(Arc::new(pc))
    }
}

/// Per-session map of live `Arc<RTCPeerConnection>` instances,
/// keyed by canonical-UUID `session_id`. Mirrors the existing
/// `pumps` map in `webrtc.rs` so a single guard discipline (lock,
/// mutate, drop, await) keeps both side-tables consistent.
#[derive(Default)]
pub(super) struct PeerConnectionRegistry {
    inner: Mutex<std::collections::HashMap<String, Arc<RTCPeerConnection>>>,
}

impl PeerConnectionRegistry {
    /// Build an empty registry.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Insert `pc` for `session_id`, returning the prior PC (if any)
    /// so the caller can `close()` it on the same task.
    pub(super) async fn insert(
        &self,
        session_id: &str,
        pc: Arc<RTCPeerConnection>,
    ) -> Option<Arc<RTCPeerConnection>> {
        self.inner.lock().await.insert(session_id.to_string(), pc)
    }

    /// Remove (and return) the peer connection for `session_id`, if
    /// any. Caller is responsible for `close()`-ing the returned
    /// handle — this method does not await any I/O so it is safe to
    /// call while holding upstream locks.
    pub(super) async fn remove(&self, session_id: &str) -> Option<Arc<RTCPeerConnection>> {
        self.inner.lock().await.remove(session_id)
    }

    /// Borrow (clone the `Arc` for) the peer connection registered
    /// under `session_id`, if any.
    pub(super) async fn get(&self, session_id: &str) -> Option<Arc<RTCPeerConnection>> {
        self.inner.lock().await.get(session_id).cloned()
    }

    /// `true` when no PC is registered.
    #[cfg(test)]
    pub(super) async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }

    /// Number of live peer connections in the registry.
    #[cfg(test)]
    pub(super) async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

/// Translate a wire-protocol [`WireIceServerConfig`] into the
/// `webrtc-rs` [`RTCConfiguration`] shape the API expects.
///
/// **Security**: the wire layer's [`super::guards::check_provide_ice_servers`]
/// already enforces every length / scheme / control-character
/// constraint on the input config. This translator therefore only
/// performs lossless field copies — it never re-validates and never
/// silently drops fields. If a hostile config slips past the guard
/// (it should not — the guard is the only entry point in the
/// driver), the upstream stack will surface the violation at
/// `set_remote_description` / `add_ice_candidate` time.
pub(super) fn translate_ice_config(wire: &WireIceServerConfig) -> RTCConfiguration {
    RTCConfiguration {
        ice_servers: wire.ice_servers.iter().map(translate_ice_server).collect(),
        ice_transport_policy: translate_ice_transport_policy(wire.ice_transport_policy),
        ..Default::default()
    }
}

/// Translate one wire [`WireIceServer`] into an [`RTCIceServer`].
///
/// `webrtc-rs` 0.17's [`RTCIceServer`] models the W3C
/// `RTCIceServer` shape with `username` and `credential` as plain
/// `String` (empty when absent) rather than `Option<String>`. We
/// pass the empty string through when the wire field is absent so
/// the upstream stack treats the server as un-authenticated (the
/// only behaviour distinguishable on the wire anyway). The
/// `credential_type` enum is intentionally **not** carried into the
/// `webrtc-rs` shape — the upstream stack only supports the
/// password flow, and the wire-layer guard refuses every other
/// variant before it reaches us.
fn translate_ice_server(wire: &WireIceServer) -> RTCIceServer {
    // Defence-in-depth: even though the guard refuses anything other
    // than `Password`, we never forward credentials when the wire
    // type says otherwise. This keeps a future wire-layer
    // regression from leaking a non-password credential into the
    // ICE agent.
    let pass_credentials = matches!(wire.credential_type, IceCredentialType::Password);
    RTCIceServer {
        urls: wire.urls.clone(),
        username: if pass_credentials {
            wire.username.clone().unwrap_or_default()
        } else {
            String::new()
        },
        credential: if pass_credentials {
            wire.credential.clone().unwrap_or_default()
        } else {
            String::new()
        },
    }
}

/// Translate the wire transport policy into the `webrtc-rs` enum.
fn translate_ice_transport_policy(wire: WireIceTransportPolicy) -> RTCIceTransportPolicy {
    match wire {
        WireIceTransportPolicy::All => RTCIceTransportPolicy::All,
        WireIceTransportPolicy::Relay => RTCIceTransportPolicy::Relay,
    }
}

/// Translate a wire-protocol [`WireIceCandidate`] into the
/// `webrtc-rs` [`RTCIceCandidateInit`] shape the API expects.
///
/// The wire DTO carries the SDP `a=candidate:` line in `candidate`
/// (without the `a=` prefix, mirroring the W3C JSON shape) plus
/// optional `sdp_mid` / `sdp_mline_index` mirroring the W3C
/// `RTCIceCandidate` JSON. We forward those fields verbatim. The
/// wire-layer guard already capped `candidate.len()` at
/// [`cmremote_wire::MAX_SIGNALLING_STRING_LEN`] and refused
/// embedded NULs.
///
/// The wire `sdp_mline_index` is `Option<u16>` — same width as the
/// `webrtc-rs` field — so no truncation can occur.
pub(super) fn translate_ice_candidate(wire: &WireIceCandidate) -> RTCIceCandidateInit {
    RTCIceCandidateInit {
        candidate: wire.candidate.clone(),
        sdp_mid: wire.sdp_mid.clone(),
        sdp_mline_index: wire.sdp_mline_index,
        username_fragment: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_server_config() -> WireIceServerConfig {
        WireIceServerConfig {
            ice_servers: vec![WireIceServer {
                urls: vec!["stun:stun.example.org:3478".into()],
                username: None,
                credential: None,
                credential_type: IceCredentialType::Password,
            }],
            ice_transport_policy: WireIceTransportPolicy::All,
        }
    }

    #[test]
    fn translate_ice_config_copies_servers_and_policy() {
        let wire = one_server_config();
        let cfg = translate_ice_config(&wire);
        assert_eq!(cfg.ice_servers.len(), 1);
        assert_eq!(cfg.ice_servers[0].urls, vec!["stun:stun.example.org:3478"]);
        assert!(cfg.ice_servers[0].username.is_empty());
        assert!(cfg.ice_servers[0].credential.is_empty());
        assert_eq!(cfg.ice_transport_policy, RTCIceTransportPolicy::All);
    }

    #[test]
    fn translate_ice_config_relay_policy_is_carried_through() {
        let mut wire = one_server_config();
        wire.ice_transport_policy = WireIceTransportPolicy::Relay;
        let cfg = translate_ice_config(&wire);
        assert_eq!(cfg.ice_transport_policy, RTCIceTransportPolicy::Relay);
    }

    #[test]
    fn translate_ice_config_passes_password_credentials() {
        let mut wire = one_server_config();
        wire.ice_servers[0].urls = vec!["turn:turn.example.org:3478".into()];
        wire.ice_servers[0].username = Some("alice".into());
        wire.ice_servers[0].credential = Some("hunter2".into());
        wire.ice_servers[0].credential_type = IceCredentialType::Password;
        let cfg = translate_ice_config(&wire);
        assert_eq!(cfg.ice_servers[0].username, "alice");
        assert_eq!(cfg.ice_servers[0].credential, "hunter2");
    }

    #[test]
    fn translate_ice_config_drops_non_password_credentials_defence_in_depth() {
        // The wire guard already refuses Oauth credentials; the
        // translator MUST also drop them, so a future guard
        // regression cannot leak a non-password credential into
        // the ICE agent.
        let mut wire = one_server_config();
        wire.ice_servers[0].username = Some("alice".into());
        wire.ice_servers[0].credential = Some("oauth-token".into());
        wire.ice_servers[0].credential_type = IceCredentialType::Oauth;
        let cfg = translate_ice_config(&wire);
        assert!(cfg.ice_servers[0].username.is_empty());
        assert!(cfg.ice_servers[0].credential.is_empty());
    }

    #[test]
    fn translate_ice_candidate_carries_every_field_verbatim() {
        let wire = WireIceCandidate {
            viewer_connection_id: "viewer-1".into(),
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            requester_name: "Alice".into(),
            org_name: "Acme".into(),
            org_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        let init = translate_ice_candidate(&wire);
        assert_eq!(
            init.candidate,
            "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host"
        );
        assert_eq!(init.sdp_mid.as_deref(), Some("0"));
        assert_eq!(init.sdp_mline_index, Some(0u16));
        assert!(init.username_fragment.is_none());
    }

    #[tokio::test]
    async fn factory_builds_and_creates_a_peer_connection() {
        // The factory has to register every default codec; if any
        // of them fail to register, `new` returns an error and the
        // driver refuses to start. This test pins that the codec
        // set the build pulls in is consistent.
        let factory = PeerConnectionFactory::new().expect("media engine init");
        let cfg = translate_ice_config(&one_server_config());
        let pc = factory.create(cfg).await.expect("peer connection");
        // PC must be in the initial state.
        assert_eq!(
            pc.connection_state(),
            webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::New,
        );
        // Close immediately so the test does not leak the ICE
        // agent's UDP sockets.
        pc.close().await.expect("close");
    }

    #[tokio::test]
    async fn registry_insert_remove_round_trips() {
        let factory = PeerConnectionFactory::new().expect("media engine init");
        let registry = PeerConnectionRegistry::new();
        let pc = factory
            .create(translate_ice_config(&one_server_config()))
            .await
            .expect("pc");
        assert!(registry.is_empty().await);
        let prior = registry.insert("sid", pc.clone()).await;
        assert!(prior.is_none());
        assert_eq!(registry.len().await, 1);
        let got = registry.get("sid").await.expect("present");
        assert!(Arc::ptr_eq(&got, &pc));
        let removed = registry.remove("sid").await.expect("present");
        assert!(Arc::ptr_eq(&removed, &pc));
        assert!(registry.is_empty().await);
        pc.close().await.expect("close");
    }

    #[tokio::test]
    async fn registry_insert_replaces_prior_entry() {
        let factory = PeerConnectionFactory::new().expect("media engine init");
        let registry = PeerConnectionRegistry::new();
        let pc1 = factory
            .create(translate_ice_config(&one_server_config()))
            .await
            .expect("pc1");
        let pc2 = factory
            .create(translate_ice_config(&one_server_config()))
            .await
            .expect("pc2");
        assert!(registry.insert("sid", pc1.clone()).await.is_none());
        let prior = registry.insert("sid", pc2.clone()).await.expect("prior");
        assert!(Arc::ptr_eq(&prior, &pc1));
        assert_eq!(registry.len().await, 1);
        pc1.close().await.expect("close pc1");
        pc2.close().await.expect("close pc2");
    }
}
