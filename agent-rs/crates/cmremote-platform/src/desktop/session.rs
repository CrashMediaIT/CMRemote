// Source: CMRemote, clean-room implementation.

//! Per-session state machine and registry for the desktop transport
//! (slice R7.k — *concrete* lifecycle infrastructure that real WebRTC
//! drivers plug into; the actual peer-connection construction is still
//! pending the supply-chain audit gated by ADR 0001).
//!
//! ## Why this lives in `cmremote-platform`
//!
//! Every previous R7 slice (R7, R7.b, R7.c, R7.d, R7.f, R7.g, R7.h,
//! R7.i, R7.j) shipped a trait surface or a wire DTO and stopped short
//! of any concrete `DesktopTransportProvider` implementation. The
//! reason was load-bearing: a real driver needs **per-session state**
//! (which session is currently negotiating, which has gone idle, what
//! its last SDP shape was) and the previous default
//! [`super::NotSupportedDesktopTransport`] was deliberately stateless.
//!
//! This module is the missing stateful seam. It is pure Rust, has no
//! `webrtc` / `webrtc-rs` dependency, no per-OS code, and no banned
//! crates — so it can land *before* Gate A (the full `webrtc` crate
//! graph audit, ADR 0001) is resolved.
//!
//! ## State machine
//!
//! ```text
//!     RemoteControl ──► Initializing ──► IceConfigured ──► NegotiatingSdp ──► Connected ──► Closed
//!                            │                  │                  │              │
//!                            └──────────────────┴──────────────────┴──────────────┘
//!                                              (any state can transition to Closed)
//! ```
//!
//! Allowed transitions:
//!
//! | From            | To              | Triggered by                       |
//! |-----------------|-----------------|------------------------------------|
//! | (none)          | `Initializing`  | `RemoteControl` invocation         |
//! | `Initializing`  | `IceConfigured` | `ProvideIceServers` invocation     |
//! | `IceConfigured` | `NegotiatingSdp`| First `SendSdpOffer` for session   |
//! | `Initializing`  | `NegotiatingSdp`| `SendSdpOffer` without prior ICE   |
//! | `NegotiatingSdp`| `Connected`     | Driver-internal (peer-connection up) |
//! | any             | `Closed`        | Idle timeout, explicit close, or replace-on-duplicate |
//!
//! Re-receiving the same trigger in the same state is a no-op
//! (idempotent — the .NET hub may retry an invocation across reconnect).
//!
//! ## Registry semantics
//!
//! - Keyed by canonical lowercase-UUID `session_id`. Callers MUST
//!   validate the id through [`super::guards`] before calling
//!   [`DesktopSessionRegistry::open`] — the registry trusts the key
//!   shape and panics in debug if a non-UUID slips through (the
//!   `[debug_assertions]` check is a defence-in-depth net, never a
//!   first line of validation).
//! - **Replace-on-duplicate.** A second `RemoteControl` for the same
//!   `session_id` closes the existing session (state ⇒ `Closed`) and
//!   creates a fresh one. The .NET hub uses this when a viewer
//!   reconnects mid-negotiation.
//! - **Idle timeout.** Each session records `last_activity` on every
//!   trigger; [`DesktopSessionRegistry::sweep_idle`] removes (and
//!   audit-logs) sessions whose last activity is older than the
//!   configured timeout. The actual scheduler is the runtime's job —
//!   the registry exposes the sweep as a single sync call so it can be
//!   driven from a `tokio::time::interval` or a test clock without
//!   coupling.
//! - **Cross-session isolation.** Mutating one session never touches
//!   another. Every accessor takes `&mut self` so the borrow checker
//!   prevents accidentally aliasing two sessions in safe Rust code.
//!
//! ## Audit logging
//!
//! Every state transition emits a `tracing` event at `info` level with
//! structured fields:
//!
//! - `session_id` — the canonical UUID
//! - `from_state` / `to_state` — the textual state names
//! - `viewer_connection_id` — the viewer that drove the transition
//!   (when known; `""` otherwise)
//! - `event` — short stable label for the trigger
//!
//! Sensitive values (`access_key`, SDP body, ICE credential) MUST NOT
//! be passed to these helpers; the constructors take only the
//! non-sensitive envelope fields and the wire-protocol guard helpers
//! refuse a payload before its sensitive fields are read.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

/// State of a single desktop-transport session.
///
/// Transitions are enforced by [`DesktopSession::transition`] — a
/// caller cannot accidentally jump from `Initializing` to `Connected`
/// without going through the intermediate states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesktopSessionState {
    /// `RemoteControl` accepted; awaiting `ProvideIceServers` or
    /// `SendSdpOffer`. The driver has not yet built any per-session
    /// peer-connection state.
    Initializing,
    /// `ProvideIceServers` accepted; the driver has the
    /// `IceServerConfig` it needs to construct an `RTCConfiguration`
    /// but has not yet started SDP negotiation.
    IceConfigured,
    /// First `SendSdpOffer` received; the peer connection is mid-handshake.
    NegotiatingSdp,
    /// Handshake completed; media is flowing.
    Connected,
    /// Session has ended (idle timeout, explicit close, or replaced by
    /// a new `RemoteControl` for the same id). A `Closed` session is
    /// kept in the registry briefly so a late-arriving signalling
    /// message gets a deterministic "session ended" failure rather
    /// than a misleading "no such session" failure; the next sweep
    /// removes it.
    Closed,
}

impl DesktopSessionState {
    /// Short stable string used in `tracing` events and result messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::IceConfigured => "ice-configured",
            Self::NegotiatingSdp => "negotiating-sdp",
            Self::Connected => "connected",
            Self::Closed => "closed",
        }
    }

    /// `true` when no further state transitions are expected — the
    /// session is awaiting removal by the next idle sweep.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Reason a session moved to [`DesktopSessionState::Closed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The registry's idle-timeout sweep evicted the session.
    IdleTimeout,
    /// A second `RemoteControl` arrived for the same `session_id`.
    Replaced,
    /// An explicit close was requested (typically by the driver after
    /// the peer-connection terminated, or by the runtime on shutdown).
    Explicit,
    /// A guard refused a transition trigger and the registry chose to
    /// close the session rather than leave it in a half-initialised
    /// state.
    GuardFailure,
}

impl CloseReason {
    /// Short stable string used in `tracing` events.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdleTimeout => "idle-timeout",
            Self::Replaced => "replaced",
            Self::Explicit => "explicit",
            Self::GuardFailure => "guard-failure",
        }
    }
}

/// Result of attempting a state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// The state changed from `from` to `to`.
    Transitioned {
        /// Previous state.
        from: DesktopSessionState,
        /// New state.
        to: DesktopSessionState,
    },
    /// The trigger was idempotent — the session was already in the
    /// requested state. `last_activity` is still refreshed so an
    /// idempotent retry counts as activity for the idle sweep.
    AlreadyInState(DesktopSessionState),
    /// The transition is not allowed from the current state. The
    /// session's state is **unchanged** and `last_activity` is **not**
    /// refreshed (a refused trigger must not extend the idle window).
    Refused {
        /// State the session is in now.
        current: DesktopSessionState,
        /// State the trigger would have moved it to.
        attempted: DesktopSessionState,
    },
}

/// A single desktop-transport session.
#[derive(Debug, Clone)]
pub struct DesktopSession {
    /// Canonical lowercase-UUID `session_id`.
    pub session_id: String,
    /// Viewer that opened the session (from `RemoteControl`).
    pub viewer_connection_id: String,
    /// Current state.
    pub state: DesktopSessionState,
    /// Wall-clock instant of the last accepted trigger. Idempotent
    /// retries refresh this; refused triggers do not.
    pub last_activity: Instant,
    /// Wall-clock instant the session was opened.
    pub opened_at: Instant,
}

impl DesktopSession {
    /// Build a new session in [`DesktopSessionState::Initializing`].
    /// Used by [`DesktopSessionRegistry::open`]; tests can also
    /// construct one directly.
    pub fn new(session_id: String, viewer_connection_id: String) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            viewer_connection_id,
            state: DesktopSessionState::Initializing,
            last_activity: now,
            opened_at: now,
        }
    }

    /// Apply a state transition. Returns the [`TransitionOutcome`]
    /// describing whether the transition was accepted, idempotent, or
    /// refused. Emits a `tracing::info!` event on every accepted
    /// non-idempotent transition.
    ///
    /// `event` is a short stable label for the trigger (e.g.
    /// `"send-sdp-offer"`) used in the audit log.
    pub fn transition(
        &mut self,
        target: DesktopSessionState,
        event: &'static str,
    ) -> TransitionOutcome {
        if self.state == target {
            self.last_activity = Instant::now();
            return TransitionOutcome::AlreadyInState(target);
        }
        if !is_allowed(self.state, target) {
            return TransitionOutcome::Refused {
                current: self.state,
                attempted: target,
            };
        }
        let from = self.state;
        self.state = target;
        self.last_activity = Instant::now();
        tracing::info!(
            session_id = %self.session_id,
            viewer_connection_id = %self.viewer_connection_id,
            from_state = from.as_str(),
            to_state = target.as_str(),
            event = event,
            "desktop session transition",
        );
        TransitionOutcome::Transitioned { from, to: target }
    }

    /// `true` when [`Self::last_activity`] is older than `timeout`
    /// relative to `now`.
    pub fn is_idle(&self, now: Instant, timeout: Duration) -> bool {
        now.saturating_duration_since(self.last_activity) >= timeout
    }
}

/// Allowed transitions. See the module-level state diagram.
fn is_allowed(from: DesktopSessionState, to: DesktopSessionState) -> bool {
    use DesktopSessionState::*;
    match (from, to) {
        // Any state can move to Closed.
        (_, Closed) => true,
        // Linear forward path.
        (Initializing, IceConfigured) => true,
        (IceConfigured, NegotiatingSdp) => true,
        (Initializing, NegotiatingSdp) => true,
        (NegotiatingSdp, Connected) => true,
        // Re-negotiation: a Connected session can drop back to
        // NegotiatingSdp when the viewer issues a new SDP offer
        // (renegotiation is part of the WebRTC contract).
        (Connected, NegotiatingSdp) => true,
        // Closed is terminal — even Closed→Closed is handled by the
        // idempotent branch in `transition`, never reaches here.
        _ => false,
    }
}

/// Default idle timeout used by [`DesktopSessionRegistry::with_default_timeout`].
///
/// Mirrors the .NET implementation's two-minute idle window for an
/// unattached desktop session.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Outcome of [`DesktopSessionRegistry::open`] — distinguishes a fresh
/// session from a replace-on-duplicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    /// No prior session existed; a fresh one was created.
    Fresh,
    /// A prior session existed and was closed (audit-logged with
    /// [`CloseReason::Replaced`]) before the fresh one was created.
    Replaced {
        /// State the prior session was in at the moment of replacement.
        prior_state: DesktopSessionState,
    },
}

/// Per-process registry of active desktop-transport sessions, keyed
/// by canonical lowercase-UUID `session_id`.
///
/// Cheap to construct (`HashMap` only); not thread-safe by itself —
/// callers wrap it in a [`tokio::sync::Mutex`] when sharing across
/// tasks. The single-mutex pattern is intentional: every method here
/// returns immediately, so the lock is never held across an `await`.
#[derive(Debug)]
pub struct DesktopSessionRegistry {
    sessions: HashMap<String, DesktopSession>,
    idle_timeout: Duration,
}

impl DesktopSessionRegistry {
    /// Build an empty registry with `idle_timeout` as the eviction window.
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            sessions: HashMap::new(),
            idle_timeout,
        }
    }

    /// Build an empty registry with the [`DEFAULT_IDLE_TIMEOUT`].
    pub fn with_default_timeout() -> Self {
        Self::new(DEFAULT_IDLE_TIMEOUT)
    }

    /// Number of sessions currently tracked, including any that are
    /// in [`DesktopSessionState::Closed`] but have not yet been swept.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// `true` when [`Self::len`] is zero.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Configured idle timeout.
    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    /// Open a new session for `session_id` (or replace an existing one).
    /// `session_id` MUST already have been validated by the caller's
    /// guard helper — the registry asserts the canonical-UUID shape
    /// in debug builds only.
    ///
    /// Emits a `tracing::info!` event for the open, and a separate
    /// `tracing::warn!` event when an existing session is replaced.
    pub fn open(&mut self, session_id: &str, viewer_connection_id: &str) -> OpenOutcome {
        debug_assert!(
            looks_like_canonical_uuid(session_id),
            "DesktopSessionRegistry::open called with a non-canonical session_id; \
             the caller must run `guards::check_remote_control` first",
        );
        let outcome = if let Some(prior) = self.sessions.remove(session_id) {
            tracing::warn!(
                session_id = %session_id,
                prior_state = prior.state.as_str(),
                close_reason = CloseReason::Replaced.as_str(),
                "desktop session replaced by a fresh RemoteControl",
            );
            OpenOutcome::Replaced {
                prior_state: prior.state,
            }
        } else {
            OpenOutcome::Fresh
        };
        let session = DesktopSession::new(session_id.to_string(), viewer_connection_id.to_string());
        tracing::info!(
            session_id = %session.session_id,
            viewer_connection_id = %session.viewer_connection_id,
            from_state = "<none>",
            to_state = session.state.as_str(),
            event = "remote-control",
            "desktop session opened",
        );
        self.sessions.insert(session_id.to_string(), session);
        outcome
    }

    /// Borrow a session immutably.
    pub fn get(&self, session_id: &str) -> Option<&DesktopSession> {
        self.sessions.get(session_id)
    }

    /// Apply a state transition to an existing session. Returns
    /// `None` if no such session is registered (the caller surfaces
    /// that as a "session not initialised" failure to the wire).
    pub fn transition(
        &mut self,
        session_id: &str,
        target: DesktopSessionState,
        event: &'static str,
    ) -> Option<TransitionOutcome> {
        let session = self.sessions.get_mut(session_id)?;
        Some(session.transition(target, event))
    }

    /// Explicitly close `session_id`. Audit-logs the close with the
    /// supplied [`CloseReason`] and removes the session from the map.
    /// Returns `true` if a session was removed.
    pub fn close(&mut self, session_id: &str, reason: CloseReason) -> bool {
        if let Some(session) = self.sessions.remove(session_id) {
            tracing::info!(
                session_id = %session.session_id,
                viewer_connection_id = %session.viewer_connection_id,
                from_state = session.state.as_str(),
                to_state = DesktopSessionState::Closed.as_str(),
                close_reason = reason.as_str(),
                event = "close",
                "desktop session closed",
            );
            true
        } else {
            false
        }
    }

    /// Evict every session whose `last_activity` is older than
    /// [`Self::idle_timeout`] relative to `now`. Returns the list of
    /// evicted session ids in the order they were removed (test
    /// helper; production callers usually only need the count).
    pub fn sweep_idle(&mut self, now: Instant) -> Vec<String> {
        let timeout = self.idle_timeout;
        let mut evicted: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.is_idle(now, timeout))
            .map(|(id, _)| id.clone())
            .collect();
        // Sort for deterministic ordering in tests; production cost is
        // negligible because `evicted` is bounded by the number of
        // active sessions on a single agent (single digits in practice).
        evicted.sort();
        for id in &evicted {
            self.close(id, CloseReason::IdleTimeout);
        }
        evicted
    }
}

impl Default for DesktopSessionRegistry {
    fn default() -> Self {
        Self::with_default_timeout()
    }
}

/// Cheap shape check matching the canonical-UUID format the .NET hub
/// emits (lowercase `8-4-4-4-12`). Duplicates the canonical check in
/// [`super::guards`] for the debug-only assertion in
/// [`DesktopSessionRegistry::open`] so we don't have to plumb the
/// guard module through this layer's API.
fn looks_like_canonical_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    s.bytes().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => matches!(b, b'0'..=b'9' | b'a'..=b'f'),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_A: &str = "11111111-2222-3333-4444-555555555555";
    const SESSION_B: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const VIEWER_1: &str = "viewer-1";
    const VIEWER_2: &str = "viewer-2";

    #[tokio::test]
    async fn open_creates_initializing_session() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        assert_eq!(reg.open(SESSION_A, VIEWER_1), OpenOutcome::Fresh);
        let s = reg.get(SESSION_A).unwrap();
        assert_eq!(s.state, DesktopSessionState::Initializing);
        assert_eq!(s.viewer_connection_id, VIEWER_1);
        assert_eq!(s.session_id, SESSION_A);
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn linear_happy_path_traverses_every_state() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        for (target, event) in [
            (DesktopSessionState::IceConfigured, "provide-ice-servers"),
            (DesktopSessionState::NegotiatingSdp, "send-sdp-offer"),
            (DesktopSessionState::Connected, "peer-connection-up"),
        ] {
            let out = reg.transition(SESSION_A, target, event).unwrap();
            assert!(
                matches!(out, TransitionOutcome::Transitioned { .. }),
                "{out:?}"
            );
        }
        assert_eq!(
            reg.get(SESSION_A).unwrap().state,
            DesktopSessionState::Connected
        );
    }

    #[tokio::test]
    async fn initializing_can_skip_directly_to_negotiating_sdp() {
        // The .NET hub may emit `SendSdpOffer` without a prior
        // `ProvideIceServers` (the viewer reuses defaults) — pin that
        // shortcut in the state machine.
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        let out = reg
            .transition(
                SESSION_A,
                DesktopSessionState::NegotiatingSdp,
                "send-sdp-offer",
            )
            .unwrap();
        assert!(matches!(
            out,
            TransitionOutcome::Transitioned {
                from: DesktopSessionState::Initializing,
                to: DesktopSessionState::NegotiatingSdp
            }
        ));
    }

    #[tokio::test]
    async fn renegotiation_drops_connected_back_to_negotiating_sdp() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        reg.transition(SESSION_A, DesktopSessionState::IceConfigured, "ice")
            .unwrap();
        reg.transition(SESSION_A, DesktopSessionState::NegotiatingSdp, "sdp")
            .unwrap();
        reg.transition(SESSION_A, DesktopSessionState::Connected, "up")
            .unwrap();
        let out = reg
            .transition(
                SESSION_A,
                DesktopSessionState::NegotiatingSdp,
                "renegotiate",
            )
            .unwrap();
        assert!(matches!(
            out,
            TransitionOutcome::Transitioned {
                from: DesktopSessionState::Connected,
                to: DesktopSessionState::NegotiatingSdp
            }
        ));
    }

    #[tokio::test]
    async fn idempotent_retry_is_recognised_and_refreshes_activity() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        reg.transition(SESSION_A, DesktopSessionState::IceConfigured, "ice")
            .unwrap();
        let before = reg.get(SESSION_A).unwrap().last_activity;
        // Sleep one tokio tick so `Instant::now()` advances measurably.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let out = reg
            .transition(SESSION_A, DesktopSessionState::IceConfigured, "ice-retry")
            .unwrap();
        assert_eq!(
            out,
            TransitionOutcome::AlreadyInState(DesktopSessionState::IceConfigured)
        );
        let after = reg.get(SESSION_A).unwrap().last_activity;
        assert!(
            after > before,
            "idempotent retry must refresh last_activity"
        );
    }

    #[tokio::test]
    async fn invalid_transition_is_refused_and_state_is_unchanged() {
        // Initializing → Connected is not allowed (must go through
        // NegotiatingSdp). The state must not change.
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        let before = reg.get(SESSION_A).unwrap().last_activity;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let out = reg
            .transition(SESSION_A, DesktopSessionState::Connected, "bad-jump")
            .unwrap();
        assert_eq!(
            out,
            TransitionOutcome::Refused {
                current: DesktopSessionState::Initializing,
                attempted: DesktopSessionState::Connected,
            }
        );
        let s = reg.get(SESSION_A).unwrap();
        assert_eq!(s.state, DesktopSessionState::Initializing);
        // Refused transition must NOT extend the idle window.
        assert_eq!(s.last_activity, before);
    }

    #[tokio::test]
    async fn transition_on_unknown_session_returns_none() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        assert!(reg
            .transition(SESSION_A, DesktopSessionState::IceConfigured, "ice")
            .is_none());
    }

    #[tokio::test]
    async fn replace_on_duplicate_remote_control_closes_prior_session() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        reg.transition(SESSION_A, DesktopSessionState::IceConfigured, "ice")
            .unwrap();
        let outcome = reg.open(SESSION_A, VIEWER_2);
        assert_eq!(
            outcome,
            OpenOutcome::Replaced {
                prior_state: DesktopSessionState::IceConfigured,
            },
        );
        // Fresh session is in Initializing, with the new viewer id.
        let s = reg.get(SESSION_A).unwrap();
        assert_eq!(s.state, DesktopSessionState::Initializing);
        assert_eq!(s.viewer_connection_id, VIEWER_2);
        // Registry size is still 1 — the prior was removed before the
        // fresh one was inserted.
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn cross_session_isolation_one_session_does_not_affect_another() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        reg.open(SESSION_A, VIEWER_1);
        reg.open(SESSION_B, VIEWER_2);
        reg.transition(SESSION_A, DesktopSessionState::IceConfigured, "ice")
            .unwrap();
        // SESSION_B must still be Initializing.
        assert_eq!(
            reg.get(SESSION_B).unwrap().state,
            DesktopSessionState::Initializing
        );
        // Closing SESSION_A leaves SESSION_B intact.
        assert!(reg.close(SESSION_A, CloseReason::Explicit));
        assert!(reg.get(SESSION_A).is_none());
        assert_eq!(
            reg.get(SESSION_B).unwrap().state,
            DesktopSessionState::Initializing
        );
    }

    #[tokio::test]
    async fn close_returns_false_for_unknown_session() {
        let mut reg = DesktopSessionRegistry::with_default_timeout();
        assert!(!reg.close(SESSION_A, CloseReason::Explicit));
    }

    #[tokio::test(start_paused = true)]
    async fn sweep_idle_evicts_sessions_past_the_timeout() {
        let mut reg = DesktopSessionRegistry::new(Duration::from_secs(60));
        reg.open(SESSION_A, VIEWER_1);
        reg.open(SESSION_B, VIEWER_2);
        // Touch SESSION_B 30 s in to keep it active.
        tokio::time::advance(Duration::from_secs(30)).await;
        reg.transition(SESSION_B, DesktopSessionState::IceConfigured, "ice")
            .unwrap();
        // Move past the SESSION_A timeout but not the (refreshed)
        // SESSION_B timeout.
        tokio::time::advance(Duration::from_secs(40)).await;
        let evicted = reg.sweep_idle(Instant::now());
        assert_eq!(evicted, vec![SESSION_A.to_string()]);
        assert!(reg.get(SESSION_A).is_none());
        assert!(reg.get(SESSION_B).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn sweep_idle_is_a_no_op_when_no_sessions_are_idle() {
        let mut reg = DesktopSessionRegistry::new(Duration::from_secs(60));
        reg.open(SESSION_A, VIEWER_1);
        tokio::time::advance(Duration::from_secs(10)).await;
        let evicted = reg.sweep_idle(Instant::now());
        assert!(evicted.is_empty());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn allowed_transitions_table_is_exhaustive() {
        use DesktopSessionState::*;
        // Every accepted edge — keep this table in lockstep with
        // `is_allowed` so a future change to the state machine is
        // covered by an explicit test failure.
        for (from, to) in [
            (Initializing, IceConfigured),
            (Initializing, NegotiatingSdp),
            (IceConfigured, NegotiatingSdp),
            (NegotiatingSdp, Connected),
            (Connected, NegotiatingSdp),
            (Initializing, Closed),
            (IceConfigured, Closed),
            (NegotiatingSdp, Closed),
            (Connected, Closed),
        ] {
            assert!(is_allowed(from, to), "{from:?} -> {to:?} should be allowed");
        }
        // A representative refusal — Initializing cannot leap to
        // Connected, IceConfigured cannot fall back to Initializing.
        assert!(!is_allowed(Initializing, Connected));
        assert!(!is_allowed(IceConfigured, Initializing));
        assert!(!is_allowed(Closed, Initializing));
    }

    #[test]
    fn state_as_str_is_stable() {
        // Audit-log consumers depend on these strings; keep them
        // pinned.
        assert_eq!(DesktopSessionState::Initializing.as_str(), "initializing");
        assert_eq!(
            DesktopSessionState::IceConfigured.as_str(),
            "ice-configured"
        );
        assert_eq!(
            DesktopSessionState::NegotiatingSdp.as_str(),
            "negotiating-sdp"
        );
        assert_eq!(DesktopSessionState::Connected.as_str(), "connected");
        assert_eq!(DesktopSessionState::Closed.as_str(), "closed");
    }

    #[test]
    fn close_reason_as_str_is_stable() {
        assert_eq!(CloseReason::IdleTimeout.as_str(), "idle-timeout");
        assert_eq!(CloseReason::Replaced.as_str(), "replaced");
        assert_eq!(CloseReason::Explicit.as_str(), "explicit");
        assert_eq!(CloseReason::GuardFailure.as_str(), "guard-failure");
    }

    #[test]
    fn looks_like_canonical_uuid_matches_the_guards_module_shape() {
        assert!(looks_like_canonical_uuid(SESSION_A));
        assert!(looks_like_canonical_uuid(SESSION_B));
        // Mixed case, wrong length, missing dashes, non-hex.
        assert!(!looks_like_canonical_uuid(
            "AAAAAAAA-bbbb-cccc-dddd-eeeeeeeeeeee"
        ));
        assert!(!looks_like_canonical_uuid("not-a-uuid"));
        assert!(!looks_like_canonical_uuid(
            "11111111222233334444555555555555"
        ));
        assert!(!looks_like_canonical_uuid(
            "zzzzzzzz-2222-3333-4444-555555555555"
        ));
    }
}
