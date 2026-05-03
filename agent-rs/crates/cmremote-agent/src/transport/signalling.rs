// Source: CMRemote, clean-room implementation.

//! Hub-bound implementation of
//! [`cmremote_platform::desktop::SignallingEgress`] (slice R7.n.7).
//!
//! The desktop-transport WebRTC driver lives in `cmremote-platform`
//! and produces SDP answers / ICE candidates as a side effect of
//! handling viewer-bound offers. To deliver those back to the .NET
//! hub, the driver calls [`SignallingEgress`]; this module supplies
//! the production implementation that wraps each call into a
//! SignalR `HubInvocation` (`SendSdpAnswer` / `SendIceCandidate`)
//! and pushes it onto the per-connection outbound channel.
//!
//! ## Per-connection lifecycle
//!
//! The transport reconnect loop creates a fresh `outbound_tx`
//! channel for every new WebSocket session. The driver, however, is
//! constructed once at runtime startup and lives for the entire
//! agent process lifetime. To bridge those two lifetimes,
//! [`HubBoundSignallingEgress`] holds an `RwLock<Option<Binding>>`
//! that the transport loop **rebinds** at the start of each session
//! and **clears** when the session ends. Calls that arrive while no
//! session is bound (e.g. during a reconnect window) are warn-logged
//! and dropped — exactly the failure mode the
//! [`cmremote_platform::desktop::LoggingSignallingEgress`] default
//! produces when no production egress is wired at all.
//!
//! ## Encoding contract
//!
//! Each invocation is rendered using the same SignalR record shape
//! the dispatch layer uses for completions:
//!
//! - JSON: a SignalR record terminated by [`RECORD_SEPARATOR`] (0x1E),
//!   pushed into the channel as [`Message::Text`].
//! - MessagePack: a length-prefixed record (per the SignalR
//!   MessagePack spec), pushed as [`Message::Binary`].
//!
//! The hub method name is sent verbatim (`SendSdpAnswer` /
//! `SendIceCandidate`) so the .NET `AgentHub` can deserialise the
//! arguments array directly into the matching `AgentSdpAnswerDto` /
//! `AgentIceCandidateDto`.
//!
//! ## Security contract
//!
//! - The egress NEVER logs the SDP body or candidate string at any
//!   level above `trace`. Only fixed-length metadata (session id,
//!   viewer connection id, byte counts) is logged at `debug`.
//! - The egress ALWAYS uses `tokio::sync::mpsc::Sender::send` (not
//!   `try_send`) so a momentarily-full channel applies back-pressure
//!   instead of silently dropping signalling traffic. The send is
//!   issued from a spawned task so the driver's caller — typically
//!   inside the WebRTC peer connection's event handler — never
//!   blocks waiting on socket back-pressure.
//! - `bind` / `unbind` are O(1) and contention-free under normal
//!   conditions; no signalling call holds the lock across an
//!   `await`.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use cmremote_platform::desktop::SignallingEgress;
use cmremote_wire::{
    to_msgpack, write_json_record, write_msgpack_record, AgentIceCandidate, AgentSdpAnswer,
    HubInvocation, HubMessageKind, HubProtocol,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

/// Hub method name for an agent → server SDP answer. MUST match the
/// .NET `AgentHub.SendSdpAnswer` method name byte-for-byte.
const TARGET_SEND_SDP_ANSWER: &str = "SendSdpAnswer";

/// Hub method name for an agent → server ICE candidate. MUST match
/// the .NET `AgentHub.SendIceCandidate` method name byte-for-byte.
const TARGET_SEND_ICE_CANDIDATE: &str = "SendIceCandidate";

/// Per-session binding the egress uses to render and dispatch each
/// invocation. The `encoding` is captured at bind time because the
/// .NET hub's selected sub-protocol is fixed for the lifetime of a
/// single WebSocket session (a reconnect can pick a different
/// encoding).
#[derive(Clone)]
struct Binding {
    encoding: HubProtocol,
    sender: mpsc::Sender<Message>,
}

/// Hub-bound [`SignallingEgress`] suitable for sharing across the
/// runtime, the transport loop, and the WebRTC driver.
///
/// Construct one with [`HubBoundSignallingEgress::new`], pass it to
/// [`cmremote_platform::desktop::WebRtcDesktopTransport::with_providers_and_egress`],
/// and rebind it from the transport loop on every new connection.
pub struct HubBoundSignallingEgress {
    binding: RwLock<Option<Binding>>,
}

impl Default for HubBoundSignallingEgress {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HubBoundSignallingEgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bound = self.binding.read().map(|g| g.is_some()).unwrap_or(false);
        f.debug_struct("HubBoundSignallingEgress")
            .field("bound", &bound)
            .finish()
    }
}

impl HubBoundSignallingEgress {
    /// Construct an unbound egress. Calls made before the first
    /// [`bind`](Self::bind) are warn-logged and dropped.
    pub fn new() -> Self {
        Self {
            binding: RwLock::new(None),
        }
    }

    /// Attach the egress to a new per-connection outbound channel
    /// and the matching SignalR sub-protocol. Replaces any previous
    /// binding atomically so the previous channel's buffered
    /// messages are unaffected (they remain owned by the dispatch
    /// task that enqueued them).
    pub fn bind(&self, encoding: HubProtocol, sender: mpsc::Sender<Message>) {
        // Poisoned-lock recovery: a panic in another bind/unbind
        // call shouldn't permanently break signalling. Take the
        // inner guard and overwrite either way.
        let mut slot = match self.binding.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some(Binding { encoding, sender });
    }

    /// Detach the egress; subsequent calls warn-log and drop until
    /// the next [`bind`](Self::bind). Idempotent — calling
    /// `unbind` twice is fine.
    pub fn unbind(&self) {
        let mut slot = match self.binding.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = None;
    }

    /// Snapshot the current binding (if any). Cloning a binding is
    /// cheap — the `Sender` is internally `Arc`-shared and the
    /// `HubProtocol` is `Copy`.
    fn current(&self) -> Option<Binding> {
        match self.binding.read() {
            Ok(g) => g.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// Render a [`HubInvocation`] into the over-the-wire framing for
/// the given encoding. Mirrors the dispatch layer's
/// `encode_completion` exactly so a single set of framing tests
/// covers both directions.
fn encode_invocation(invocation: &HubInvocation, encoding: HubProtocol) -> Message {
    match encoding {
        HubProtocol::Json => {
            let bytes = serde_json::to_vec(invocation).expect("hub invocation JSON serialisation");
            let framed = write_json_record(&bytes);
            Message::Text(String::from_utf8(framed).expect("valid utf-8"))
        }
        HubProtocol::Messagepack => {
            let bytes = to_msgpack(invocation).expect("hub invocation msgpack serialisation");
            let framed = write_msgpack_record(&bytes).expect("msgpack invocation framing");
            Message::Binary(framed)
        }
    }
}

/// Build a fire-and-forget invocation (no `invocationId`) for the
/// named hub `target` carrying a single positional argument
/// `argument`. The agent never awaits a completion for signalling
/// traffic — the .NET hub forwards to the viewer and that's the
/// entire round-trip.
fn build_invocation(target: &str, argument: serde_json::Value) -> HubInvocation {
    HubInvocation {
        kind: HubMessageKind::Invocation as u8,
        invocation_id: None,
        target: target.to_owned(),
        arguments: vec![argument],
    }
}

#[async_trait]
impl SignallingEgress for HubBoundSignallingEgress {
    async fn send_sdp_answer(&self, session_id: &str, viewer_connection_id: &str, sdp: String) {
        let Some(binding) = self.current() else {
            warn!(
                session_id = %session_id,
                viewer_connection_id = %viewer_connection_id,
                sdp_bytes = sdp.len(),
                event = "signalling-egress-sdp-answer-dropped",
                "no hub session bound; dropping SDP answer"
            );
            return;
        };

        let dto = AgentSdpAnswer {
            viewer_connection_id: viewer_connection_id.to_owned(),
            session_id: session_id.to_owned(),
            sdp,
        };
        let argument = match serde_json::to_value(&dto) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    viewer_connection_id = %viewer_connection_id,
                    error = %e,
                    "failed to serialise SDP answer; dropping"
                );
                return;
            }
        };
        let invocation = build_invocation(TARGET_SEND_SDP_ANSWER, argument);
        let msg = encode_invocation(&invocation, binding.encoding);

        debug!(
            session_id = %session_id,
            viewer_connection_id = %viewer_connection_id,
            sdp_bytes = dto.sdp.len(),
            "dispatching SendSdpAnswer to hub"
        );

        // Hop into a task so the WebRTC event handler never blocks
        // on outbound socket back-pressure. `send().await` (not
        // `try_send`) so a momentarily-full channel applies
        // back-pressure rather than silently dropping signalling.
        let session_id_owned = session_id.to_owned();
        let viewer_id_owned = viewer_connection_id.to_owned();
        tokio::spawn(async move {
            if binding.sender.send(msg).await.is_err() {
                warn!(
                    session_id = %session_id_owned,
                    viewer_connection_id = %viewer_id_owned,
                    "outbound channel closed before SDP answer was sent; dropping"
                );
            }
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
        let Some(binding) = self.current() else {
            warn!(
                session_id = %session_id,
                viewer_connection_id = %viewer_connection_id,
                candidate_bytes = candidate.len(),
                event = "signalling-egress-ice-candidate-dropped",
                "no hub session bound; dropping ICE candidate"
            );
            return;
        };

        let dto = AgentIceCandidate {
            viewer_connection_id: viewer_connection_id.to_owned(),
            session_id: session_id.to_owned(),
            candidate,
            sdp_mid,
            sdp_mline_index,
        };
        let argument = match serde_json::to_value(&dto) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    viewer_connection_id = %viewer_connection_id,
                    error = %e,
                    "failed to serialise ICE candidate; dropping"
                );
                return;
            }
        };
        let invocation = build_invocation(TARGET_SEND_ICE_CANDIDATE, argument);
        let msg = encode_invocation(&invocation, binding.encoding);

        debug!(
            session_id = %session_id,
            viewer_connection_id = %viewer_connection_id,
            candidate_bytes = dto.candidate.len(),
            "dispatching SendIceCandidate to hub"
        );

        let session_id_owned = session_id.to_owned();
        let viewer_id_owned = viewer_connection_id.to_owned();
        tokio::spawn(async move {
            if binding.sender.send(msg).await.is_err() {
                warn!(
                    session_id = %session_id_owned,
                    viewer_connection_id = %viewer_id_owned,
                    "outbound channel closed before ICE candidate was sent; dropping"
                );
            }
        });
    }
}

/// Convenience: an unbound egress wrapped in an `Arc`, ready for
/// injection into the runtime. The transport loop later attaches a
/// per-connection sender via [`HubBoundSignallingEgress::bind`].
pub fn shared() -> Arc<HubBoundSignallingEgress> {
    Arc::new(HubBoundSignallingEgress::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    use cmremote_wire::{decode_envelope_with, HubEnvelope, JsonFrameReader, MsgPackFrameReader};

    /// Pull every queued frame out of the receiver, decode it, and
    /// assert exactly one invocation matching the predicate is
    /// present. Spins for up to ~500 ms because the egress dispatches
    /// from a tokio task.
    async fn drain_one_invocation(
        rx: &mut mpsc::Receiver<Message>,
        encoding: HubProtocol,
    ) -> HubInvocation {
        // Wait briefly for the spawned task to enqueue the message.
        let msg = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .expect("egress did not produce an outbound frame within 500ms")
            .expect("outbound channel closed unexpectedly");

        let bytes = match (encoding, msg) {
            (HubProtocol::Json, Message::Text(s)) => s.into_bytes(),
            (HubProtocol::Messagepack, Message::Binary(b)) => b,
            (e, other) => panic!("unexpected message kind for {e:?}: {other:?}"),
        };

        // The framing readers tolerate one or many records per push.
        let envelope = match encoding {
            HubProtocol::Json => {
                let mut reader = JsonFrameReader::new();
                reader.push(&bytes).expect("valid framing");
                let record = reader.next_record().expect("one record");
                decode_envelope_with(&record, encoding).expect("valid envelope")
            }
            HubProtocol::Messagepack => {
                let mut reader = MsgPackFrameReader::new();
                reader.push(&bytes).expect("valid framing");
                let record = reader.next_record().expect("one record");
                decode_envelope_with(&record, encoding).expect("valid envelope")
            }
        };

        match envelope {
            HubEnvelope::Invocation(inv) => inv,
            other => panic!("expected invocation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unbound_egress_drops_sdp_answer_without_panicking() {
        let egress = HubBoundSignallingEgress::new();
        // Should be a no-op (warn log only). No panic, no send.
        egress
            .send_sdp_answer("sid", "viewer", "v=0\r\n".into())
            .await;
        // Re-binding and re-checking after-the-fact: the previous call
        // is genuinely lost (no replay), proving the unbind path is
        // drop-not-buffer.
        let (tx, mut rx) = mpsc::channel::<Message>(8);
        egress.bind(HubProtocol::Json, tx);
        // Channel must be empty.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn bound_egress_emits_send_sdp_answer_invocation_json() {
        let egress = HubBoundSignallingEgress::new();
        let (tx, mut rx) = mpsc::channel::<Message>(8);
        egress.bind(HubProtocol::Json, tx);

        egress
            .send_sdp_answer(
                "11111111-2222-3333-4444-555555555555",
                "viewer-7",
                "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n".into(),
            )
            .await;

        let inv = drain_one_invocation(&mut rx, HubProtocol::Json).await;
        assert_eq!(inv.target, "SendSdpAnswer");
        assert!(inv.invocation_id.is_none(), "fire-and-forget invocation");
        assert_eq!(inv.arguments.len(), 1);
        let dto: AgentSdpAnswer =
            serde_json::from_value(inv.arguments[0].clone()).expect("AgentSdpAnswer round-trip");
        assert_eq!(dto.session_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(dto.viewer_connection_id, "viewer-7");
        assert!(dto.sdp.starts_with("v=0\r\n"));
    }

    #[tokio::test]
    async fn bound_egress_emits_send_sdp_answer_invocation_msgpack() {
        let egress = HubBoundSignallingEgress::new();
        let (tx, mut rx) = mpsc::channel::<Message>(8);
        egress.bind(HubProtocol::Messagepack, tx);

        egress
            .send_sdp_answer("sid", "viewer", "v=0\r\n".into())
            .await;

        let inv = drain_one_invocation(&mut rx, HubProtocol::Messagepack).await;
        assert_eq!(inv.target, "SendSdpAnswer");
        assert_eq!(inv.arguments.len(), 1);
        let dto: AgentSdpAnswer =
            serde_json::from_value(inv.arguments[0].clone()).expect("AgentSdpAnswer round-trip");
        assert_eq!(dto.session_id, "sid");
    }

    #[tokio::test]
    async fn bound_egress_emits_send_ice_candidate_with_optional_fields() {
        let egress = HubBoundSignallingEgress::new();
        let (tx, mut rx) = mpsc::channel::<Message>(8);
        egress.bind(HubProtocol::Json, tx);

        egress
            .send_ice_candidate(
                "sid",
                "viewer",
                "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host".into(),
                Some("0".into()),
                Some(0),
            )
            .await;

        let inv = drain_one_invocation(&mut rx, HubProtocol::Json).await;
        assert_eq!(inv.target, "SendIceCandidate");
        let dto: AgentIceCandidate =
            serde_json::from_value(inv.arguments[0].clone()).expect("AgentIceCandidate round-trip");
        assert_eq!(dto.viewer_connection_id, "viewer");
        assert_eq!(dto.sdp_mid.as_deref(), Some("0"));
        assert_eq!(dto.sdp_mline_index, Some(0));
        assert!(dto.candidate.starts_with("candidate:"));
    }

    #[tokio::test]
    async fn bound_egress_preserves_end_of_candidates_marker() {
        let egress = HubBoundSignallingEgress::new();
        let (tx, mut rx) = mpsc::channel::<Message>(8);
        egress.bind(HubProtocol::Json, tx);

        egress
            .send_ice_candidate("sid", "viewer", String::new(), None, None)
            .await;

        let inv = drain_one_invocation(&mut rx, HubProtocol::Json).await;
        let dto: AgentIceCandidate = serde_json::from_value(inv.arguments[0].clone()).unwrap();
        assert_eq!(dto.candidate, "");
        assert!(dto.sdp_mid.is_none());
        assert!(dto.sdp_mline_index.is_none());
    }

    #[tokio::test]
    async fn rebinding_swaps_outbound_channel_atomically() {
        let egress = HubBoundSignallingEgress::new();
        let (tx_old, mut rx_old) = mpsc::channel::<Message>(8);
        let (tx_new, mut rx_new) = mpsc::channel::<Message>(8);

        egress.bind(HubProtocol::Json, tx_old);
        egress.bind(HubProtocol::Json, tx_new);

        egress
            .send_sdp_answer("sid", "viewer", "v=0\r\n".into())
            .await;

        // Old channel: rebind dropped its sender, so the only thing
        // `recv` may yield is `None` (channel closed). It must not
        // observe an actual `Some(_)` payload — that would mean the
        // egress dispatched to the previous binding.
        let old = tokio::time::timeout(std::time::Duration::from_millis(100), rx_old.recv()).await;
        match old {
            Ok(None) | Err(_) => {} // closed or timed out — both fine
            Ok(Some(msg)) => panic!("old binding leaked a message after rebind: {msg:?}"),
        }
        // New channel receives the answer.
        let _ = drain_one_invocation(&mut rx_new, HubProtocol::Json).await;
    }

    #[tokio::test]
    async fn unbind_drops_subsequent_messages() {
        let egress = HubBoundSignallingEgress::new();
        let (tx, mut rx) = mpsc::channel::<Message>(8);
        egress.bind(HubProtocol::Json, tx);
        egress.unbind();

        egress
            .send_sdp_answer("sid", "viewer", "v=0\r\n".into())
            .await;
        // After unbind, the egress drops the held sender. `recv` may
        // return `None` (channel closed) or time out, but must NOT
        // return `Some(_)`.
        let res = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        match res {
            Ok(None) | Err(_) => {}
            Ok(Some(msg)) => panic!("unbind did not drop the sender: {msg:?}"),
        }
    }
}
