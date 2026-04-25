// Source: CMRemote, clean-room implementation.

//! Outbound-signalling seam for the desktop-transport WebRTC driver
//! (slice R7.m).
//!
//! The agent receives `SendSdpOffer` / `SendIceCandidate` /
//! `ProvideIceServers` from the .NET hub via `IAgentHubClient`. The
//! WebRTC driver (slice R7.m, this crate) then needs to push back
//! the matching SDP answer and any locally-trickled ICE candidates
//! through the **server-bound** half of the hub
//! (`IAgentHub::SendSdpAnswer` / `SendIceCandidate`). That outbound
//! invocation lives in `cmremote-agent` (it owns the hub
//! connection); the driver lives in `cmremote-platform` and must not
//! pull in the hub-protocol crate.
//!
//! [`SignallingEgress`] is the crate-spanning seam: an async trait
//! the driver depends on, with a default [`LoggingSignallingEgress`]
//! implementation that just emits a `tracing::warn!` event so a
//! build with the driver feature on but no real egress wired (e.g.
//! a test harness or an early integration build) surfaces every
//! answer / candidate in the audit log instead of silently dropping
//! it. The agent runtime supplies the real server-bound
//! implementation in a follow-up slice.

use async_trait::async_trait;

/// Async trait the WebRTC driver calls when it needs to deliver a
/// locally-produced SDP answer or ICE candidate back to the .NET
/// hub. Implementations are responsible for invoking the matching
/// server-bound hub method (`SendSdpAnswer` / `SendIceCandidate`).
///
/// Implementations MUST be `Send + Sync` so the driver can stash one
/// behind an `Arc<dyn SignallingEgress>` shared across every
/// per-session peer-connection event handler.
///
/// Implementations MUST NOT log or echo any sensitive payload (SDP
/// fingerprints, ICE credentials) at any level above `debug`. The
/// driver passes the wire shapes verbatim; redaction is the
/// implementation's responsibility.
#[async_trait]
pub trait SignallingEgress: Send + Sync {
    /// Deliver an agent-produced SDP answer to the viewer named by
    /// `viewer_connection_id`, scoped to the session named by
    /// `session_id`. `sdp` is the raw SDP text the WebRTC stack
    /// emitted; the implementation forwards it unchanged.
    async fn send_sdp_answer(&self, session_id: &str, viewer_connection_id: &str, sdp: String);

    /// Deliver an agent-produced ICE candidate to the viewer named by
    /// `viewer_connection_id`, scoped to the session named by
    /// `session_id`. `candidate` is the SDP `a=candidate:` line
    /// (without the `a=` prefix) the WebRTC stack emitted; the
    /// `sdp_mid` and `sdp_mline_index` fields mirror the W3C
    /// `RTCIceCandidate` shape and are passed through verbatim.
    async fn send_ice_candidate(
        &self,
        session_id: &str,
        viewer_connection_id: &str,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    );
}

/// Default [`SignallingEgress`] used when no real hub-bound egress
/// has been wired. Emits a structured `tracing::warn!` event for
/// every outbound answer / candidate so the audit log captures the
/// drop, then returns. Production builds replace this with a
/// concrete implementation that invokes the server-bound hub
/// methods.
///
/// Used by `cmremote-platform`'s default constructors and by the
/// agent runtime as a transitional default — keeps the driver
/// observable without coupling `cmremote-platform` to the hub
/// protocol crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoggingSignallingEgress;

#[async_trait]
impl SignallingEgress for LoggingSignallingEgress {
    async fn send_sdp_answer(&self, session_id: &str, viewer_connection_id: &str, sdp: String) {
        tracing::warn!(
            session_id = %session_id,
            viewer_connection_id = %viewer_connection_id,
            sdp_bytes = sdp.len(),
            event = "signalling-egress-sdp-answer-dropped",
            "no SignallingEgress wired; dropping SDP answer (length only logged)",
        );
    }

    async fn send_ice_candidate(
        &self,
        session_id: &str,
        viewer_connection_id: &str,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) {
        tracing::warn!(
            session_id = %session_id,
            viewer_connection_id = %viewer_connection_id,
            candidate_bytes = candidate.len(),
            sdp_mid = sdp_mid.as_deref().unwrap_or(""),
            sdp_mline_index = sdp_mline_index.unwrap_or(u16::MAX),
            event = "signalling-egress-ice-candidate-dropped",
            "no SignallingEgress wired; dropping ICE candidate (length only logged)",
        );
    }
}

/// Test-only [`SignallingEgress`] that captures every outbound
/// message in an in-memory buffer. Lives behind `cfg(test)` so it
/// does not bloat the production binary; tests in this crate can
/// construct one without an extra dependency.
#[cfg(test)]
pub mod testing {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use super::SignallingEgress;

    /// Single captured outbound message — either an SDP answer or an
    /// ICE candidate — recorded by [`CapturingSignallingEgress`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CapturedSignal {
        /// `send_sdp_answer` invocation.
        SdpAnswer {
            /// Canonical-UUID `session_id` the driver passed.
            session_id: String,
            /// `viewer_connection_id` the driver passed.
            viewer_connection_id: String,
            /// SDP body the driver emitted (verbatim).
            sdp: String,
        },
        /// `send_ice_candidate` invocation.
        IceCandidate {
            /// Canonical-UUID `session_id` the driver passed.
            session_id: String,
            /// `viewer_connection_id` the driver passed.
            viewer_connection_id: String,
            /// `a=candidate:` line (no `a=` prefix).
            candidate: String,
            /// `sdpMid` mirror.
            sdp_mid: Option<String>,
            /// `sdpMLineIndex` mirror.
            sdp_mline_index: Option<u16>,
        },
    }

    /// In-memory [`SignallingEgress`] used by the `webrtc.rs` tests
    /// to assert the driver actually produces an answer / candidate.
    /// Cheap to clone — the inner buffer is shared via an `Arc`.
    #[derive(Debug, Default, Clone)]
    pub struct CapturingSignallingEgress {
        captured: Arc<Mutex<Vec<CapturedSignal>>>,
    }

    impl CapturingSignallingEgress {
        /// Build a fresh capturing egress with an empty buffer.
        pub fn new() -> Self {
            Self::default()
        }

        /// Snapshot of every captured message in arrival order.
        /// Cheap — the result is a clone of the buffer, not a borrow.
        pub async fn captured(&self) -> Vec<CapturedSignal> {
            self.captured.lock().await.clone()
        }

        /// Number of captured messages so far.
        pub async fn len(&self) -> usize {
            self.captured.lock().await.len()
        }

        /// `true` when no messages have been captured.
        pub async fn is_empty(&self) -> bool {
            self.captured.lock().await.is_empty()
        }
    }

    #[async_trait]
    impl SignallingEgress for CapturingSignallingEgress {
        async fn send_sdp_answer(&self, session_id: &str, viewer_connection_id: &str, sdp: String) {
            self.captured.lock().await.push(CapturedSignal::SdpAnswer {
                session_id: session_id.to_string(),
                viewer_connection_id: viewer_connection_id.to_string(),
                sdp,
            });
        }

        async fn send_ice_candidate(
            &self,
            session_id: &str,
            viewer_connection_id: &str,
            candidate: String,
            sdp_mid: Option<String>,
            sdp_mline_index: Option<u16>,
        ) {
            self.captured
                .lock()
                .await
                .push(CapturedSignal::IceCandidate {
                    session_id: session_id.to_string(),
                    viewer_connection_id: viewer_connection_id.to_string(),
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                });
        }
    }
}
