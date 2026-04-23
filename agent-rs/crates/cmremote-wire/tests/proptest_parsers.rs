// Source: CMRemote, clean-room implementation.
//
// Property-based parser tests for the wire layer. Roadmap reference:
// Track S / S4 — Fuzzing and parser hardening.
//
// `proptest` drives two complementary properties:
//
//   1. **Round-trip stability** — for every well-formed value of a wire
//      type, `decode(encode(v)) == v`, and the second encode is
//      byte-stable against the first. This holds independently on both
//      the JSON and MessagePack codecs and is the contract slice R2
//      relies on when it negotiates either transport on the WebSocket.
//
//   2. **Parser robustness** — for arbitrary byte strings, the decoders
//      must *never* panic. They must return a `Result::Err` through
//      `WireError` so that the connection loop (slice R2) can log +
//      drop the frame rather than aborting the process. A crash here
//      would be a denial-of-service vector for a malicious server.
//
// `cargo-fuzz` (in `crates/cmremote-wire/fuzz/`) covers the same
// surfaces with coverage-guided inputs on nightly. The two approaches
// are intentional: proptest catches obvious counter-examples fast on
// every PR, and the fuzzer grinds deeper overnight.

use cmremote_wire::{
    from_msgpack, to_msgpack, ConnectionInfo, HubClose, HubCompletion, HubInvocation, HubPing,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Generators.
//
// These intentionally produce shapes that are *valid inputs to the
// encoders*, not "any" wire value. The encode-then-decode invariant
// only makes sense starting from a value the encoder can emit.
// ---------------------------------------------------------------------------

/// Conservative ASCII identifier suitable for `DeviceID` / `OrganizationID`.
/// Limited to printable ASCII so that the generator does not accidentally
/// construct values that the legacy .NET agent's stricter validator would
/// reject on the other side of the wire.
fn ascii_token() -> impl Strategy<Value = String> {
    "[A-Za-z0-9_\\-]{1,32}"
}

/// Optional ASCII token, used for fields the spec declares as nullable.
fn opt_ascii() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), ascii_token().prop_map(Some)]
}

fn host_url() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        "https://[a-z]{1,12}\\.example\\.com(/[a-z0-9]{0,8}){0,2}".prop_map(Some),
    ]
}

fn arb_connection_info() -> impl Strategy<Value = ConnectionInfo> {
    (ascii_token(), host_url(), opt_ascii(), opt_ascii()).prop_map(
        |(device_id, host, organization_id, server_verification_token)| ConnectionInfo {
            device_id,
            host,
            organization_id,
            server_verification_token,
        },
    )
}

/// A JSON value we are prepared to ship as an invocation argument or
/// completion result. We keep this strictly first-order (no nested
/// arrays or objects) so the generator stays cheap; recursive shapes
/// are covered by the coverage-guided fuzzer.
fn arb_simple_json() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i32>().prop_map(|i| serde_json::Value::Number(i.into())),
        "[ -~]{0,32}".prop_map(serde_json::Value::String),
    ]
}

/// Same as [`arb_simple_json`] but excludes JSON `null`.
///
/// serde intentionally folds `null` into `None` when deserializing
/// into `Option<T>`, so `Some(Value::Null)` is not a stable value
/// inside fields like [`HubCompletion::result`]. That folding is a
/// documented serde behaviour, not a parser bug — we simply avoid
/// generating inputs that would round-trip through it.
fn arb_nonnull_json() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i32>().prop_map(|i| serde_json::Value::Number(i.into())),
        "[ -~]{0,32}".prop_map(serde_json::Value::String),
    ]
}

fn arb_hub_invocation() -> impl Strategy<Value = HubInvocation> {
    (
        opt_ascii(),
        ascii_token(),
        prop::collection::vec(arb_simple_json(), 0..4),
    )
        .prop_map(|(invocation_id, target, arguments)| HubInvocation {
            kind: 1,
            invocation_id,
            target,
            arguments,
        })
}

fn arb_hub_completion() -> impl Strategy<Value = HubCompletion> {
    // Mutually exclusive: either a result *or* an error, never both —
    // the validator enforces this, and we must not generate inputs
    // that would fail `validate()`.
    let ok = (ascii_token(), arb_nonnull_json()).prop_map(|(invocation_id, value)| HubCompletion {
        kind: 3,
        invocation_id,
        result: Some(value),
        error: None,
    });
    let err = (ascii_token(), "[ -~]{0,64}").prop_map(|(invocation_id, msg)| HubCompletion {
        kind: 3,
        invocation_id,
        result: None,
        error: Some(msg),
    });
    let void = ascii_token().prop_map(|invocation_id| HubCompletion {
        kind: 3,
        invocation_id,
        result: None,
        error: None,
    });
    prop_oneof![ok, err, void]
}

fn arb_hub_close() -> impl Strategy<Value = HubClose> {
    (prop::option::of("[ -~]{0,64}"), any::<bool>()).prop_map(|(error, allow_reconnect)| HubClose {
        kind: 7,
        error,
        allow_reconnect,
    })
}

// ---------------------------------------------------------------------------
// Round-trip + byte-stability properties.
// ---------------------------------------------------------------------------

/// Shared round-trip invariant: JSON encode → decode → equal value,
/// and re-encoding the decoded value yields identical bytes.
fn assert_json_round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let s = serde_json::to_string(value).expect("json encode");
    let back: T = serde_json::from_str(&s).expect("json decode");
    prop_assert_eq_helper(&back, value, "json round-trip diverged");
    let s2 = serde_json::to_string(&back).expect("json re-encode");
    prop_assert_eq_helper(&s, &s2, "json re-encode is not byte-stable");
}

/// Shared round-trip invariant: MessagePack encode → decode → equal
/// value, and re-encoding the decoded value yields byte-identical bytes.
fn assert_msgpack_round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let bytes = to_msgpack(value).expect("msgpack encode");
    let back: T = from_msgpack(&bytes).expect("msgpack decode");
    prop_assert_eq_helper(&back, value, "msgpack round-trip diverged");
    let bytes2 = to_msgpack(&back).expect("msgpack re-encode");
    prop_assert_eq_helper(&bytes, &bytes2, "msgpack re-encode is not byte-stable");
}

/// Helper that turns `assert_eq!`-style failures inside proptest
/// generators into readable panics. `proptest` already formats the
/// shrunk counterexample; this just adds a tag.
fn prop_assert_eq_helper<T: PartialEq + std::fmt::Debug>(left: &T, right: &T, tag: &str) {
    assert_eq!(left, right, "{tag}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn connection_info_round_trips_json(info in arb_connection_info()) {
        assert_json_round_trip(&info);
    }

    #[test]
    fn connection_info_round_trips_msgpack(info in arb_connection_info()) {
        assert_msgpack_round_trip(&info);
    }

    #[test]
    fn connection_info_debug_never_leaks_token(info in arb_connection_info()) {
        // Security invariant pinned by docs/wire-protocol.md → Security
        // model → On-disk secret hygiene. Regardless of what non-empty
        // token the struct holds, the redacting Debug must not print it.
        //
        // We require the generated token to contain a non-ASCII-letter
        // marker character so it is guaranteed not to be a substring of
        // the fixed `<redacted>` placeholder that `Debug` writes.
        let tagged_token = info.server_verification_token.as_ref().map(|t| {
            // Prefix guarantees the token is not a substring of "<redacted>".
            format!("SVT_{}#", t)
        });
        let tagged = ConnectionInfo {
            server_verification_token: tagged_token.clone(),
            ..info
        };
        let rendered = format!("{tagged:?}");
        if let Some(tok) = &tagged_token {
            prop_assert!(
                !rendered.contains(tok.as_str()),
                "Debug output leaked verification token: {rendered}"
            );
            prop_assert!(
                rendered.contains("<redacted>"),
                "Debug output should mark the token as redacted: {rendered}"
            );
        }
    }

    #[test]
    fn hub_invocation_round_trips_json(inv in arb_hub_invocation()) {
        assert_json_round_trip(&inv);
    }

    #[test]
    fn hub_invocation_round_trips_msgpack(inv in arb_hub_invocation()) {
        assert_msgpack_round_trip(&inv);
    }

    #[test]
    fn hub_completion_round_trips_json(comp in arb_hub_completion()) {
        comp.validate().expect("generator must only produce valid completions");
        assert_json_round_trip(&comp);
    }

    #[test]
    fn hub_completion_round_trips_msgpack(comp in arb_hub_completion()) {
        comp.validate().expect("generator must only produce valid completions");
        assert_msgpack_round_trip(&comp);
    }

    #[test]
    fn hub_close_round_trips_json(close in arb_hub_close()) {
        assert_json_round_trip(&close);
    }

    #[test]
    fn hub_close_round_trips_msgpack(close in arb_hub_close()) {
        assert_msgpack_round_trip(&close);
    }

    // -------------------------------------------------------------------
    // Parser robustness. No crashes on arbitrary bytes.
    //
    // These are the cheap, always-on guardrails that match the nightly
    // cargo-fuzz targets. If one of these ever fails the build, the
    // same byte string should be dropped into
    // `fuzz/corpus/<target>/` as a regression seed before the fix
    // lands.
    // -------------------------------------------------------------------

    #[test]
    fn json_connection_info_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        // We only care that the decoder does not panic. A successful
        // parse is fine (and means the validator will be exercised
        // below); a failure is also fine.
        if let Ok(s) = std::str::from_utf8(&bytes) {
            match serde_json::from_str::<ConnectionInfo>(s) {
                Ok(info) => {
                    // `validate` is called on every real load path;
                    // verify it, too, is panic-free on arbitrary input.
                    let _ = info.validate();
                    let _ = format!("{info:?}");
                }
                Err(_) => { /* expected for most random input */ }
            }
        }
    }

    #[test]
    fn json_envelope_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        if let Ok(s) = std::str::from_utf8(&bytes) {
            let _ = serde_json::from_str::<HubInvocation>(s);
            let _ = serde_json::from_str::<HubCompletion>(s);
            let _ = serde_json::from_str::<HubPing>(s);
            let _ = serde_json::from_str::<HubClose>(s);
        }
    }

    #[test]
    fn msgpack_decoders_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = from_msgpack::<ConnectionInfo>(&bytes);
        let _ = from_msgpack::<HubInvocation>(&bytes);
        let _ = from_msgpack::<HubCompletion>(&bytes);
        let _ = from_msgpack::<HubPing>(&bytes);
        let _ = from_msgpack::<HubClose>(&bytes);
    }
}

// `HubPing` is a zero-field type, so there is nothing to randomise; a
// single pinned round-trip test keeps it in the same file as the other
// envelope properties.
#[test]
fn ping_round_trips_both_codecs() {
    let p = HubPing::new();
    assert_json_round_trip(&p);
    assert_msgpack_round_trip(&p);
}
