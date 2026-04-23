// Source: CMRemote, clean-room implementation.
//
// Fuzz target: ConnectionInfo JSON parser.
//
// The ConnectionInfo struct is read from disk on every agent start and
// is the first attack surface a malicious local file (or a corrupted
// config written by a buggy deploy) would touch. The parser must never
// panic on any byte string; every divergence from that invariant is an
// availability bug for the agent.
//
// Roadmap reference: Track S / S4 — Fuzzing and parser hardening.

#![no_main]

use cmremote_wire::ConnectionInfo;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The on-disk format is UTF-8 JSON. Non-UTF-8 input is simply not
    // a meaningful ConnectionInfo; we still exercise it through the
    // MessagePack codec path in a sibling target.
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(info) = serde_json::from_str::<ConnectionInfo>(s) {
            // If the input parses, both `validate` and the redacting
            // `Debug` must stay panic-free on the resulting value.
            let _ = info.validate();
            let _ = format!("{info:?}");

            // Re-encoding must round-trip. A divergence here would be
            // a codec bug and is worth a corpus entry.
            let s2 = serde_json::to_string(&info).expect("re-encode");
            let info2: ConnectionInfo = serde_json::from_str(&s2).expect("re-decode");
            assert_eq!(info, info2, "re-encode round-trip diverged");
        }
    }
});
