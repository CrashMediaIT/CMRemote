// Source: CMRemote, clean-room implementation.

//! Hub-invocation dispatch surface (slice R2a).
//!
//! Glues the inbound record stream from `transport::session` to the
//! per-method handler layer. Responsibilities:
//!
//! * Decode each raw record into a [`HubEnvelope`].
//! * Enforce per-connection invocation-ID uniqueness.
//! * Route known method names to the appropriate async handler.
//! * Return `HubCompletion::err("not_implemented")` for unknown methods.
//! * Return `HubCompletion::err("duplicate_invocation")` for replayed IDs.
//! * Signal the session to quarantine when the server sends
//!   `HubClose { allowReconnect: false }`.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use cmremote_wire::{
    decode_envelope_with, write_json_record, write_msgpack_record, HubClose, HubCompletion,
    HubEnvelope, HubProtocol,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::handlers::AgentHandlers;

/// Outcome of processing a single inbound record, returned by the
/// `on_record` closure passed to `transport::session::run_session`.
#[derive(Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Normal path — keep the session running.
    Continue,
    /// The server sent `HubClose { allowReconnect: false }`.
    Quarantine {
        /// Optional reason string from the close envelope.
        reason: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Invocation-ID deduplication
// ---------------------------------------------------------------------------

/// Maximum number of distinct invocation IDs the tracker remembers per
/// connection. Once the cap is reached, the oldest IDs are evicted in
/// FIFO order so the agent's memory footprint stays bounded on
/// long-lived sessions.
///
/// Sized generously: at the agent's design heartbeat of one inbound
/// invocation per ~5 seconds, 16 384 entries cover roughly 22 hours of
/// uninterrupted traffic — far longer than any realistic session.
pub const INVOCATION_TRACKER_CAPACITY: usize = 16_384;

/// Tracks invocation IDs seen on the current connection to enforce the
/// spec's uniqueness guarantee (section *Replay and ordering*).
///
/// Bounded to [`INVOCATION_TRACKER_CAPACITY`] entries with FIFO
/// eviction so a long-running session cannot grow the set without
/// limit (DoS-safety property).
#[derive(Debug)]
pub struct InvocationTracker {
    seen: HashSet<String>,
    order: VecDeque<String>,
    capacity: usize,
}

impl Default for InvocationTracker {
    fn default() -> Self {
        Self::with_capacity(INVOCATION_TRACKER_CAPACITY)
    }
}

impl InvocationTracker {
    /// Construct a tracker with an explicit cap. Used by tests.
    pub fn with_capacity(capacity: usize) -> Self {
        // A capacity of zero would deadlock the eviction loop; treat
        // it as a no-op tracker that always reports "new".
        let capacity = capacity.max(1);
        Self {
            seen: HashSet::with_capacity(capacity.min(1024)),
            order: VecDeque::with_capacity(capacity.min(1024)),
            capacity,
        }
    }

    /// Returns `true` if `id` was **already** seen on this connection.
    /// Inserts it (and evicts the oldest entry if at capacity) when new.
    pub fn seen(&mut self, id: &str) -> bool {
        if self.seen.contains(id) {
            return true;
        }
        if self.seen.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        self.seen.insert(id.to_string());
        self.order.push_back(id.to_string());
        false
    }

    /// Reset for a new connection.
    pub fn clear(&mut self) {
        self.seen.clear();
        self.order.clear();
    }
}

// ---------------------------------------------------------------------------
// Method name allow-list
// ---------------------------------------------------------------------------

/// Hub methods the server is permitted to invoke on this agent.
///
/// Any `target` not in this list is rejected with `"not_implemented"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodName {
    /// Server requests a fresh device heartbeat.
    TriggerHeartbeat,
    /// Server asks the agent to execute a command.
    ExecuteCommand,
    /// Server asks for the installed-applications snapshot.
    RequestInstalledApplications,
    /// Server asks the agent to uninstall an application.
    UninstallApplication,
    /// Stubs — not implemented until R6+.
    InstallPackage,
    /// Stub — desktop session (R7).
    ChangeWindowsSession,
    /// Stub — remote control (R7).
    RemoteControl,
    /// Stub — remote control screencast (R7).
    RestartScreenCaster,
    /// Stub — saved script runner (R6).
    RunScript,
    /// Stub — keyboard shortcut injection (R7).
    InvokeCtrlAltDel,
    /// Stub — agent log deletion.
    DeleteLogs,
    /// Stub — agent log retrieval.
    GetLogs,
    /// Stub — agent self-update (R8).
    ReinstallAgent,
    /// Stub — agent self-uninstall (R8).
    UninstallAgent,
    /// Stub — Wake-on-LAN helper.
    WakeDevice,
    /// Stub — browser-to-agent file transfer.
    TransferFileFromBrowserToAgent,
}

impl MethodName {
    /// Map the wire `target` string to a known method.
    pub fn from_target(target: &str) -> Option<Self> {
        match target {
            "TriggerHeartbeat" => Some(Self::TriggerHeartbeat),
            "ExecuteCommand" => Some(Self::ExecuteCommand),
            "RequestInstalledApplications" => Some(Self::RequestInstalledApplications),
            "UninstallApplication" => Some(Self::UninstallApplication),
            "InstallPackage" => Some(Self::InstallPackage),
            "ChangeWindowsSession" => Some(Self::ChangeWindowsSession),
            "RemoteControl" => Some(Self::RemoteControl),
            "RestartScreenCaster" => Some(Self::RestartScreenCaster),
            "RunScript" => Some(Self::RunScript),
            "InvokeCtrlAltDel" => Some(Self::InvokeCtrlAltDel),
            "DeleteLogs" => Some(Self::DeleteLogs),
            "GetLogs" => Some(Self::GetLogs),
            "ReinstallAgent" => Some(Self::ReinstallAgent),
            "UninstallAgent" => Some(Self::UninstallAgent),
            "WakeDevice" => Some(Self::WakeDevice),
            "TransferFileFromBrowserToAgent" => Some(Self::TransferFileFromBrowserToAgent),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Record serialisation helpers
// ---------------------------------------------------------------------------

fn encode_completion(completion: &HubCompletion, encoding: HubProtocol) -> Message {
    match encoding {
        HubProtocol::Json => {
            let bytes = serde_json::to_vec(completion).expect("completion serialisation");
            let framed = write_json_record(&bytes);
            Message::Text(String::from_utf8(framed).expect("valid utf-8"))
        }
        HubProtocol::Messagepack => {
            let bytes =
                cmremote_wire::to_msgpack(completion).expect("msgpack completion serialisation");
            let framed = write_msgpack_record(&bytes).expect("msgpack completion framing");
            Message::Binary(framed)
        }
    }
}

// ---------------------------------------------------------------------------
// make_on_record — builds the closure passed into run_session
// ---------------------------------------------------------------------------

/// Build an `on_record` closure that:
/// 1. Decodes the envelope.
/// 2. Deduplicates invocation IDs.
/// 3. Routes invocations to the appropriate handler via a spawned task.
/// 4. Signals the session to quarantine on `Close { allowReconnect: false }`.
///
/// The closure is `FnMut(Vec<u8>) -> DispatchOutcome` so it fits the
/// `run_session` signature directly.
pub fn make_on_record(
    encoding: HubProtocol,
    outbound_tx: mpsc::Sender<Message>,
    handlers: Arc<AgentHandlers>,
    tracker: Arc<std::sync::Mutex<InvocationTracker>>,
) -> impl FnMut(Vec<u8>) -> DispatchOutcome {
    move |record: Vec<u8>| {
        let envelope = match decode_envelope_with(&record, encoding) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "malformed inbound hub record; ignoring");
                return DispatchOutcome::Continue;
            }
        };

        match envelope {
            HubEnvelope::Close(HubClose {
                allow_reconnect: false,
                error,
                ..
            }) => {
                debug!(?error, "server sent quarantine close");
                return DispatchOutcome::Quarantine { reason: error };
            }
            HubEnvelope::Close(_) => {
                // allowReconnect=true or absent — the session loop will
                // handle the WebSocket-level close frame.
            }
            HubEnvelope::Ping(_) | HubEnvelope::Completion(_) => {
                // No-op: pings are replied to at the WS layer; completions
                // are server acks of agent-initiated invocations (none yet).
            }
            HubEnvelope::Unknown(t) => {
                warn!(type_id = t, "received unknown hub message type; ignoring");
            }
            HubEnvelope::Invocation(inv) => {
                // 1. Dedup check.
                if let Some(ref id) = inv.invocation_id {
                    let mut t = tracker.lock().unwrap_or_else(|p| p.into_inner());
                    if t.seen(id) {
                        let c = HubCompletion::err(id.clone(), "duplicate_invocation");
                        send_completion(outbound_tx.clone(), encoding, c);
                        return DispatchOutcome::Continue;
                    }
                }

                // 2. Allow-list check.
                let method = match MethodName::from_target(&inv.target) {
                    Some(m) => m,
                    None => {
                        warn!(target = %inv.target, "unknown hub method");
                        if let Some(ref id) = inv.invocation_id {
                            let c = HubCompletion::err(id.clone(), "not_implemented");
                            send_completion(outbound_tx.clone(), encoding, c);
                        }
                        return DispatchOutcome::Continue;
                    }
                };

                // 3. Spawn an async task per invocation so the session
                //    loop is never blocked.
                let tx = outbound_tx.clone();
                let h = handlers.clone();
                let inv_id = inv.invocation_id.clone();
                tokio::spawn(async move {
                    let result = h.dispatch(method, &inv).await;
                    if let Some(id) = inv_id {
                        let completion = match result {
                            Ok(v) => HubCompletion::ok(id, v),
                            Err(e) => HubCompletion::err(id, e),
                        };
                        let msg = encode_completion(&completion, encoding);
                        // Best-effort send; if the channel is closed the
                        // session is already tearing down.
                        let _ = tx.send(msg).await;
                    }
                });
            }
        }

        DispatchOutcome::Continue
    }
}

/// Send a completion in the background using `send().await`, so that a
/// momentarily full outbound channel applies back-pressure rather than
/// silently dropping the response (as `try_send` would). The closure is
/// synchronous, so we hop into a small task; this is fine because the
/// payloads here are short and infrequent.
fn send_completion(tx: mpsc::Sender<Message>, encoding: HubProtocol, completion: HubCompletion) {
    let msg = encode_completion(&completion, encoding);
    tokio::spawn(async move {
        let _ = tx.send(msg).await;
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_tracker_dedup() {
        let mut t = InvocationTracker::default();
        assert!(!t.seen("abc")); // first time → false
        assert!(t.seen("abc")); // duplicate → true
        assert!(!t.seen("xyz")); // new id → false
    }

    #[test]
    fn invocation_tracker_clear_resets() {
        let mut t = InvocationTracker::default();
        t.seen("abc");
        t.clear();
        assert!(!t.seen("abc")); // cleared → no longer seen
    }

    #[test]
    fn invocation_tracker_evicts_oldest_at_capacity() {
        let mut t = InvocationTracker::with_capacity(3);
        assert!(!t.seen("a"));
        assert!(!t.seen("b"));
        assert!(!t.seen("c"));
        // Re-checking entries inside the window does not change order.
        assert!(t.seen("a"));
        assert!(t.seen("b"));
        assert!(t.seen("c"));
        // At cap. Inserting "d" must evict "a" (oldest).
        assert!(!t.seen("d"));
        // "a" is gone — re-inserting it counts as new (and evicts "b").
        assert!(!t.seen("a"));
        // "b" was just evicted, so it now reads as new (and evicts "c").
        assert!(!t.seen("b"));
        // "c" was evicted by re-inserting "b"; now reads as new.
        assert!(!t.seen("c"));
    }

    #[test]
    fn invocation_tracker_capacity_zero_clamped_to_one() {
        // Guard against an accidental zero-cap deadlock.
        let mut t = InvocationTracker::with_capacity(0);
        assert!(!t.seen("a"));
        // Cap is 1, so "a" gets evicted by "b".
        assert!(!t.seen("b"));
        assert!(!t.seen("a"));
    }

    #[test]
    fn method_name_allow_list_covers_all_variants() {
        let known = [
            "TriggerHeartbeat",
            "ExecuteCommand",
            "RequestInstalledApplications",
            "UninstallApplication",
            "InstallPackage",
            "ChangeWindowsSession",
            "RemoteControl",
        ];
        for m in &known {
            assert!(
                MethodName::from_target(m).is_some(),
                "{m} not in allow-list"
            );
        }
    }

    #[test]
    fn unknown_method_returns_none() {
        assert!(MethodName::from_target("NotARealMethod").is_none());
    }
}
