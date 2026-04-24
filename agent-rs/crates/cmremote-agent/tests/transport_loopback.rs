// Source: CMRemote, clean-room implementation.
//
// End-to-end integration test for slice R2 — the connection /
// heartbeat loop. We can't exercise the wss:// + TLS path inside CI
// without a CA-signed cert, so the test drives the session machinery
// against a plain-ws loopback server, which is enough to pin:
//
//   * the SignalR handshake exchange (request, server-ok response)
//   * a steady-state inbound record arriving on a json-encoded session
//   * a server-initiated WebSocket close producing
//     `SessionExit::Reconnect`
//   * a clean exit on a local shutdown signal
//   * a typed handshake-rejection error path
//
// The wss:// + cert-validation paths are pinned by unit tests in
// `transport::connect`. Together the two halves give us full slice
// coverage without needing a TLS terminator in CI.

use std::time::Duration;

use cmremote_agent::transport::{
    perform_handshake, run_session, SessionError, SessionExit, IDLE_TIMEOUT, PING_INTERVAL,
};
use cmremote_wire::{
    write_json_record, HandshakeResponse, HubMessageKind, HubProtocol, JsonFrameReader,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

/// What the loopback server should do after the handshake completes.
enum AfterHandshake {
    /// Send one server-initiated record, then close the connection.
    SendRecordThenClose(&'static str),
    /// Sit idle and let the session run until the test drops it.
    KeepOpen,
}

/// Spin up a single-connection ws loopback server and return the
/// bound address. The server runs as a tokio task and exits after
/// the first connection completes.
async fn spawn_loopback_server(after: AfterHandshake) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("ws accept");

        // ---- Handshake ----
        // Read until we have one full json record (the agent's
        // handshake request) and reply with `{}` followed by 0x1E.
        let mut reader = JsonFrameReader::new();
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    reader.push(t.as_bytes()).expect("push");
                }
                Some(Ok(Message::Binary(b))) => reader.push(&b).expect("push"),
                Some(Ok(_)) => continue,
                Some(Err(e)) => panic!("ws error during handshake: {e}"),
                None => panic!("stream ended during handshake"),
            }
            if reader.next_record().is_some() {
                break;
            }
        }

        let ok = serde_json::to_vec(&HandshakeResponse::ok()).expect("encode ok");
        let framed = write_json_record(&ok);
        ws.send(Message::Text(String::from_utf8(framed).unwrap()))
            .await
            .expect("send ok");

        // ---- After handshake ----
        match after {
            AfterHandshake::SendRecordThenClose(payload) => {
                let framed = write_json_record(payload.as_bytes());
                ws.send(Message::Text(String::from_utf8(framed).unwrap()))
                    .await
                    .expect("send record");
                ws.send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "test-done".into(),
                })))
                .await
                .expect("send close");
            }
            AfterHandshake::KeepOpen => {
                // Drain frames until the client closes; that's the
                // signal we should stop.
                while let Some(Ok(_)) = ws.next().await {}
            }
        }
    });

    addr
}

/// Open a plain ws connection to the loopback server. Used by the
/// tests below to bypass the wss-only enforcement that
/// `run_until_shutdown` deliberately applies to production configs.
async fn open_plain_ws(
    addr: std::net::SocketAddr,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{addr}/hubs/agent");
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    ws
}

#[tokio::test]
async fn handshake_round_trip_against_loopback_server() {
    let addr = spawn_loopback_server(AfterHandshake::KeepOpen).await;
    let mut ws = open_plain_ws(addr).await;
    perform_handshake(&mut ws, HubProtocol::Json)
        .await
        .expect("handshake");
}

#[tokio::test]
async fn session_surfaces_inbound_record_then_exits_on_server_close() {
    // Server sends a single Heartbeat invocation (well-formed
    // SignalR JSON) then closes the connection. The session
    // driver should hand us the record, then return
    // `SessionExit::Reconnect` because the WebSocket-level Close
    // is reconnect-eligible (no allowReconnect=false envelope).
    let payload = r#"{"type":1,"target":"Heartbeat","arguments":[]}"#;
    let addr = spawn_loopback_server(AfterHandshake::SendRecordThenClose(payload)).await;
    let mut ws = open_plain_ws(addr).await;
    perform_handshake(&mut ws, HubProtocol::Json)
        .await
        .expect("handshake");

    let (_tx, mut rx) = watch::channel(false);
    let mut received: Vec<Vec<u8>> = Vec::new();
    let exit = run_session(ws, HubProtocol::Json, &mut rx, |rec| received.push(rec))
        .await
        .expect("session");

    assert!(
        matches!(exit, SessionExit::Reconnect { .. }),
        "got {exit:?}"
    );
    assert_eq!(received.len(), 1);
    let got: serde_json::Value = serde_json::from_slice(&received[0]).unwrap();
    assert_eq!(got["type"], HubMessageKind::Invocation as u8);
    assert_eq!(got["target"], "Heartbeat");
}

#[tokio::test]
async fn session_exits_cleanly_on_local_shutdown() {
    let addr = spawn_loopback_server(AfterHandshake::KeepOpen).await;
    let mut ws = open_plain_ws(addr).await;
    perform_handshake(&mut ws, HubProtocol::Json)
        .await
        .expect("handshake");

    let (tx, mut rx) = watch::channel(false);

    // Flip shutdown to true after a short delay; the session must
    // observe it and return `LocalShutdown`.
    let shutdown_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = tx.send(true);
    });

    let exit = run_session(ws, HubProtocol::Json, &mut rx, |_| {})
        .await
        .expect("session");
    assert_eq!(exit, SessionExit::LocalShutdown);

    shutdown_handle.await.unwrap();
}

#[tokio::test]
async fn handshake_rejects_a_bad_handshake_response() {
    // A server that replies with `{"error":"protocol_version_unsupported"}`
    // must cause `perform_handshake` to surface a typed error.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        // Wait for the agent's handshake request, then reply with
        // an error response.
        let _ = ws.next().await;
        let resp = serde_json::to_vec(&HandshakeResponse::rejected("protocol_version_unsupported"))
            .unwrap();
        let framed = write_json_record(&resp);
        ws.send(Message::Text(String::from_utf8(framed).unwrap()))
            .await
            .unwrap();
    });

    let mut ws = open_plain_ws(addr).await;
    let err = perform_handshake(&mut ws, HubProtocol::Json)
        .await
        .expect_err("handshake should be rejected");
    match err {
        SessionError::HandshakeRejected(reason) => {
            assert_eq!(reason, "protocol_version_unsupported");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn ping_interval_leaves_room_for_one_lost_ping_before_idle_timeout() {
    // The spec sets PING_INTERVAL = 15 s and IDLE_TIMEOUT = 30 s
    // so that one lost ping is recoverable: we ping at +15 s, the
    // pong is dropped, and we still have 15 s before the deadline
    // fires. Encoding that *relationship* here catches a future
    // tweak that breaks the recovery margin.
    assert!(
        PING_INTERVAL * 2 <= IDLE_TIMEOUT,
        "ping interval {:?} must be <= half of idle timeout {:?}",
        PING_INTERVAL,
        IDLE_TIMEOUT
    );
}
