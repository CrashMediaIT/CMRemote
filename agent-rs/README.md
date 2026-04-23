# CMRemote Rust Agent (`agent-rs`)

> **Status:** R0 scaffold — workspace layout, configuration loader,
> structured logging, signal handling, and CI plumbing only. **No
> network I/O is implemented yet.** Slice R1 (wire types + test
> vectors) and R2 (connection / heartbeat loop) are the next planned
> work items. See `ROADMAP.md` ➜ *Rust agent slice-by-slice delivery
> plan* for the full sequence.

This is the clean-room Rust re-implementation of the CMRemote endpoint
agent (Module 2b of the separation track). It will eventually replace
the .NET agent under [`Agent/`](../Agent), but for now the two ship
side-by-side and the Rust agent is opt-in per device via the
`agent-channel` setting.

## Why Rust

See `ROADMAP.md` ➜ *Approved language and project-shape decisions* for
the full rationale. In short: a single static binary in the low-MB
range, no in-process PowerShell SDK attack surface, and a type system
that lets us encode the package-install job state machine
(`Queued → Running → Success | Failed | Cancelled`) at compile time.

## Layout

```
agent-rs/
├── Cargo.toml                  # workspace manifest
├── rust-toolchain.toml         # pin stable Rust
├── crates/
│   ├── cmremote-wire/          # wire DTOs (clean-room, derived from docs/wire-protocol.md)
│   ├── cmremote-platform/      # OS abstraction traits
│   └── cmremote-agent/         # binary: config, logging, runtime
└── README.md
```

## Build

```sh
# from the repository root
cd agent-rs
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

CI runs all four commands on every push (see
`.github/workflows/rust.yml`).

## Run (R0 scaffold behaviour)

```sh
# Start a no-op agent that loads ./ConnectionInfo.json and waits
# for SIGINT / SIGTERM.
cargo run -p cmremote-agent -- \
    --host https://cmremote.example.com \
    --organization org-1 \
    --device dev-test
```

## Provenance

Every source file under `agent-rs/` carries the `// Source: CMRemote,
clean-room implementation.` header. Third-party crates pulled in via
`Cargo.toml` are upstream packages — their licences are tracked in
`THIRD_PARTY_NOTICES.md` at the repository root.
