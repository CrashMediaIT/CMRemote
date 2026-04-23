// Source: CMRemote, clean-room implementation.
//
// Conformance tests for the shared wire-protocol corpus under
// `docs/wire-protocol-vectors/`. Both the .NET and Rust agents are
// required to round-trip these files; this file is the Rust half.

use std::path::{Path, PathBuf};

use cmremote_wire::{ConnectionInfo, HubClose, HubCompletion, HubInvocation, HubPing};

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
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["protocol"], "json");
    assert_eq!(v["version"], 1);
}

#[test]
fn handshake_server_ok_is_empty_object() {
    let raw = read(&vectors_root().join("handshake").join("server-ok.json"));
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v.is_object());
    assert_eq!(v.as_object().unwrap().len(), 0);
}

#[test]
fn handshake_server_error_carries_error_field() {
    let raw = read(&vectors_root().join("handshake").join("server-error.json"));
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v["error"].is_string());
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
