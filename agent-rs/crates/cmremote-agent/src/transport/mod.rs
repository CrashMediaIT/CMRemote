// Source: CMRemote, clean-room implementation.

//! WebSocket transport for the agent ↔ server hub.
//!
//! Slice R2 of the Rust agent rewrite (see `ROADMAP.md` ➜ *Rust agent
//! slice-by-slice delivery plan*). Splits cleanly into:
//!
//! * [`backoff`] — jittered exponential reconnect schedule
//! * [`connect`] — URL + header construction, sub-protocol negotiation
//! * [`session`] — handshake, framed read loop with heartbeat
//!
//! The top-level [`run_until_shutdown`] glues those together: connect,
//! handshake, drive the session, on disconnect either back off and
//! reconnect or honour a quarantine close, and on local shutdown tear
//! down cleanly.
//!
//! Slice R2a wires in the dispatch layer: the `on_record` closure now
//! routes inbound hub invocations to the appropriate handler.

pub mod backoff;
pub mod connect;
pub mod session;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cmremote_wire::{ConnectionInfo, HubProtocol};
use tokio::select;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tracing::{error, info, warn};

pub use backoff::{Backoff, BACKOFF_BASE, BACKOFF_CAP};
pub use connect::{
    build_request, negotiate_subprotocol, ConnectError, AGENT_HUB_PATH, PROTOCOL_VERSION,
};
pub use session::{
    build_ping_frame, perform_handshake, run_session, SessionError, SessionExit, HANDSHAKE_TIMEOUT,
    IDLE_TIMEOUT, PING_INTERVAL,
};

use crate::dispatch::{make_on_record, InvocationTracker};
use crate::handlers::AgentHandlers;

/// Errors surfaced from the transport layer to the runtime.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The configured `Host` is malformed or uses a non-`wss://`
    /// scheme; retrying would not help.
    #[error("agent configuration is invalid; cannot connect")]
    Configuration,

    /// Transient connect-time error.
    #[error(transparent)]
    Connect(#[from] ConnectError),
}

/// Top-level reconnect loop.
///
/// Runs until either `shutdown` flips to `true` (cooperative stop
/// from the runtime) or the server quarantines the agent.
pub async fn run_until_shutdown(
    info: ConnectionInfo,
    handlers: Arc<AgentHandlers>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), TransportError> {
    let mut backoff = Backoff::new();

    // Honour the spec's "messagepack preferred in production" policy.
    let preferred = HubProtocol::Messagepack;

    loop {
        if *shutdown.borrow() {
            info!("transport stopping before connect: shutdown requested");
            return Ok(());
        }

        let req = match build_request(&info, preferred) {
            Ok(r) => r,
            Err(ConnectError::InvalidHost { .. } | ConnectError::InsecureScheme(_)) => {
                error!(
                    "transport configuration is invalid; not retrying. \
                     Fix the agent config and restart."
                );
                return Err(TransportError::Configuration);
            }
            Err(other) => return Err(TransportError::Connect(other)),
        };

        info!(attempt = backoff.attempts(), "dialling hub");

        let result = select! {
            r = tokio_tungstenite::connect_async(req) => r,
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("shutdown signal received during dial");
                    return Ok(());
                }
                continue;
            }
        };

        let (mut ws, response) = match result {
            Ok(ok) => ok,
            Err(e) => {
                let sleep = backoff.next_sleep();
                warn!(error = %e, retry_in = ?sleep, "dial failed");
                if wait_or_shutdown(sleep, &mut shutdown).await {
                    return Ok(());
                }
                continue;
            }
        };

        let encoding = match negotiate_subprotocol(response.headers().get(SEC_WEBSOCKET_PROTOCOL)) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "server selected an unsupported sub-protocol; closing");
                let _ = ws.close(None).await;
                let sleep = backoff.next_sleep();
                if wait_or_shutdown(sleep, &mut shutdown).await {
                    return Ok(());
                }
                continue;
            }
        };

        match perform_handshake(&mut ws, encoding).await {
            Ok(()) => {
                backoff.reset();
            }
            Err(e) => {
                warn!(error = %e, "handshake failed");
                let _ = ws.close(None).await;
                let sleep = backoff.next_sleep();
                if wait_or_shutdown(sleep, &mut shutdown).await {
                    return Ok(());
                }
                continue;
            }
        }

        // Per-connection outbound channel: dispatch tasks inject
        // HubCompletion messages that the session loop flushes to the sink.
        let (outbound_tx, mut outbound_rx) =
            mpsc::channel::<tokio_tungstenite::tungstenite::Message>(64);

        // Fresh per-connection invocation-ID tracker.
        let tracker = Arc::new(Mutex::new(InvocationTracker::default()));

        let on_record = make_on_record(encoding, outbound_tx, handlers.clone(), tracker);

        // Steady state: read loop + ping ticker + outbound drain, all
        // racing the shutdown signal.
        let exit = run_session(ws, encoding, &mut shutdown, &mut outbound_rx, on_record).await;

        match exit {
            Ok(SessionExit::LocalShutdown) => return Ok(()),
            Ok(SessionExit::Quarantined { reason }) => {
                warn!(?reason, "server quarantined this agent; not reconnecting");
                return Ok(());
            }
            Ok(SessionExit::Reconnect { reason }) => {
                let sleep = backoff.next_sleep();
                info!(?reason, retry_in = ?sleep, "session ended; reconnecting");
                if wait_or_shutdown(sleep, &mut shutdown).await {
                    return Ok(());
                }
            }
            Ok(SessionExit::IdleTimeout) => {
                let sleep = backoff.next_sleep();
                info!(retry_in = ?sleep, "idle-timeout; reconnecting");
                if wait_or_shutdown(sleep, &mut shutdown).await {
                    return Ok(());
                }
            }
            Err(e) => {
                let sleep = backoff.next_sleep();
                warn!(error = %e, retry_in = ?sleep, "session error; reconnecting");
                if wait_or_shutdown(sleep, &mut shutdown).await {
                    return Ok(());
                }
            }
        }
    }
}

/// Sleep for `dur`, but bail out early if the shutdown signal flips
/// to `true`. Returns `true` iff the loop should stop now.
async fn wait_or_shutdown(dur: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    if dur.is_zero() {
        return *shutdown.borrow();
    }
    select! {
        _ = tokio::time::sleep(dur) => *shutdown.borrow(),
        _ = shutdown.changed() => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_or_shutdown_returns_true_immediately_when_already_set() {
        let (tx, mut rx) = watch::channel(false);
        tx.send(true).unwrap();
        let stopped = wait_or_shutdown(Duration::from_millis(50), &mut rx).await;
        assert!(stopped);
    }

    #[tokio::test]
    async fn wait_or_shutdown_returns_false_when_signal_stays_low() {
        let (_tx, mut rx) = watch::channel(false);
        let stopped = wait_or_shutdown(Duration::from_millis(20), &mut rx).await;
        assert!(!stopped);
    }

    #[tokio::test]
    async fn wait_or_shutdown_short_circuits_on_zero_duration() {
        let (_tx, mut rx) = watch::channel(false);
        let stopped = wait_or_shutdown(Duration::ZERO, &mut rx).await;
        assert!(!stopped);
    }
}
