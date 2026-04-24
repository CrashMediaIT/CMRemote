# CMRemote Wire-Protocol Test Vectors

This directory is the **shared corpus** that pins the byte-level
shapes called out in [`../wire-protocol.md`](../wire-protocol.md).
Both the .NET reference implementation and the Rust clean-room
re-implementation are required to round-trip every file here.

> If you change a vector, you are changing the wire protocol.
> Update [`../wire-protocol.md`](../wire-protocol.md) in the same
> commit and bump `protocolVersion` if the change is breaking.

## Layout

```
wire-protocol-vectors/
├── connection-info/
│   ├── valid/        # ConnectionInfo.json files an agent must accept
│   └── invalid/      # ConnectionInfo.json files an agent must reject
├── handshake/        # SignalR handshake request/response records
├── envelope/         # Hub-message envelopes (invocation, completion, …)
└── method-surface/   # Per-method request + result DTOs (slice R7.d
                      # locks the four desktop-transport methods;
                      # slice R7.g adds the WebRTC signalling DTOs
                      # under method-surface/signalling/).
```

Files are intentionally one-shape-per-file so that test failures
point straight at the shape that drifted.

## Conventions

- All files are UTF-8 JSON with **no BOM**, **no trailing
  newline beyond a single `\n`**, and use **2-space indentation**.
  Whitespace is part of the contract for the human-facing files
  (`connection-info/`, `handshake/`); for the on-the-wire envelope
  files the wire form is the *minified* version. Tests MUST
  compare against the parsed value, not the raw bytes, except for
  vectors explicitly marked `*-canonical.json`.
- `invalid/` vectors carry a top-level `_reject_reason` comment
  field (prefixed with `_` so a permissive parser ignores it) so
  a human reading the failure can see why the implementation was
  expected to reject.
- New vectors require a corresponding spec section *and* a test
  in at least one implementation. CI guards the second part by
  failing if a vector is added that no test references.

## Who consumes this

- `agent-rs/crates/cmremote-wire/tests/vectors.rs` —
  Rust round-trip tests.
- `Tests/Server.Tests/WireProtocolVectorTests.cs` *(planned in
  slice R2a)* — .NET conformance tests.

Both runners locate this directory by walking up from
`CARGO_MANIFEST_DIR` / the test assembly directory until a
`docs/wire-protocol-vectors` folder is found, so the corpus has
exactly one home in the repo.
