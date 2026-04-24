// Source: CMRemote, clean-room implementation.
//
// Conformance tests for the shared wire-protocol corpus under
// `docs/wire-protocol-vectors/`. Both the .NET and Rust agents are
// required to round-trip these files; this file is the Rust half.

use std::path::{Path, PathBuf};

use cmremote_wire::{
    from_msgpack, to_msgpack, ChangeWindowsSessionRequest, ConnectionInfo, DesktopTransportResult,
    HandshakeRequest, HandshakeResponse, HubClose, HubCompletion, HubInvocation, HubPing,
    HubProtocol, IceCandidate, InvokeCtrlAltDelRequest, RemoteControlSessionRequest,
    RestartScreenCasterRequest, SdpAnswer, SdpKind, SdpOffer,
};

/// Walk up from the crate's manifest directory until we find the
/// `docs/wire-protocol-vectors` folder. The corpus has exactly one
/// home in the repo, so locating it deterministically is part of the
/// contract documented in the corpus README.
fn vectors_root() -> PathBuf {
    let mut cur: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    loop {
        let candidate = cur.join("docs").join("wire-protocol-vectors");
        if candidate.is_dir() {
            return candidate;
        }
        if !cur.pop() {
            panic!(
                "could not find docs/wire-protocol-vectors above {}",
                env!("CARGO_MANIFEST_DIR")
            );
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn connection_info_valid_vectors_parse_and_validate() {
    let dir = vectors_root().join("connection-info").join("valid");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("valid dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = read(&path);
        let info: ConnectionInfo = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: deserialize: {e}", path.display()));
        info.validate()
            .unwrap_or_else(|e| panic!("{}: validate: {e}", path.display()));
        // Round-trip preserves PascalCase field names.
        let re = serde_json::to_string(&info).unwrap();
        assert!(
            re.contains("\"DeviceID\""),
            "{}: serialised form lost DeviceID",
            path.display()
        );
        count += 1;
    }
    assert!(
        count >= 2,
        "expected at least 2 valid vectors, found {count}"
    );
}

#[test]
fn connection_info_invalid_vectors_are_rejected() {
    let dir = vectors_root().join("connection-info").join("invalid");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("invalid dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = read(&path);
        // The invalid corpus is structurally still JSON; rejection
        // happens at `validate()` time, not at deserialization.
        let info: ConnectionInfo = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: deserialize: {e}", path.display()));
        assert!(
            info.validate().is_err(),
            "{}: vector was expected to be rejected by validate()",
            path.display()
        );
        count += 1;
    }
    assert!(
        count >= 3,
        "expected at least 3 invalid vectors, found {count}"
    );
}

#[test]
fn handshake_request_round_trips() {
    let raw = read(&vectors_root().join("handshake").join("agent-request.json"));
    // Generic-shape sanity check (preserves the previous assertion).
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["protocol"], "json");
    assert_eq!(v["version"], 1);

    // Typed round-trip pins the wire shape to the public Rust type
    // every transport now consumes (slice R2).
    let req: HandshakeRequest = serde_json::from_str(&raw).unwrap();
    assert_eq!(req.protocol, HubProtocol::Json);
    assert_eq!(req.version, 1);
    let re = serde_json::to_string(&req).unwrap();
    assert!(re.contains("\"protocol\":\"json\""));
}

#[test]
fn handshake_server_ok_is_empty_object() {
    let raw = read(&vectors_root().join("handshake").join("server-ok.json"));
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v.is_object());
    assert_eq!(v.as_object().unwrap().len(), 0);

    let resp: HandshakeResponse = serde_json::from_str(&raw).unwrap();
    assert!(resp.is_ok());
    assert_eq!(serde_json::to_string(&resp).unwrap(), "{}");
}

#[test]
fn handshake_server_error_carries_error_field() {
    let raw = read(&vectors_root().join("handshake").join("server-error.json"));
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v["error"].is_string());

    let resp: HandshakeResponse = serde_json::from_str(&raw).unwrap();
    assert!(!resp.is_ok());
    assert_eq!(resp.error.as_deref(), Some("protocol_version_unsupported"));
}

#[test]
fn invocation_vectors_round_trip() {
    let dir = vectors_root().join("envelope");

    let hb_raw = read(&dir.join("invocation-heartbeat.json"));
    let hb: HubInvocation = serde_json::from_str(&hb_raw).unwrap();
    assert_eq!(hb.kind, 1);
    assert_eq!(hb.invocation_id.as_deref(), Some("7"));
    assert_eq!(hb.target, "Heartbeat");
    assert!(hb.arguments.is_empty());

    let faf_raw = read(&dir.join("invocation-fire-and-forget.json"));
    let faf: HubInvocation = serde_json::from_str(&faf_raw).unwrap();
    assert_eq!(faf.kind, 1);
    assert!(
        faf.invocation_id.is_none(),
        "fire-and-forget vector must not carry invocationId"
    );
    let re = serde_json::to_string(&faf).unwrap();
    assert!(
        !re.contains("invocationId"),
        "fire-and-forget round-trip leaked invocationId: {re}"
    );
}

#[test]
fn completion_vectors_round_trip_and_validate() {
    let dir = vectors_root().join("envelope");

    let ok: HubCompletion = serde_json::from_str(&read(&dir.join("completion-ok.json"))).unwrap();
    assert_eq!(ok.kind, 3);
    assert_eq!(ok.invocation_id, "7");
    assert!(ok.error.is_none());
    ok.validate().expect("ok completion validates");

    let err: HubCompletion =
        serde_json::from_str(&read(&dir.join("completion-error.json"))).unwrap();
    assert_eq!(err.kind, 3);
    assert_eq!(err.error.as_deref(), Some("invalid_arguments"));
    err.validate().expect("error completion validates");
}

#[test]
fn ping_vector_round_trips() {
    let raw = read(&vectors_root().join("envelope").join("ping.json"));
    let p: HubPing = serde_json::from_str(&raw).unwrap();
    assert_eq!(p.kind, 6);
    let re = serde_json::to_string(&p).unwrap();
    assert_eq!(re, r#"{"type":6}"#);
}

#[test]
fn close_vectors_round_trip_with_correct_reconnect_flag() {
    let dir = vectors_root().join("envelope");

    let shutdown: HubClose = serde_json::from_str(&read(&dir.join("close-shutdown.json"))).unwrap();
    assert_eq!(shutdown.kind, 7);
    assert!(shutdown.allow_reconnect);
    assert_eq!(shutdown.error.as_deref(), Some("server_shutting_down"));

    let quarantine: HubClose =
        serde_json::from_str(&read(&dir.join("close-quarantine.json"))).unwrap();
    assert!(
        !quarantine.allow_reconnect,
        "quarantine vector must forbid reconnect"
    );
    assert_eq!(quarantine.error.as_deref(), Some("agent_quarantined"));
}

// -------------------------------------------------------------------------
// Slice R1b — MessagePack codec conformance.
//
// Every JSON vector in the corpus must also round-trip through the
// MessagePack codec: JSON → T → MessagePack bytes → T → MessagePack bytes
// (byte-stable re-encode) → T (decoded from the re-encode equals the
// original). Cross-encoding equivalence is the contract slice R2 relies
// on when it negotiates either transport on the WebSocket.
// -------------------------------------------------------------------------

/// Generic JSON ↔ MessagePack round-trip for a single vector file.
///
/// Panics with the vector path on any divergence so a corpus regression
/// is easy to attribute.
fn assert_msgpack_round_trip<T>(path: &Path)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let raw = read(path);
    let from_json: T = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{}: json decode: {e}", path.display()));

    let bytes = to_msgpack(&from_json)
        .unwrap_or_else(|e| panic!("{}: msgpack encode: {e}", path.display()));
    let from_mp: T =
        from_msgpack(&bytes).unwrap_or_else(|e| panic!("{}: msgpack decode: {e}", path.display()));
    assert_eq!(
        from_json,
        from_mp,
        "{}: json and msgpack decodes diverge",
        path.display()
    );

    // Byte-stable re-encode: encoding the decoded value produces the
    // exact same bytes as the first encode. This pins the codec's
    // determinism, which slice R2 relies on for replay tests.
    let bytes2 = to_msgpack(&from_mp)
        .unwrap_or_else(|e| panic!("{}: msgpack re-encode: {e}", path.display()));
    assert_eq!(
        bytes,
        bytes2,
        "{}: msgpack re-encode is not byte-stable",
        path.display()
    );
}

#[test]
fn connection_info_valid_vectors_round_trip_through_msgpack() {
    let dir = vectors_root().join("connection-info").join("valid");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("valid dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        assert_msgpack_round_trip::<ConnectionInfo>(&path);
        count += 1;
    }
    assert!(count >= 2, "expected ≥2 valid vectors, found {count}");
}

#[test]
fn envelope_vectors_round_trip_through_msgpack() {
    let dir = vectors_root().join("envelope");

    assert_msgpack_round_trip::<HubInvocation>(&dir.join("invocation-heartbeat.json"));
    assert_msgpack_round_trip::<HubInvocation>(&dir.join("invocation-fire-and-forget.json"));
    assert_msgpack_round_trip::<HubCompletion>(&dir.join("completion-ok.json"));
    assert_msgpack_round_trip::<HubCompletion>(&dir.join("completion-error.json"));
    assert_msgpack_round_trip::<HubPing>(&dir.join("ping.json"));
    assert_msgpack_round_trip::<HubClose>(&dir.join("close-shutdown.json"));
    assert_msgpack_round_trip::<HubClose>(&dir.join("close-quarantine.json"));
}

// -------------------------------------------------------------------------
// Slice R7.d — Method-surface vectors for the four desktop-transport hub
// methods. Each request / result vector must round-trip through both
// JSON and MessagePack, and the deserialised value must keep its
// PascalCase wire field names on re-serialisation so the .NET hub can
// drive either agent fleet without a contract bump.
// -------------------------------------------------------------------------

const DESKTOP_VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";
const DESKTOP_VALID_ORG_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

#[test]
fn method_surface_remote_control_request_vector_round_trips() {
    let path = vectors_root()
        .join("method-surface")
        .join("remote-control")
        .join("request.json");
    let raw = read(&path);
    let req: RemoteControlSessionRequest = serde_json::from_str(&raw).unwrap();
    assert_eq!(req.session_id, DESKTOP_VALID_SESSION_ID);
    assert_eq!(req.org_id, DESKTOP_VALID_ORG_ID);
    let re = serde_json::to_string(&req).unwrap();
    // PascalCase must survive the round-trip — this is the contract
    // with the .NET hub.
    assert!(re.contains("\"SessionId\""), "{re}");
    assert!(re.contains("\"AccessKey\""), "{re}");
    assert!(re.contains("\"OrgId\""), "{re}");
    assert_msgpack_round_trip::<RemoteControlSessionRequest>(&path);
}

#[test]
fn method_surface_remote_control_result_vectors_round_trip() {
    let dir = vectors_root().join("method-surface").join("remote-control");

    let ok: DesktopTransportResult =
        serde_json::from_str(&read(&dir.join("result-success.json"))).unwrap();
    assert!(ok.success);
    assert_eq!(ok.session_id, DESKTOP_VALID_SESSION_ID);
    // ErrorMessage is omitted from the on-wire form when None.
    let re = serde_json::to_string(&ok).unwrap();
    assert!(!re.contains("ErrorMessage"), "{re}");

    let bad: DesktopTransportResult =
        serde_json::from_str(&read(&dir.join("result-failure.json"))).unwrap();
    assert!(!bad.success);
    let msg = bad.error_message.as_deref().unwrap();
    assert!(msg.contains("RemoteControl"), "{msg}");
    // The result MUST NOT echo any access key — the failure-vector
    // file was authored with this in mind, but the round-trip
    // assertion pins it against future drift.
    assert!(!msg.contains("REDACTED-IN-LOGS"), "{msg}");

    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-success.json"));
    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-failure.json"));
}

#[test]
fn method_surface_restart_screen_caster_vectors_round_trip() {
    let dir = vectors_root()
        .join("method-surface")
        .join("restart-screen-caster");

    let req: RestartScreenCasterRequest =
        serde_json::from_str(&read(&dir.join("request.json"))).unwrap();
    assert_eq!(req.session_id, DESKTOP_VALID_SESSION_ID);
    assert_eq!(req.viewer_ids.len(), 2);
    let re = serde_json::to_string(&req).unwrap();
    assert!(re.contains("\"ViewerIds\""), "{re}");

    assert_msgpack_round_trip::<RestartScreenCasterRequest>(&dir.join("request.json"));
    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-success.json"));
}

#[test]
fn method_surface_change_windows_session_vectors_round_trip() {
    let dir = vectors_root()
        .join("method-surface")
        .join("change-windows-session");

    let req: ChangeWindowsSessionRequest =
        serde_json::from_str(&read(&dir.join("request.json"))).unwrap();
    assert_eq!(req.session_id, DESKTOP_VALID_SESSION_ID);
    assert_eq!(req.target_session_id, 1);
    let re = serde_json::to_string(&req).unwrap();
    assert!(re.contains("\"TargetSessionId\":1"), "{re}");

    assert_msgpack_round_trip::<ChangeWindowsSessionRequest>(&dir.join("request.json"));
    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-success.json"));
}

#[test]
fn method_surface_invoke_ctrl_alt_del_vectors_round_trip() {
    let dir = vectors_root()
        .join("method-surface")
        .join("invoke-ctrl-alt-del");

    // Request is a unit struct → JSON `null`.
    let raw = read(&dir.join("request.json"));
    assert_eq!(raw.trim(), "null");
    let _: InvokeCtrlAltDelRequest = serde_json::from_str(&raw).unwrap();

    let bad: DesktopTransportResult =
        serde_json::from_str(&read(&dir.join("result-failure.json"))).unwrap();
    assert!(!bad.success);
    // No session id in the request type → the result echoes empty.
    assert!(bad.session_id.is_empty());
    assert!(bad
        .error_message
        .as_deref()
        .unwrap()
        .contains("InvokeCtrlAltDel"));

    assert_msgpack_round_trip::<InvokeCtrlAltDelRequest>(&dir.join("request.json"));
    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-failure.json"));
}

// -------------------------------------------------------------------------
// Slice R7.g — Signalling vectors. The .NET side will land its half of
// the WebRTC negotiation surface against these byte-stable JSON shapes,
// so each vector must round-trip through both JSON and MessagePack and
// keep its PascalCase wire field names intact on re-serialisation.
// -------------------------------------------------------------------------

#[test]
fn signalling_sdp_offer_vector_round_trips() {
    let path = vectors_root()
        .join("method-surface")
        .join("signalling")
        .join("sdp-offer.json");
    let raw = read(&path);
    let req: SdpOffer = serde_json::from_str(&raw).unwrap();
    assert_eq!(req.session_id, DESKTOP_VALID_SESSION_ID);
    assert_eq!(req.org_id, DESKTOP_VALID_ORG_ID);
    assert_eq!(req.kind, SdpKind::Offer);
    assert!(req.sdp.starts_with("v=0\r\n"), "{}", req.sdp);
    let re = serde_json::to_string(&req).unwrap();
    // PascalCase must survive — this is the contract with the .NET hub.
    assert!(re.contains("\"ViewerConnectionId\""), "{re}");
    assert!(re.contains("\"SessionId\""), "{re}");
    assert!(re.contains("\"OrgId\""), "{re}");
    assert!(re.contains("\"Kind\":\"Offer\""), "{re}");
    assert!(re.contains("\"Sdp\""), "{re}");
    assert_msgpack_round_trip::<SdpOffer>(&path);
}

#[test]
fn signalling_sdp_answer_vector_round_trips() {
    let path = vectors_root()
        .join("method-surface")
        .join("signalling")
        .join("sdp-answer.json");
    let raw = read(&path);
    let req: SdpAnswer = serde_json::from_str(&raw).unwrap();
    assert_eq!(req.session_id, DESKTOP_VALID_SESSION_ID);
    assert_eq!(req.kind, SdpKind::Answer);
    let re = serde_json::to_string(&req).unwrap();
    assert!(re.contains("\"Kind\":\"Answer\""), "{re}");
    assert_msgpack_round_trip::<SdpAnswer>(&path);
}

#[test]
fn signalling_ice_candidate_vector_round_trips() {
    let dir = vectors_root().join("method-surface").join("signalling");

    let path = dir.join("ice-candidate.json");
    let req: IceCandidate = serde_json::from_str(&read(&path)).unwrap();
    assert_eq!(req.session_id, DESKTOP_VALID_SESSION_ID);
    assert_eq!(req.sdp_mid.as_deref(), Some("0"));
    assert_eq!(req.sdp_mline_index, Some(0));
    let re = serde_json::to_string(&req).unwrap();
    assert!(re.contains("\"Candidate\""), "{re}");
    assert!(re.contains("\"SdpMid\":\"0\""), "{re}");
    assert!(re.contains("\"SdpMlineIndex\":0"), "{re}");
    assert_msgpack_round_trip::<IceCandidate>(&path);
}

#[test]
fn signalling_ice_candidate_end_of_candidates_marker_vector_round_trips() {
    let path = vectors_root()
        .join("method-surface")
        .join("signalling")
        .join("ice-candidate-end-of-candidates.json");
    let raw = read(&path);
    let req: IceCandidate = serde_json::from_str(&raw).unwrap();
    // RFC 8838 end-of-candidates: empty `candidate` line, no mid, no
    // mline index. The .NET side detects the marker by absence of a
    // value rather than by comparing against an empty string, so the
    // round-trip must preserve `null` for both Option fields.
    assert_eq!(req.candidate, "");
    assert!(req.sdp_mid.is_none());
    assert!(req.sdp_mline_index.is_none());
    let re = serde_json::to_string(&req).unwrap();
    assert!(re.contains("\"SdpMid\":null"), "{re}");
    assert!(re.contains("\"SdpMlineIndex\":null"), "{re}");
    assert_msgpack_round_trip::<IceCandidate>(&path);
}

#[test]
fn signalling_result_vectors_round_trip() {
    let dir = vectors_root().join("method-surface").join("signalling");

    let ok: DesktopTransportResult =
        serde_json::from_str(&read(&dir.join("result-success.json"))).unwrap();
    assert!(ok.success);
    assert_eq!(ok.session_id, DESKTOP_VALID_SESSION_ID);
    let re = serde_json::to_string(&ok).unwrap();
    // ErrorMessage is omitted from the on-wire form when None.
    assert!(!re.contains("ErrorMessage"), "{re}");

    let bad: DesktopTransportResult =
        serde_json::from_str(&read(&dir.join("result-failure.json"))).unwrap();
    assert!(!bad.success);
    let msg = bad.error_message.as_deref().unwrap();
    assert!(msg.contains("SendSdpOffer"), "{msg}");
    // The failure vector deliberately mirrors the stub's
    // OS-not-supported message; pin that it never carries a
    // placeholder access key (no `RemoteControl` request shape would
    // have one in this method, but pin it anyway against future
    // drift in result-shape conventions).
    assert!(!msg.contains("REDACTED-IN-LOGS"), "{msg}");

    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-success.json"));
    assert_msgpack_round_trip::<DesktopTransportResult>(&dir.join("result-failure.json"));
}
