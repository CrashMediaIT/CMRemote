// Source: CMRemote, clean-room implementation.
//
// Fuzz target: hub-envelope JSON parser.
//
// The envelope types are the primary attack surface at runtime — every
// frame the agent receives on the hub transport is decoded as one of
// these. A panic here would be a remote-triggerable denial of service
// for the entire agent process.
//
// Each kind is exercised in turn so a single input stresses all four
// variants and the dispatch code that would route between them in
// slice R2.
//
// Roadmap reference: Track S / S4 — Fuzzing and parser hardening.

#![no_main]

use cmremote_wire::{HubClose, HubCompletion, HubInvocation, HubPing};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(inv) = serde_json::from_str::<HubInvocation>(s) {
        // Re-encode + redecode must be lossless.
        if let Ok(reenc) = serde_json::to_string(&inv) {
            let inv2: HubInvocation =
                serde_json::from_str(&reenc).expect("re-decode of re-encoded value must succeed");
            assert_eq!(inv, inv2, "invocation re-encode round-trip diverged");
        }
    }

    if let Ok(comp) = serde_json::from_str::<HubCompletion>(s) {
        // `validate` enforces the mutual-exclusion rule (result XOR error);
        // the fuzzer is free to find inputs that violate it, and the rule
        // must be reported via Err, never via panic.
        let _ = comp.validate();
    }

    let _ = serde_json::from_str::<HubPing>(s);
    let _ = serde_json::from_str::<HubClose>(s);
});
