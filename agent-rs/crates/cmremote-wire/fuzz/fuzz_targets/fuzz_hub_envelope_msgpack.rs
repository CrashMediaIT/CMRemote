// Source: CMRemote, clean-room implementation.
//
// Fuzz target: hub-envelope MessagePack parser.
//
// The MessagePack codec (slice R1b) is the second supported transport
// for the hub protocol. It shares the struct shapes with the JSON
// target but exercises a completely different parser — `rmp-serde`'s
// binary decoder — so crashes found here generally do *not* overlap
// with the JSON corpus. The fuzzer is also free to feed `ConnectionInfo`
// bytes through this decoder since the spec permits either codec on the
// WebSocket.
//
// Roadmap reference: Track S / S4 — Fuzzing and parser hardening.

#![no_main]

use cmremote_wire::{
    from_msgpack, to_msgpack, ConnectionInfo, HubClose, HubCompletion, HubInvocation, HubPing,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `from_msgpack` MUST return `Err` on garbage; it must never panic.
    if let Ok(info) = from_msgpack::<ConnectionInfo>(data) {
        // Re-encoding must be byte-stable (the determinism property
        // that slice R2 relies on when replaying captured traffic).
        let reenc = to_msgpack(&info).expect("re-encode must succeed for a decoded value");
        let info2: ConnectionInfo = from_msgpack(&reenc).expect("re-decode must succeed");
        assert_eq!(info, info2, "connection_info msgpack re-encode diverged");
    }

    if let Ok(inv) = from_msgpack::<HubInvocation>(data) {
        let reenc = to_msgpack(&inv).expect("re-encode must succeed for a decoded value");
        let inv2: HubInvocation = from_msgpack(&reenc).expect("re-decode must succeed");
        assert_eq!(inv, inv2, "invocation msgpack re-encode diverged");
    }

    if let Ok(comp) = from_msgpack::<HubCompletion>(data) {
        let _ = comp.validate();
    }

    let _ = from_msgpack::<HubPing>(data);
    let _ = from_msgpack::<HubClose>(data);
});
