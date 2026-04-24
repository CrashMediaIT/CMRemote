# CMRemote Agent ↔ Server Wire Protocol

> **Status:** Module 0 — *frozen for slice R1*. This document is the
> authoritative specification of the agent ↔ server wire protocol.
> Both the .NET reference implementation under `Agent/` and the
> clean-room Rust re-implementation under `agent-rs/` are obliged to
> conform to it. When the implementations and this document disagree,
> the document wins and the implementations get bug reports.
>
> This is a clean-room specification — no upstream source was consulted
> while writing it. Where the wire format is observably constrained by
> the SignalR hub protocol the server already speaks, that constraint
> is documented as such, with a reference to the public SignalR
> protocol documentation rather than to any specific upstream code.
>
> **Companion test-vector corpus:** [`wire-protocol-vectors/`](./wire-protocol-vectors/).
> Every section that pins a wire shape ends with a *Vectors:* line
> naming the corpus files that pin it. CI fails if the implementations
> stop round-tripping the corpus.

## Goals

1. Pin every byte that crosses the wire so the Rust agent and the
   .NET agent are interchangeable from the server's perspective.
2. Allow versioned, backwards-compatible evolution: every connection
   carries a `protocolVersion` integer and the server is required to
   reject mismatches with a structured error rather than silently
   misbehaving.
3. Keep the spec testable: every section ends with the name of the
   *test-vector file* that pins it (under
   [`docs/wire-protocol-vectors/`](./wire-protocol-vectors/)).
4. **Be conservative on the security boundary.** The agent runs
   privileged on every endpoint 24/7; the wire is the principal
   attack surface. Section [Security model](#security-model) is
   normative, not aspirational, and any change to it requires an
   explicit roadmap entry.

## Security model

The agent is a privileged process on the endpoint. Anything the
server can convince the agent to do is, in effect, an unattended
arbitrary-code primitive on every device in the fleet. The wire
protocol therefore makes the following **mandatory** guarantees.

### Transport

- The only supported transport is **WebSocket over TLS** (`wss://`).
  Plain `ws://` is rejected by the agent at connect time and is
  never negotiated, even on `localhost`. The legacy long-polling
  fallback in the SignalR client is **not** implemented and never
  will be.
- TLS floor: **TLS 1.2 minimum, TLS 1.3 preferred.** The agent uses
  the platform certificate store; pinning is out of scope for v1
  but is reserved for a future `protocolVersion` bump.
- The agent **must** validate the server certificate against the
  system trust store. Disabling certificate validation is not a
  supported configuration; there is no flag for it and the
  implementations must not add one.
- Outbound connections only. The agent never opens a listening
  socket as part of the hub protocol.

### Authentication and identity

- Every WebSocket upgrade request carries:
  - `Authorization: Bearer <organization-token>` — issued by the
    server when the device first enrolls in an organisation.
  - `X-Device-Id: <uuid-v4>` — the persistent per-device id from
    `ConnectionInfo.json`.
  - `X-Protocol-Version: 1` — the wire-protocol version this client
    speaks. The server rejects with HTTP `426 Upgrade Required` and
    a JSON body `{"error":"protocol_version_unsupported","minimum":N,"maximum":M}`
    if it cannot speak the requested version.
- The bearer token is treated as a **secret**: it is never logged,
  never echoed in error messages, and never serialised into
  diagnostics bundles. The Rust agent enforces this by giving
  `ConnectionInfo` a hand-written `Debug` impl that redacts the
  token (see `cmremote-wire::ConnectionInfo`).
- The server replies to the upgrade with a `Sec-WebSocket-Protocol`
  header naming the negotiated hub-protocol encoding (`json` or
  `messagepack`). The agent **must** treat any other value as a
  protocol violation and close the connection with code `1002`.
- After the WebSocket is open, the SignalR handshake (see
  [Hub protocol](#hub-protocol) below) is the *only* control
  message accepted before authentication-bound business methods
  may be invoked.

### Server identity verification

After the first successful authenticated connect, the server
issues a `ServerVerificationToken` to the agent. The agent persists
it in `ConnectionInfo.json` and presents it back on every
subsequent connect as the `X-Server-Verification` header. A server
that does not recognise its own token must reject the connection
with `401`. This protects against rogue-server takeover scenarios
where an attacker briefly stands up a CMRemote server that knows
the org token but not the per-device verification token.

### On-disk secret hygiene

- `ConnectionInfo.json` is written with file mode `0600` on POSIX
  and the equivalent ACL (owner-only) on Windows. The agent
  refuses to load a `ConnectionInfo.json` that is group- or
  world-readable on POSIX and emits a structured warning when the
  ACL on Windows grants read to anyone other than the service
  account or `SYSTEM`.
- No bearer token, verification token, or installer secret is
  permitted in process command-line arguments. CLI flags accept
  *paths* to secret material, not the secret material itself.
- The agent never writes secrets to its rolling log files. The
  `tracing` filter strips known-sensitive fields before any sink
  sees them.

### Input validation

- The agent applies the same DTO schema validation to inbound
  invocations that the server applies to outbound ones. A method
  whose arguments do not match its declared schema is rejected
  with `Completion { error: "invalid_arguments" }` and the
  invocation is **not** retried.
- All shell-out paths in the agent (script execution, package
  install, MSI install) take an `argv` array — never a joined
  command string. The wire DTOs reflect that: arguments are
  always `Vec<String>` / `string[]`, never a single `string`.
- Filenames received from the server are sanitised through
  `Shared::MsiFileValidator` (PR C1) before being used as
  filesystem paths.
- `protocolVersion`, `invocationId`, and method names are
  validated as a strict allow-list: unknown method names are
  rejected before argument parsing begins.

### Replay and ordering

- `invocationId` is unique within a connection. The agent must
  reject a duplicate `invocationId` from the same connection with
  `Completion { error: "duplicate_invocation" }`.
- The protocol is **not** replay-safe across reconnects on its
  own. State that must survive a reconnect (job state machines,
  installed-applications snapshots) is reconciled at the service
  layer using server-issued, server-stored identifiers, not by
  the agent re-playing local intent. This is a deliberate design
  choice that keeps the wire stateless.

### Failure modes that are *not* the wire's job

- Authorisation (which operator may dispatch which job to which
  device) is enforced server-side and is *not* a wire concern.
  The wire carries only "the server told the agent to do X";
  whether the operator was allowed to ask is decided before the
  invocation is queued.
- Org-scope checks happen on the server in `AgentHub` and
  `CircuitConnection`. The agent assumes any invocation that
  reaches it has already passed those checks.

**Vectors:** none — this section pins behaviour, not byte layout.
The behaviours above are exercised by the integration tests in
`Tests/` and by `agent-rs/crates/cmremote-wire/tests/`.

## Transport

- WebSocket over TLS, endpoint path **`/hubs/agent`** on the
  CMRemote server.
- Per-message text frames carry JSON; per-message binary frames
  carry MessagePack. Mixing encodings on a single connection is
  not supported.
- Each SignalR record is terminated by the ASCII record separator
  byte **`0x1E`**, as required by the SignalR JSON hub-protocol
  framing. MessagePack records are length-prefixed per the
  SignalR MessagePack hub-protocol framing.

## Hub protocol

CMRemote uses ASP.NET Core SignalR's hub protocol, which is documented
publicly at <https://github.com/dotnet/aspnetcore/blob/main/src/SignalR/docs/specs/HubProtocol.md>.
We support exactly two encodings:

- **`json`** — newline-record-separator-framed JSON. Preferred for
  development and for humans reading captures.
- **`messagepack`** — preferred in production for size and speed.

The Rust agent negotiates `messagepack` by default and falls back
to `json` only if the server explicitly rejects it (for example,
older servers built against an older SignalR build).

### Handshake

The first record sent by the agent after the WebSocket opens is
the SignalR handshake request:

```json
{"protocol":"json","version":1}
```

terminated by `0x1E`. The server replies with:

```json
{}
```

on success (also terminated by `0x1E`), or with
`{"error":"<reason>"}` on failure. After a failed handshake the
agent closes the WebSocket with code `1002` and surfaces the
reason in its log.

**Vectors:**
[`handshake/agent-request.json`](./wire-protocol-vectors/handshake/agent-request.json),
[`handshake/server-ok.json`](./wire-protocol-vectors/handshake/server-ok.json),
[`handshake/server-error.json`](./wire-protocol-vectors/handshake/server-error.json).

### Message-type discriminator

Mirrors the SignalR spec:

| Value | Name           | Direction       |
|-------|----------------|-----------------|
| 1     | Invocation     | both            |
| 2     | StreamItem     | both            |
| 3     | Completion     | both            |
| 4     | StreamInvocation | client → server |
| 5     | CancelInvocation | client → server |
| 6     | Ping           | both            |
| 7     | Close          | server → client |

CMRemote's agent does not currently use `StreamInvocation` or
`CancelInvocation`; both are reserved for slice R7 (desktop
transport) and may be sent by the server, but the agent will
respond with `Completion { error: "not_implemented" }` until
that slice ships.

### Envelope shapes (JSON)

#### Invocation (type 1)

```json
{"type":1,"invocationId":"7","target":"Heartbeat","arguments":[]}
```

Fields:

- `type` — always `1`.
- `invocationId` — caller-chosen, unique within the connection.
  Omitted for fire-and-forget invocations (in which case no
  `Completion` is expected).
- `target` — hub method name. Must match the allow-list in
  [Method surface](#method-surface) verbatim, including case.
- `arguments` — positional argument array. Empty arrays are
  serialised as `[]`, not omitted.

**Vectors:**
[`envelope/invocation-heartbeat.json`](./wire-protocol-vectors/envelope/invocation-heartbeat.json),
[`envelope/invocation-fire-and-forget.json`](./wire-protocol-vectors/envelope/invocation-fire-and-forget.json).

#### Completion (type 3)

```json
{"type":3,"invocationId":"7","result":null}
```

or, on error:

```json
{"type":3,"invocationId":"7","error":"invalid_arguments"}
```

Exactly one of `result` and `error` is present. The agent must
treat a `Completion` carrying both fields as a protocol violation
and close the connection with code `1002`.

**Vectors:**
[`envelope/completion-ok.json`](./wire-protocol-vectors/envelope/completion-ok.json),
[`envelope/completion-error.json`](./wire-protocol-vectors/envelope/completion-error.json).

#### Ping (type 6)

```json
{"type":6}
```

Sent by either side every 15 s of idle time. A peer that does not
receive any frame (ping, invocation, or otherwise) for 30 s
closes the connection with code `1011` and reconnects with the
backoff schedule below.

**Vectors:**
[`envelope/ping.json`](./wire-protocol-vectors/envelope/ping.json).

#### Close (type 7, server → client only)

```json
{"type":7,"error":"server_shutting_down","allowReconnect":true}
```

`allowReconnect` defaults to `true` if absent. The agent must
respect `allowReconnect: false` by not attempting to reconnect
until the operator restarts it; this is the wire's mechanism for
permanently quarantining a misbehaving agent.

**Vectors:**
[`envelope/close-shutdown.json`](./wire-protocol-vectors/envelope/close-shutdown.json),
[`envelope/close-quarantine.json`](./wire-protocol-vectors/envelope/close-quarantine.json).

## Bootstrap configuration (`ConnectionInfo.json`)

The agent reads its bootstrap configuration from
`ConnectionInfo.json` in its working directory. Keys are
PascalCase (historic; preserved to allow in-place upgrade from
the .NET agent):

| Field                     | Type            | Required | Notes                                                                          |
|---------------------------|-----------------|----------|--------------------------------------------------------------------------------|
| `DeviceID`                | UUID v4 string  | yes      | Generated by the agent on first run if absent.                                 |
| `Host`                    | URL             | yes      | Trailing `/` is stripped on load. `https://` only.                             |
| `OrganizationID`          | string          | yes      | Tenant identifier issued by the CMRemote server.                               |
| `ServerVerificationToken` | opaque string   | no       | Issued by the server on first successful connect.                              |
| `OrganizationToken`       | opaque string   | no       | Bearer token sent as `Authorization: Bearer <…>` on the WebSocket upgrade. Optional: a freshly-deployed agent may enrol without one and the server hands one back on first connect. Treated as a secret (redacted from `Debug` output). |

CLI overrides (`--host`, `--organization`, `--device`, `--config`)
take precedence over file values, matching the legacy behaviour.
**Secret material is never accepted on the command line** —
`ServerVerificationToken` and `OrganizationToken` are file-only.

`ConnectionInfo.Debug` redaction: implementations must guarantee
that `ServerVerificationToken` and `OrganizationToken` are never
written to logs or panic output. The Rust crate enforces this with
a hand-written `Debug` impl pinned by `debug_redacts_*` tests in
`cmremote-wire`.

**Vectors:**
[`connection-info/valid/full.json`](./wire-protocol-vectors/connection-info/valid/full.json),
[`connection-info/valid/no-token.json`](./wire-protocol-vectors/connection-info/valid/no-token.json),
[`connection-info/invalid/missing-host.json`](./wire-protocol-vectors/connection-info/invalid/missing-host.json),
[`connection-info/invalid/missing-org.json`](./wire-protocol-vectors/connection-info/invalid/missing-org.json),
[`connection-info/invalid/blank-device.json`](./wire-protocol-vectors/connection-info/invalid/blank-device.json).

## Connection lifecycle

1. **TLS connect** to `wss://<host>/hubs/agent` carrying the
   headers from [Authentication and identity](#authentication-and-identity).
2. **Handshake** — agent sends the SignalR handshake, server
   replies. A non-empty error in the reply terminates the
   connection.
3. **Steady state** — bidirectional `Invocation`, `StreamItem`,
   `Completion`, and `Ping` messages flow until either side sends
   `Close` or the underlying socket dies.
4. **Reconnect** — on any non-quarantine close, the agent
   reconnects with **jittered exponential backoff**: base 1 s,
   factor 2, cap 60 s, full jitter. The reconnect counter resets
   on a successful handshake.
5. **Idle disconnect** — 30 s without an inbound frame causes the
   agent to close with code `1011` and reconnect.

**Vectors:** behavioural, exercised by `agent-rs` integration
tests in slice R2.

## Method surface

Hub method names (`SendDeviceInfo`, `Heartbeat`, `InstallPackage`,
`UninstallApp`, …) are enumerated and frozen in slice R2a as the
*Agent contract freeze* deliverable. Until R2a lands, the
following invariants apply to every method on the surface:

- Method names are **PascalCase**, ASCII-only, and case-sensitive.
- Argument arrays are **positional**; no named-argument forms.
- `result` payloads are JSON objects (never bare scalars) so
  fields can be added without bumping `protocolVersion`.
- Server → agent invocations always carry an `invocationId`
  unless the method is explicitly fire-and-forget. The current
  fire-and-forget set is empty; new entries require a roadmap
  note.
- Agent → server invocations may omit `invocationId` for
  telemetry-style methods (`Heartbeat`, `Log`).

R2a will add a row per method with: argument schema, completion
schema, security expectations (org-scope, idempotency, allowed
states), and a corresponding test-vector file.

## Versioning

`protocolVersion` is currently **1**. Breaking changes increment
it; additive changes do not. The server is required to advertise
the highest version it supports in its handshake response and to
reject clients more than one major version behind with the
`426 Upgrade Required` flow described above.
