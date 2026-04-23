<!-- Source: CMRemote, clean-room implementation. -->

# CMRemote threat model

**Status:** living document, revision 1 (April 2026).
**Roadmap reference:** [Track S / S3](../ROADMAP.md#-track-s--security--supply-chain-baseline-cross-cutting).
**Owners:** maintainers of the [`CrashMediaIT/CMRemote`](https://github.com/CrashMediaIT/CMRemote) repository.
**Review cadence:** reviewed at the start of every module rewrite listed
in the roadmap's [*Clean-room redesign / separation track*](../ROADMAP.md#clean-room-redesign--separation-track-lead-track),
and re-reviewed any time a trust boundary moves. Each review bumps the
revision number above and is recorded in the *Change log* at the bottom.

## What this document is (and is not)

This document is the *narrative, cross-surface* threat model for
CMRemote. It expands on the **normative** [*Security model* section of
`docs/wire-protocol.md`](wire-protocol.md#security-model), which pins
the wire-level guarantees that every implementation (agent, server,
tests, fuzz harnesses) must meet.

| Document                    | Normative? | Scope                                                             |
|-----------------------------|------------|-------------------------------------------------------------------|
| `docs/wire-protocol.md`     | ✅ Yes     | Byte layouts, envelope shapes, MUST/MUST-NOT on the wire.         |
| `docs/threat-model.md` (this) | ❌ No (advisory) | Surfaces, trust boundaries, STRIDE, non-goals, review cadence. |
| `SECURITY.md`               | ✅ Yes (policy) | How to report, what response to expect, safe-harbour.        |

Where this document and `wire-protocol.md` disagree, **`wire-protocol.md`
wins**. This document's job is to give reviewers a map of the system so
a security change in one surface is visible to the others; it is not
where behaviour is defined.

## System in one page

```text
                  ┌──────────────┐         (HTTPS, cookies)
                  │   Operator   │ ─────────────────────────────────────┐
                  │   browser    │                                      │
                  └──────────────┘                                      ▼
                                                   ┌────────────────────────────┐
  ┌────────┐     (WSS / signed-build HTTPS)        │  CMRemote server           │
  │ Agent  │ ───────────────────────────────────▶  │    ├── AgentHub (SignalR)  │
  │ (Rust  │ ◀───────────────────────────────────  │    ├── Razor pages / API   │
  │  / .NET)│                                      │    ├── Migration importer  │
  └────────┘                                       │    ├── Upload intake       │
       │                                           │    └── WebRTC signalling   │
       │ (WebRTC media/data over DTLS-SRTP,        └────────────────────────────┘
       │  brokered by signalling above)                     │            │
       ▼                                                    ▼            ▼
  ┌────────────┐                                    ┌──────────────┐  ┌──────────────┐
  │ Remote-    │                                    │ Application  │  │ Legacy DBs   │
  │ control    │                                    │ database     │  │ (SQLite /    │
  │ peer       │                                    │ (SQL Server) │  │  Postgres /  │
  │ (Desktop)  │                                    └──────────────┘  │  SQL Server) │
  └────────────┘                                                      └──────────────┘
```

Every arrow in this diagram is a trust boundary and gets its own row
in the STRIDE table below.

## Assets

The highest-value assets in the system, ordered by blast radius on
compromise:

1. **Any agent's execution context.** The agent runs as `SYSTEM` on
   Windows and `root` on Linux/macOS in the default configuration.
   Arbitrary-code execution in an agent context is arbitrary-code
   execution on an entire customer endpoint.
2. **The org-token ↔ device-id mapping on the server.** Forgery of
   this mapping lets an attacker impersonate an enrolled device and,
   with other bugs, receive jobs intended for a real device.
3. **Operator credentials and session cookies.** Operators can
   dispatch jobs to every device they can see.
4. **Uploaded MSIs and scripts.** They become arbitrary-code on every
   targeted device. Provenance is entirely the server's responsibility.
5. **Bootstrap secrets at rest.** `ConnectionInfo.json`'s
   `ServerVerificationToken` and the org bearer token are what an
   attacker needs to pivot from "I have filesystem read on a device"
   to "I can impersonate this device".
6. **Legacy-database contents during migration.** A malicious or
   corrupted legacy DB file is an attacker-controlled parser input
   that reaches `CMRemote.Migration.Legacy` (roadmap **M2**).

## Adversaries we consider

| Adversary                        | Capabilities assumed                                                     | In scope? |
|----------------------------------|--------------------------------------------------------------------------|-----------|
| **Network attacker (on-path)**   | Full read/write on the TLS channel metadata, cannot break TLS 1.2+.      | ✅ Yes |
| **Rogue server (takeover)**      | Temporarily stands up a CMRemote server that knows the org token.       | ✅ Yes |
| **Compromised operator account** | Has valid cookies, no server-admin. Limited to operator authz.           | ✅ Yes |
| **Malicious tenant operator**    | Legitimate operator of tenant A, attempts to reach tenant B's devices.   | ✅ Yes |
| **Malicious uploaded MSI**       | Arbitrary bytes uploaded through the MSI intake path.                    | ✅ Yes |
| **Malicious legacy DB file**     | Arbitrary bytes in a SQLite/Postgres/SQL-Server file fed to the importer.| ✅ Yes |
| **Local unprivileged user (endpoint)** | Can read world-readable files, cannot escalate without another bug. | ✅ Yes |
| **Local administrator (endpoint)** | Already has `SYSTEM`/`root`. Can replace the agent binary.              | ❌ Out of scope — see *Non-goals*. |
| **Server-host root**             | Already has the DB, the signing key, and every cookie.                   | ❌ Out of scope. |
| **Supply-chain adversary (upstream dep)** | Publishes a malicious crate/nuget update.                       | ✅ Yes — mitigations in Track S / S2. |
| **Well-funded state actor with a TLS 1.3 break** | …                                                           | ❌ Out of scope. |

## Surfaces and trust boundaries

Each subsection below is one surface. The STRIDE table lists the
categories that are *live* on that surface — "N/A" means the category
does not meaningfully apply, not that it is "handled elsewhere". The
**Validation side** column is the thing we want reviewers to check on
every PR that touches the surface: *which side of the boundary is
responsible for enforcing this invariant?*

### S-1 — Agent ↔ Server hub (`/hubs/agent`, WSS)

**Boundary:** the endpoint device trusts the server only after
bearer-token + server-verification-token validation; the server trusts
the agent only for telemetry claims about itself, never for
authorisation decisions.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | Rogue server stands up with a stolen org token. | Per-device `ServerVerificationToken` issued on first successful connect and required on every reconnect (`wire-protocol.md` §*Server identity verification*). | Agent rejects if server can't echo its own token; server rejects unknown tokens with `401`. |
| **T**ampering | On-path attacker mutates frames. | TLS 1.2 floor, 1.3 preferred; `wss://` only; cert validation mandatory with no disable flag. | Agent verifies via platform trust store; server MUST present a valid chain. |
| **R**epudiation | Agent denies having received a job. | Server-issued `invocationId` is logged server-side with outcome; agent ships structured logs of its own completions. | Server is the system of record. Agent logs are advisory. |
| **I**nformation disclosure | Bearer / verification tokens appear in logs, panics, or diagnostics bundles. | `ConnectionInfo` has a hand-written redacting `Debug` impl (pinned by unit test in `cmremote-wire::connection_info::tests::debug_redacts_server_verification_token`). `tracing` filters strip sensitive fields before any sink. | Agent side: redaction. Server side: standard ASP.NET log scrubbing. |
| **D**oS | Agent flooded with invalid frames. | Strict allow-list validation *before* argument parsing; unknown `target` is a protocol violation (close code `1002`), not an ignored message. | Both sides: bail out on the first structural violation; never retry. |
| **E**oP | Arbitrary code on the endpoint via an unsanitised shell-out. | Every shell-out takes an `argv` array — never a joined command string. MSI filenames go through `Shared::MsiFileValidator`. | Agent side. |

**Open items / known gaps:**

- Certificate **pinning** is intentionally out of scope for protocol v1
  (see `wire-protocol.md` §*Transport*). An attacker who compromises
  a system CA that is trusted by the endpoint can MITM a fresh
  enrolment. Re-enrolment after the first successful connect is
  protected by the verification token.
- The agent's reconnect loop (slice R2) does not yet apply an
  exponential-backoff jitter across the fleet. Worth tracking under
  R2 so a server restart does not produce a synchronised stampede.

### S-2 — Server ↔ Application database

**Boundary:** the server is the sole writer; no agent or operator has
direct DB access. The DB is assumed to be on a trusted network
segment.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | Unauthorised process connects to the DB. | DB creds live in server config (or secret-store) only; least-privilege DB user per environment. | DB side (GRANTs); server side (connection string handling). |
| **T**ampering | SQL injection through a poorly-parameterised query. | All queries use EF Core parameterised LINQ or explicitly parameterised raw SQL; no string concatenation into `FromSqlRaw`. | Server side. Reviewable by grep for `FromSqlRaw` / `ExecuteSqlRaw`. |
| **R**epudiation | Admin action lacks an audit trail. | Operator-initiated mutations are logged via the existing audit-log path; migration importer (M2) writes an `ImportRun` row. | Server side. |
| **I**nformation disclosure | DB backup leaks tokens. | Tokens stored hashed where possible; where a token must be stored reversibly (e.g. org token echoed to agents), column-level protection is the operator's responsibility. | Server + operator runbook. |
| **D**oS | Unbounded query from a crafted filter. | Every list endpoint paginates; query timeouts configured on the DbContext. | Server side. |
| **E**oP | DB user has more permissions than the app needs. | Documented least-privilege user; migrations run as a separate, higher-privileged user out-of-band. | Operator runbook. |

### S-3 — Server ↔ Operator browser (Razor / Blazor circuits + cookies)

**Boundary:** the browser is never trusted for authorisation decisions;
it is trusted only for the identity claims baked into its cookie after
server-side login.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | Operator session hijack via stolen cookie. | `HttpOnly`, `Secure`, `SameSite=Strict` on the auth cookie; short session lifetime; re-auth for sensitive actions. | Server side. |
| **T**ampering | CSRF against state-changing endpoints. | Antiforgery tokens on every non-GET form; Blazor circuits are origin-bound. | Server side. |
| **R**epudiation | Operator denies having run a destructive job. | All job dispatches are audit-logged with operator id, device id, timestamp. | Server side. |
| **I**nformation disclosure | XSS leaks cookies or tokens. | Razor auto-encodes by default; any `@Html.Raw` / `MarkupString` usage requires a reviewer sign-off. CSP header disallows inline scripts where feasible. | Server side. |
| **D**oS | Blazor circuit exhaustion from an authenticated client. | Server caps circuits per user; idle circuits are torn down. | Server side. |
| **E**oP | Tenant-A operator reaches Tenant-B devices. | Every query that touches a device filters by the operator's org-scope; no endpoint accepts a client-supplied `OrgId` without re-checking it against the authenticated identity. | Server side. Reviewable by grep for direct `DeviceId` queries without an `OrgId` constraint. |

### S-4 — Migration importer ↔ legacy databases (roadmap M2)

**Boundary:** the legacy DB file is **attacker-controlled input**. A
CMRemote operator is expected to point the importer at a DB they
believe is theirs, but the file itself must be parsed with the same
paranoia as any internet-sourced blob.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | A legacy DB file claims to belong to a different org. | The import flow binds the import run to an authenticated operator's org; org-id fields in the source are ignored except as hints. | Importer (server-side). |
| **T**ampering | Malicious rows trigger SQL-injection-like bugs in the importer's own DB writes. | The importer reads the legacy file via parameterised EF providers only; no string-concatenated SQL against the target DB. | Server side. |
| **R**epudiation | Unclear what was imported and by whom. | Each import run writes an `ImportRun` row with operator, timestamp, source hash, and per-row outcome. | Server side. |
| **I**nformation disclosure | Legacy DB parser errors leak internal paths / env vars. | Parser errors are wrapped in a stable `ImportError` type before hitting the operator UI; raw exception text is logged server-side only. | Server side. |
| **D**oS | Zip-bomb / billion-laughs legacy file. | Size cap on uploaded files; streaming reader with per-row timeout; any single row over N bytes aborts the run. | Server side. |
| **E**oP | Legacy file drives arbitrary code in a dependency (e.g. a sqlite extension, a `.dll` loader). | `CMRemote.Migration.Legacy` loads SQLite with extensions disabled; Postgres / SQL Server connections go through the standard ADO.NET providers with `Integrated Security=false` and no `LOAD` permissions. | Server side. |

### S-5 — Agent-upgrade pipeline (signed-build fetch)

**Boundary:** the agent is asked to replace its own binary. This is
the highest-risk primitive in the entire system — compromise here is
fleet-wide arbitrary code with a single push.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | Attacker serves a malicious binary from a lookalike URL. | The upgrade descriptor names a **signed artefact**; the agent verifies the signature against a bundled public key (planned, roadmap **S5**) before executing the artefact. Download URL alone is never trusted. | Agent side. |
| **T**ampering | On-path attacker mutates bytes in transit. | HTTPS + signature check; the signature pins the exact bytes, not just the URL or the version. | Agent side. |
| **R**epudiation | Agent denies having been upgraded. | Upgrade outcomes are reported back to the server and recorded in the device history. | Server side (system of record). |
| **I**nformation disclosure | Upgrade metadata reveals internal build paths. | Build metadata emitted by CI is curated; no symbol files on the public artefact URL. | CI side (roadmap **S5** / SBOM work). |
| **D**oS | A bad upgrade bricks the fleet. | Staged rollout (per-device opt-in cohorts); agent keeps the previous binary and rolls back on startup failure within N minutes. | Agent + server (rollout policy server-side, roll-back agent-side). |
| **E**oP | This surface *is* EoP if it fails — see rows above. | Belt-and-suspenders: signature, staged rollout, roll-back, audit log. | All of the above. |

**Open items / known gaps:**

- The signing key management story is not finalised; roadmap **S5**
  covers SBOM + signing and is a hard prerequisite for the agent
  upgrade pipeline going live.
- Roll-back on Linux depends on being able to re-exec the previous
  binary even after a failed `systemd` reload. Track under R3.

### S-6 — Uploaded-MSI intake (`PR C1`)

**Boundary:** the operator can upload an arbitrary file; the server
stores it and may push it to any number of devices for silent install.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | File claims to be an MSI but isn't. | `Shared::MsiFileValidator` verifies the MSI structured-storage header before the file is accepted. | Server side (intake) + agent side (before invoking `msiexec`). |
| **T**ampering | File is mutated between upload and install. | Server stores the content-addressed hash; agents fetch by hash and verify on download. | Both sides. |
| **R**epudiation | Operator denies having uploaded. | Upload audit-logged with operator + hash + timestamp. | Server side. |
| **I**nformation disclosure | Filename leaks an internal path on the operator's machine. | Intake strips everything but the basename; sanitised name is used on the agent side. | Server side. |
| **D**oS | Multi-GB upload exhausts server disk. | Per-file and per-operator quotas, streamed upload with back-pressure. | Server side. |
| **E**oP | MSI runs arbitrary custom actions as `SYSTEM`. | This is **inherent** to how MSI works and is accepted risk — an operator who can upload an MSI can already run code on their own targeted devices. Documented in *Non-goals*. Mitigated by org-scope gating and audit logs, not by content inspection. | Operator authz (server) + operator trust. |

### S-7 — Desktop remote-control transport (WebRTC)

**Boundary:** two peers (operator browser and the agent on a device)
exchange media and data over a DTLS-SRTP session that the server only
**brokers**; the server does not see the media plaintext.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | Attacker joins the WebRTC session as the wrong peer. | Signalling runs over the authenticated hub; SDP fingerprints are exchanged only through that authenticated channel. | Server side (signalling authz). |
| **T**ampering | Media is mutated on-path. | DTLS-SRTP provides confidentiality + integrity for both media and data-channel traffic. | Both peers. |
| **R**epudiation | Operator denies having taken a remote session. | Session start / stop audit-logged server-side; optional session recording subject to operator policy. | Server side. |
| **I**nformation disclosure | TURN relay sees media. | TURN is used only for fallback NAT traversal; media is end-to-end encrypted regardless of relay path. | Both peers. |
| **D**oS | Unbounded data-channel traffic exhausts the agent. | Data-channel message-size cap; rate-limit per session. | Agent side. |
| **E**oP | Data-channel command surface lets an operator escape the "remote screen" boundary into the agent's general command set. | The data-channel exposes only the already-gated `StartRemoteSession` / `StopRemoteSession` / input-event methods; adding new methods is a wire-protocol change (version bump) and therefore goes through this doc's review cadence. | Agent side. |

### S-8 — Supply chain (third-party dependencies)

**Boundary:** every `Cargo.toml`, every `*.csproj`, every GitHub Action,
and every base image is a trust delegation to an upstream maintainer.

| STRIDE | Threat | Mitigation | Validation side |
|--------|--------|------------|-----------------|
| **S**poofing | Typo-squat crate / package. | `cargo-deny` `sources` allow-list pins to `crates.io`; `deny` list bans known-problematic crates (`openssl-sys`, `ring`). NuGet + Docker resolved through trusted registries. | CI (roadmap **S2**). |
| **T**ampering | Malicious version of a legitimate dep. | Lockfiles (`Cargo.lock` committed; .NET `packages.lock.json` pending per Track S / S2). Dependabot surfaces upgrades weekly with a separate security-update stream. | CI + reviewers. |
| **R**epudiation | N/A — upstream publishers operate independently of us. | — | — |
| **I**nformation disclosure | Dep's telemetry phones home. | Out of scope per *Non-goals* (we run CI in a public-network context anyway). Flagged at dep-add time if noticed. | Reviewers. |
| **D**oS | Vulnerable dep causes a crash that grounds the fleet. | `cargo-audit` + GitHub `dependency-review` on every PR; weekly scheduled sweep against `main`; CodeQL workflow (roadmap **S6**). | CI. |
| **E**oP | Compromised build action gets secrets. | Workflows use `permissions: read-all` by default and explicitly widen only where needed; OSSF Scorecard (roadmap **S2**) monitors the posture. | CI. |

## Trust-boundary cheatsheet

A reviewer looking at a PR should be able to answer two questions for
every changed file on a security-sensitive surface:

1. **Which boundary does this code sit on?** (S-1 through S-8 above.)
2. **Which side of that boundary is responsible for the invariants the
   PR touches?** (The "Validation side" column.)

If either answer is unclear, the PR description should say so, and the
relevant STRIDE row should be updated in this document as part of the
same PR.

## Non-goals

CMRemote does **not** defend against:

1. **An attacker who already has administrative privilege on an
   endpoint.** A local `Administrators` user on Windows or `root` on
   Linux/macOS can replace the agent binary, read `ConnectionInfo.json`
   (it is mode `0600` but `root` bypasses that), and generally do
   anything the agent can. The agent's job is to protect its trust
   boundary from *remote* attackers, not from local ones who already
   have the same privileges.
2. **An attacker with root on the server host.** That attacker owns the
   DB, the signing key (once S5 lands), every operator cookie, and the
   ability to push an arbitrary upgrade. No cryptographic posture we
   can adopt changes that.
3. **Operators abusing legitimate authority.** An operator authorised
   to push MSIs to Tenant A's devices can push *any* MSI to Tenant A's
   devices. Audit logs exist to detect abuse; this document does not
   claim to prevent it.
4. **Breaking of TLS 1.2+ by a state-level adversary.** We adopt
   industry-standard TLS hygiene and no more.
5. **Side-channel attacks against the agent (CPU, timing, power).**
   Out of scope for a userspace remote-management agent.
6. **Availability under sustained DoS from an already-authenticated
   agent.** An attacker who has enrolled a device can tie up resources;
   we rate-limit but do not attempt perfect isolation. See `SECURITY.md`
   §*Scope*.
7. **Data confidentiality against the host OS.** Secrets written to the
   device filesystem are protected by OS ACLs; if the OS is compromised
   so are they.

## Assumptions

The mitigations above rely on the following holding true. If any stops
being true, the relevant STRIDE row is re-opened.

- Endpoint OS file-permission primitives (`chmod 0600`, NTFS ACLs) work
  as documented. The agent enforces them but does not audit the OS
  itself.
- TLS 1.2+ against a system-trusted CA is cryptographically sound.
- Committed lockfiles accurately reflect the tree that CI builds.
- The `rustls` + `aws-lc-rs` (or equivalent) crypto provider chain in
  the Rust agent is maintained upstream.
- The SignalR MessagePack + JSON hub protocols are bug-compatible
  between the server's .NET implementation and the agent's Rust
  implementation, pinned by the corpus under
  [`docs/wire-protocol-vectors/`](wire-protocol-vectors/) and the
  tests in `agent-rs/crates/cmremote-wire/tests/vectors.rs`.

## Review cadence

- **Every module rewrite** listed in the roadmap's *Clean-room redesign
  / separation track* is gated on this document being re-read and, if
  needed, amended. A PR that implements a module without a visible
  threat-model review is a reviewer push-back.
- **Any change to a trust boundary** (new surface, boundary moves,
  validation side changes) is an amendment to this document as part of
  the same PR.
- **Quarterly**, the owners walk the STRIDE tables against the issue
  tracker to confirm nothing has silently drifted.

## Change log

| Revision | Date       | Change                                                                       |
|----------|------------|------------------------------------------------------------------------------|
| 1        | 2026-04-23 | Initial version. Covers surfaces S-1 through S-8; closes roadmap item **S3**. |

## See also

- [`docs/wire-protocol.md`](wire-protocol.md) — normative wire spec,
  especially its own *Security model* section.
- [`SECURITY.md`](../SECURITY.md) — how to report a vulnerability.
- [`ROADMAP.md`](../ROADMAP.md) — the Track S work items referenced
  throughout this document.
- [`agent-rs/deny.toml`](../agent-rs/deny.toml) — the Rust supply-chain
  policy invoked by the `supply-chain` workflow.
