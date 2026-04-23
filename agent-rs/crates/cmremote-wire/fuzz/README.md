# `cmremote-wire` fuzz targets

Coverage-guided fuzz targets for the `cmremote-wire` parsers. Part of
the CMRemote roadmap's **Track S / S4 — Fuzzing and parser hardening**.

## Why this crate is outside the main workspace

`cargo-fuzz` requires a **nightly** Rust toolchain and pulls in
`libfuzzer-sys`. We keep the `agent-rs/` workspace on stable Rust and
off the libfuzzer dependency tree, so this crate carries its own
`Cargo.toml` with an empty `[workspace]` table to opt out of the parent
workspace.

Day-to-day development runs the `proptest` suite under
[`crates/cmremote-wire/tests/proptest_parsers.rs`](../tests/proptest_parsers.rs)
instead; it covers the same invariants with random inputs on stable.
The fuzzers here grind much deeper overnight (see the scheduled
[`fuzz.yml`](../../../../.github/workflows/fuzz.yml) workflow).

## Targets

| Target | Surface | Invariants checked |
|---|---|---|
| `fuzz_connection_info_json` | `ConnectionInfo::deserialize` (serde_json) | No panic on any bytes; `validate()` + redacting `Debug` are panic-free on parsed values; re-encode is lossless. |
| `fuzz_hub_envelope_json` | `HubInvocation` / `HubCompletion` / `HubPing` / `HubClose` (serde_json) | No panic on any bytes; per-type re-encode is lossless when decode succeeds. |
| `fuzz_hub_envelope_msgpack` | All of the above via `rmp-serde` | No panic on any bytes; byte-stable re-encode for values that decode successfully. |

## Running locally

You need a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

From `agent-rs/crates/cmremote-wire/`:

```sh
# Seed each target with the on-disk corpus before the first run so the
# fuzzer starts from known-valid shapes (see "Corpus seeding" below).
cargo +nightly fuzz run fuzz_connection_info_json
cargo +nightly fuzz run fuzz_hub_envelope_json
cargo +nightly fuzz run fuzz_hub_envelope_msgpack
```

Each command runs forever until it finds a crash or you stop it with
Ctrl-C. For a bounded local run, use `-- -max_total_time=60` (libFuzzer
flag).

## Corpus seeding

The on-disk test-vector corpus at
[`docs/wire-protocol-vectors/`](../../../../docs/wire-protocol-vectors)
is the canonical source of known-good inputs. Both the CI workflow
and a local developer should copy these into `fuzz/corpus/<target>/`
before starting a run so the fuzzer has a non-empty seed queue:

```sh
# From agent-rs/crates/cmremote-wire/fuzz/ (run once; idempotent)
mkdir -p corpus/fuzz_connection_info_json \
         corpus/fuzz_hub_envelope_json \
         corpus/fuzz_hub_envelope_msgpack

cp ../../../../docs/wire-protocol-vectors/connection-info/valid/*.json \
   ../../../../docs/wire-protocol-vectors/connection-info/invalid/*.json \
   corpus/fuzz_connection_info_json/

cp ../../../../docs/wire-protocol-vectors/envelope/*.json \
   corpus/fuzz_hub_envelope_json/
cp ../../../../docs/wire-protocol-vectors/envelope/*.json \
   corpus/fuzz_hub_envelope_msgpack/
```

The fuzzer will then generate MessagePack inputs by mutating those
seeds; libFuzzer's byte-level mutators are happy to explore a different
encoding even from JSON seeds.

## Triaging a crash

1. Copy the crashing input out of `artifacts/<target>/` into a
   permanent regression location — the canonical spot is a new row in
   `docs/wire-protocol-vectors/` with its own fixture file.
2. Add a pinned unit test in
   `crates/cmremote-wire/src/<parser>.rs` or
   `crates/cmremote-wire/tests/vectors.rs` that decodes the same bytes
   and asserts the fixed behaviour (usually: parser returns `Err`, does
   not panic).
3. Fix the parser / codec. Land the fix in the same PR as the test so
   the regression is pinned before the fuzzer finds it again.
4. Leave the crash file in `corpus/<target>/` so libFuzzer uses it as
   a seed in subsequent runs.

## CI

The [`fuzz`](../../../../.github/workflows/fuzz.yml) workflow runs
nightly (and on manual dispatch). It does **not** block pull requests;
a crash opens a GitHub issue and uploads the minimised reproducer as a
workflow artifact so a human can triage it the next morning.
