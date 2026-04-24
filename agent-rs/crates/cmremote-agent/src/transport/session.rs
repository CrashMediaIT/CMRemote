// Source: CMRemote, clean-room implementation.

//! Hub session: handshake exchange, framed read/write loops, and the
//! 15 s ping / 30 s idle-timeout heartbeat machinery.
//!
//! The session is generic over the WebSocket sink/stream pair so the
//! same code path drives both the production `WebSocketStream` and
//! the in-process loopback used by integration tests, without forking
//! the implementation. The split between sink and stream is the
//! reason the heartbeat ping can run concurrently with the read
//! loop: each half lives in its own [`tokio::select!`] arm.

use std::time::Duration;

use cmremote_wire::{
    write_json_record, write_msgpack_record, FramingError, HandshakeRequest, HandshakeResponse,
    HubMessageKind, HubPing, HubProtocol, JsonFrameReader, MsgPackFrameReader,
};
use futures_util::{SinkExt, StreamExt};
use tokio::select;
use tokio::sync::watch;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, trace, warn};

/// Period between outbound `Ping` frames when the connection is idle.
///
/// Pinned by `docs/wire-protocol.md` ➜ *Hub protocol* ➜ *Ping*: 15 s
/// idle ⇒ send a ping; 30 s of total inbound silence ⇒ tear down with
/// code `1011` and reconnect.
pub const PING_INTERVAL: Duration = Duration::from_secs(15);

/// Maximum permissible inbound silence before the agent declares the
/// connection dead.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time we wait for the server to acknowledge the SignalR
/// handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Reasons a hub session terminated. The reconnect loop in
/// [`super::run_until_shutdown`] inspects this to decide whether to
/// back off and retry, surface a fatal error, or stop entirely.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionExit {
    /// The server (or hub-level Close envelope) asked the agent **not**
    /// to reconnect.
    Quarantined {
        /// Optional human-readable reason carried on the Close frame.
        reason: Option<String>,
    },
    /// Idle-timeout fired. We sent code `1011`; reconnect with backoff.
    IdleTimeout,
    /// The server sent `Close { allowReconnect: true }` (or absent),
    /// or the WebSocket simply died. Reconnect with backoff.
    Reconnect {
        /// Optional human-readable reason carried on the Close frame,
        /// or `None` if the stream just ended.
        reason: Option<String>,
    },
    /// Local shutdown signal observed.
    LocalShutdown,
}

/// Session-level errors that propagate up to the reconnect loop as
/// "this attempt failed; back off and retry".
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Underlying WebSocket transport error.
    #[error("websocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),

    /// Wire-level framing error (oversize record, malformed varint, …).
    #[error(transparent)]
    Framing(#[from] FramingError),

    /// SignalR handshake was rejected by the server.
    #[error("handshake rejected: {0}")]
    HandshakeRejected(String),

    /// Handshake timed out.
    #[error("handshake timed out after {:?}", HANDSHAKE_TIMEOUT)]
    HandshakeTimeout,

    /// The server sent an unexpected control frame during handshake.
    #[error("unexpected frame during handshake: {0}")]
    HandshakeProtocol(&'static str),

    /// JSON serialisation/deserialisation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// MessagePack serialisation failed.
    #[error("msgpack error: {0}")]
    MsgPack(String),
}

// Manual `From` so call-sites can keep `?`-ing the unboxed
// `tungstenite::Error` while the variant itself stays cheap to move
// around (see `clippy::result_large_err`).
impl From<tokio_tungstenite::tungstenite::Error> for SessionError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        SessionError::WebSocket(Box::new(e))
    }
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Perform the SignalR handshake exchange on the just-opened
/// connection.
pub async fn perform_handshake<S>(ws: &mut S, encoding: HubProtocol) -> Result<(), SessionError>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let req = HandshakeRequest::new(encoding);
    let payload = serde_json::to_vec(&req)?;
    // Per spec the handshake is *always* JSON, regardless of the
    // hub-protocol encoding negotiated for steady-state traffic.
    let framed = write_json_record(&payload);
    ws.send(Message::Text(
        String::from_utf8(framed).expect("handshake bytes are valid UTF-8"),
    ))
    .await?;
    debug!(?encoding, "sent SignalR handshake request");

    let mut reader = JsonFrameReader::new();
    let deadline_result = timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            let msg = ws.next().await.ok_or(SessionError::HandshakeProtocol(
                "stream closed during handshake",
            ))??;
            match msg {
                Message::Text(text) => reader.push(text.as_bytes())?,
                Message::Binary(bytes) => reader.push(&bytes)?,
                Message::Close(frame) => {
                    let reason = frame.map(|f| f.reason.into_owned());
                    return Err(SessionError::HandshakeRejected(
                        reason.unwrap_or_else(|| "server closed during handshake".to_owned()),
                    ));
                }
                Message::Ping(payload) => {
                    ws.send(Message::Pong(payload)).await?;
                    continue;
                }
                Message::Pong(_) | Message::Frame(_) => continue,
            }
            if let Some(record) = reader.next_record() {
                return Ok::<Vec<u8>, SessionError>(record);
            }
        }
    })
    .await;

    let record = match deadline_result {
        Ok(r) => r?,
        Err(_elapsed) => return Err(SessionError::HandshakeTimeout),
    };

    let response: HandshakeResponse = serde_json::from_slice(&record)?;
    if let Some(error) = response.error {
        return Err(SessionError::HandshakeRejected(error));
    }
    info!("hub handshake accepted");
    Ok(())
}

// ---------------------------------------------------------------------------
// Heartbeat helpers
// ---------------------------------------------------------------------------

/// Build a single SignalR ping frame in the negotiated encoding.
pub fn build_ping_frame(encoding: HubProtocol) -> Result<Message, SessionError> {
    let ping = HubPing::new();
    debug_assert_eq!(ping.kind, HubMessageKind::Ping as u8);
    match encoding {
        HubProtocol::Json => {
            let payload = serde_json::to_vec(&ping)?;
            let framed = write_json_record(&payload);
            // SignalR JSON pings are text frames per spec.
            Ok(Message::Text(
                String::from_utf8(framed).expect("ping bytes are valid UTF-8"),
            ))
        }
        HubProtocol::Messagepack => {
            let payload = cmremote_wire::to_msgpack(&ping)
                .map_err(|e| SessionError::MsgPack(e.to_string()))?;
            let framed = write_msgpack_record(&payload)?;
            Ok(Message::Binary(framed))
        }
    }
}

// ---------------------------------------------------------------------------
// Steady-state session driver: split sink + stream, race read loop +
// heartbeat + shutdown.
// ---------------------------------------------------------------------------

/// Drive the session until one of: server close, idle timeout,
/// shutdown signal, transport error.
///
/// `on_record` is invoked once per inbound hub record; in slice R2
/// the runtime ignores those (the dispatch layer arrives in slice
/// R2a). The hook exists so integration tests can assert on what
/// actually came through.
pub async fn run_session<S>(
    ws: S,
    encoding: HubProtocol,
    shutdown: &mut watch::Receiver<bool>,
    mut on_record: impl FnMut(Vec<u8>),
) -> Result<SessionExit, SessionError>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    let mut json = JsonFrameReader::new();
    let mut msgpack = MsgPackFrameReader::new();
    let mut last_seen = Instant::now();
    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    // The first tick fires immediately; skip it so we don't ping
    // before the server has had a chance to send anything.
    ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping_ticker.tick().await;

    loop {
        // Idle deadline tracks wall-clock since the last inbound
        // frame, not since the last loop iteration.
        let idle_deadline = last_seen + IDLE_TIMEOUT;
        let idle_remaining = idle_deadline.saturating_duration_since(Instant::now());

        select! {
            biased;

            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = sink.send(Message::Close(Some(CloseFrame {
                        code: CloseCode::Normal,
                        reason: "shutdown".into(),
                    }))).await;
                    return Ok(SessionExit::LocalShutdown);
                }
            }

            _ = tokio::time::sleep(idle_remaining) => {
                warn!(
                    silence = ?IDLE_TIMEOUT,
                    "no inbound frame within idle window; closing"
                );
                let _ = sink.send(Message::Close(Some(CloseFrame {
                    code: CloseCode::from(1011u16),
                    reason: "idle".into(),
                }))).await;
                return Ok(SessionExit::IdleTimeout);
            }

            _ = ping_ticker.tick() => {
                let frame = build_ping_frame(encoding)?;
                if let Err(e) = sink.send(frame).await {
                    return Err(SessionError::from(e));
                }
                trace!("sent heartbeat ping");
            }

            msg = stream.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(SessionError::from(e)),
                    None => return Ok(SessionExit::Reconnect { reason: None }),
                };
                last_seen = Instant::now();

                match msg {
                    Message::Text(text) => {
                        if matches!(encoding, HubProtocol::Json) {
                            json.push(text.as_bytes())?;
                            while let Some(rec) = json.next_record() {
                                trace!(bytes = rec.len(), "inbound json record");
                                on_record(rec);
                            }
                        } else {
                            // Wrong-encoding frame on a messagepack
                            // session is a protocol violation per
                            // spec; close 1002.
                            let _ = sink.send(Message::Close(Some(CloseFrame {
                                code: CloseCode::from(1002u16),
                                reason: "wrong-encoding".into(),
                            }))).await;
                            return Ok(SessionExit::Reconnect {
                                reason: Some("server sent text on a messagepack session".to_owned()),
                            });
                        }
                    }
                    Message::Binary(bytes) => {
                        if matches!(encoding, HubProtocol::Messagepack) {
                            msgpack.push(&bytes)?;
                            while let Some(rec) = msgpack.next_record() {
                                trace!(bytes = rec.len(), "inbound msgpack record");
                                on_record(rec);
                            }
                        } else {
                            let _ = sink.send(Message::Close(Some(CloseFrame {
                                code: CloseCode::from(1002u16),
                                reason: "wrong-encoding".into(),
                            }))).await;
                            return Ok(SessionExit::Reconnect {
                                reason: Some("server sent binary on a json session".to_owned()),
                            });
                        }
                    }
                    Message::Ping(payload) => {
                        sink.send(Message::Pong(payload)).await?;
                    }
                    Message::Pong(_) => { /* counts as activity, no further work */ }
                    Message::Close(frame) => {
                        let reason = frame.and_then(|f| {
                            if f.reason.is_empty() { None } else { Some(f.reason.into_owned()) }
                        });
                        // The hub-level Close envelope (with
                        // `allowReconnect`) is dispatched inside the
                        // record stream above; the WebSocket-level
                        // close is treated as reconnectable here.
                        return Ok(SessionExit::Reconnect { reason });
                    }
                    Message::Frame(_) => { /* unreachable without `unstable` features */ }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_frame_round_trips_through_reader_json() {
        let frame = build_ping_frame(HubProtocol::Json).unwrap();
        let bytes = match frame {
            Message::Text(s) => s.into_bytes(),
            other => panic!("expected text frame, got {other:?}"),
        };
        let mut r = JsonFrameReader::new();
        r.push(&bytes).unwrap();
        let rec = r.next_record().expect("one record produced");
        let ping: HubPing = serde_json::from_slice(&rec).unwrap();
        assert_eq!(ping.kind, HubMessageKind::Ping as u8);
    }

    #[test]
    fn ping_frame_round_trips_through_reader_msgpack() {
        let frame = build_ping_frame(HubProtocol::Messagepack).unwrap();
        let bytes = match frame {
            Message::Binary(b) => b,
            other => panic!("expected binary frame, got {other:?}"),
        };
        let mut r = MsgPackFrameReader::new();
        r.push(&bytes).unwrap();
        let rec = r.next_record().expect("one record produced");
        let ping: HubPing = cmremote_wire::from_msgpack(&rec).unwrap();
        assert_eq!(ping.kind, HubMessageKind::Ping as u8);
    }

    #[test]
    fn timing_constants_match_spec() {
        // Pin the spec values directly so an editorial change to the
        // protocol doc doesn't silently drift the timeouts.
        assert_eq!(PING_INTERVAL, Duration::from_secs(15));
        assert_eq!(IDLE_TIMEOUT, Duration::from_secs(30));
        assert!(HANDSHAKE_TIMEOUT >= Duration::from_secs(5));
    }
}
