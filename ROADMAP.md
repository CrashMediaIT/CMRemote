# CMRemote Roadmap

> **Project status (Apr 2026):** CMRemote is **not yet in production**. That
> changes the calculus: the **clean-room rewrite — and specifically the move
> to Rust** — is now the **top priority**. Per project-owner direction we
> stop trying to ship Package Manager polish on top of the inherited .NET
> codebase and instead invest that time in the rewrite, with a hard
> requirement that the cut-over from the old Docker image to the new
> application is **non-destructive** for any database or agent that exists
> in the field today.

This roadmap is therefore organised in three bands:

1. **Band 1 — Rewrite & cut-over (now).** Rust agent, clean-room server,
   first-boot setup wizard, in-place migration of the legacy Docker
   database to PostgreSQL, and a background agent-upgrade pipeline that
   honours device-online state and a 60-day inactivity cut-off. A
   cross-cutting **Track S — Security & supply-chain baseline** runs
   alongside Track R and gates every other slice: no functional work
   lands before the security gate that would catch its class of issue is
   already green.
2. **Band 2 — Feature work to carry forward.** Package Manager (PRs A/B/C1
   already shipped, C2/C3/D queued) and the agent-deployment redesign
   (PR E). These are still on the roadmap but are now subordinate to
   Band 1 and will be re-implemented inside the clean-room codebase
   rather than further extended on top of the legacy one. The hardening
   items originally batched under **PR D** have been re-scoped into
   Track S and individually pulled forward into the server-rewrite
   modules in which they naturally belong.
3. **Band 3 — UI / brand alignment.** During the Razor UI rebuild the
   application's colour scheme is realigned with **crashmedia.ca**.

> **Status legend**
> ✅ shipped  &nbsp;·&nbsp;  🟡 in progress  &nbsp;·&nbsp;  🔜 planned

---

## Band 1 — Rewrite & cut-over *(top priority)*

### Current focus *(end of slices **R2** · **R2a** · **R3** · **R4** · **R5** · **R6** · **R7** *(initial + R7.b security-contract enforcement + R7.c capture/encode trait surface + R7.d method-surface vectors + R7.f crypto-provider ADR + R7.g signalling DTOs / dispatch + R7.h input-injection / consent-prompt trait surface)* / S4 / **M1 milestone complete (M1.1 + M1.2 + M1.3 + M1.4 + M1.5)** + M2 complete + **M3 service + orchestrator landed** + **M4 admin dashboard shipped** — Apr 2026)*

Module 0 (wire-protocol spec + JSON test-vector corpus), slice **R1a**
(`cmremote-wire` JSON round-trip + redacting `Debug`), slice **R1b**
(MessagePack codec + byte-stable corpus round-trip), slice **R2**
(WebSocket connection / SignalR handshake / 15 s ping + 30 s
idle-timeout heartbeat / jittered exponential reconnect), slice **R2a**
(hub dispatch surface — `HubEnvelope` router, `InvocationTracker`
dedup, `MethodName` allow-list, completion injection, `HubClose`
quarantine path, wire-protocol method-surface vectors frozen), slice
**R3** (cross-platform `DeviceSnapshot` + `LinuxDeviceInfoProvider`
reading `/proc/meminfo` · `/proc/stat` · `/sys/class/net` · `df` ·
`/etc/os-release`; `TriggerHeartbeat` hub handler), slice **R4**
(async `tokio::process::Command` script executor, `ExecuteCommand`
hub handler, 5-minute timeout, stdout+stderr capture), slice **R5**
(`InstalledApplicationsProvider` trait, `DpkgProvider` parsing
`dpkg-query` + `rpm -qa`, `RequestInstalledApplications` and
`UninstallApplication` hub handlers, `NotSupportedAppsProvider`
stub for Windows/macOS), slice **R6** (package-manager wire surface
+ [`PackageProviderHandler`](agent-rs/crates/cmremote-platform/src/packages.rs)
trait + safety helpers — Chocolatey package-id / version
allow-lists, OLE2 magic-byte check, SHA-256 verify with
constant-time hex compare — `NotSupportedPackageProvider`,
`CompositePackageProvider` router, and the `InstallPackage` hub
handler wired through `AgentHandlers`; the composite ships with
**no concrete handlers registered** so every request is answered
with a structured "not supported" failure until the signed-build
pipeline lands with slice R8 — operator sees a clean job-failed
status rather than a hung job; the rustls-only HTTPS
[`ReqwestArtifactDownloader`](agent-rs/crates/cmremote-platform/src/packages/reqwest_download.rs)
is now wired in with `aws-lc-rs` as the crypto provider — no
`ring`, no `openssl-sys` — and feeds both the package providers
and the M3 agent self-update handler), slice **R7** *(initial +
R7.b + R7.c + R7.d + R7.f + R7.g + R7.h)*
(desktop-transport wire surface — `RemoteControlSessionRequest` /
`RestartScreenCasterRequest` / `ChangeWindowsSessionRequest` /
`InvokeCtrlAltDelRequest` / `DesktopTransportResult` PascalCase
DTOs in
[`cmremote-wire::desktop`](agent-rs/crates/cmremote-wire/src/desktop/);
[`DesktopTransportProvider`](agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
async trait; `NotSupportedDesktopTransport` safety stub naming the
host OS in every failure; the four hub methods now route to the
provider through `AgentHandlers` instead of the generic
`not_implemented` fall-through; **slice R7.b** lifts the security
contract into [`cmremote-platform::desktop::guards`](agent-rs/crates/cmremote-platform/src/desktop/guards.rs)
— cross-org refusal, operator-string sanitiser (length cap,
non-printable / NUL / DEL refusal, Unicode bidi-override / Trojan
Source refusal), and canonical lowercase-UUID `session_id` checks
that the stub *and* every future driver runs before any other
field is read; **slice R7.c** adds the
[`DesktopCapturer`](agent-rs/crates/cmremote-platform/src/desktop/media.rs)
+ `VideoEncoder` async traits with `NotSupportedDesktopCapturer` /
`NotSupportedVideoEncoder` defaults so the eventual WebRTC driver
plugs into a stable seam without any wire churn; **slice R7.d**
freezes the wire under [`docs/wire-protocol-vectors/method-surface/`](docs/wire-protocol-vectors/method-surface/)
with byte-stable JSON request + result vectors per method, all
round-tripped through both JSON and MessagePack; **slice R7.f**
ships the [WebRTC crypto-provider ADR](docs/decisions/0001-webrtc-crypto-provider.md)
that documents the trade-offs between admitting `ring`, forking
`webrtc` onto `aws-lc-rs`, and writing a thinner SRTP / DTLS / SCTP
stack on `aws-lc-rs` directly — and the security-review questions
maintainers must answer before any of those options is actionable
(the `deny.toml` ban on `ring` stays in place); **slice R7.g**
adds the WebRTC signalling DTOs (`SdpOffer` / `SdpAnswer` /
`IceCandidate` PascalCase shapes in
[`cmremote-wire::desktop::signalling`](agent-rs/crates/cmremote-wire/src/desktop/signalling.rs)),
the matching guard helpers and stub `on_sdp_offer` /
`on_sdp_answer` / `on_ice_candidate` hooks on
`DesktopTransportProvider`, the dispatch routing for the three new
hub methods (`SendSdpOffer` / `SendSdpAnswer` / `SendIceCandidate`),
and frozen byte-stable vectors under
[`docs/wire-protocol-vectors/method-surface/signalling/`](docs/wire-protocol-vectors/method-surface/signalling/);
**slice R7.h** adds the input-injection trait surface
([`MouseInput`](agent-rs/crates/cmremote-platform/src/desktop/input.rs) /
`KeyboardInput` / `Clipboard` async traits with `MouseButton` /
`KeyCode` / `NamedKey` / `ScrollAxis` DTOs and `NotSupported*`
fail-closed defaults) and the on-host consent-prompt trait
surface ([`ConsentPrompter`](agent-rs/crates/cmremote-platform/src/desktop/consent.rs)
async trait, `ConsentRequest::sanitised` constructor reusing the
R7.b operator-string guard, `ConsentDecision`
`Granted` / `Denied` / `Timeout`, and a fail-closed
`DenyAllConsentPrompter` default — a host with no consent UI
configured *refuses* sessions instead of silently allowing them)
— concrete WebRTC capture / encode driver and per-OS input /
consent UI drivers land in follow-up slices once the ADR is
decided, without any further dispatch or wire churn),
the first
security-gate items **S1** (`SECURITY.md` + coordinated-disclosure
policy), **S2** (supply-chain CI via `cargo-deny`, `cargo-audit`,
`dependency-review`, OSSF Scorecard, Dependabot), **S3**
([threat model document](docs/threat-model.md)), **S4** (fuzzing
and parser hardening — `proptest` suite on stable + `cargo-fuzz`
targets under `agent-rs/crates/cmremote-wire/fuzz/` seeded from the
corpus + nightly scheduled workflow
[`fuzz.yml`](.github/workflows/fuzz.yml)), the **complete M2
milestone**, the **complete M1 milestone** (scaffolding +
all five operator-facing wizard steps — Welcome / Preflight,
Database connection with live `SELECT 1`, optional legacy import
wired to the M2 `MigrationRunner`, first-organisation +
server-admin bootstrap, and Done step that writes the
`CMRemote.Setup.Completed` marker and routes the operator to
sign-in), and the **M3 service + orchestrator scaffolding**
(`AgentUpgradeStatus` table + EF migrations for SQLite / SQL Server
/ PostgreSQL, full `IAgentUpgradeService` state machine with
exponential-backoff retries + 60-day inactivity cut-off + on-connect
reactivation hook wired through `AgentHub.DeviceCameOnline`,
`AgentUpgradeOrchestrator` `IHostedService` with bounded-concurrency
queue + refusal-while-busy rail + per-dispatch timeout, and a
pluggable `IAgentUpgradeDispatcher` abstraction registered with a
no-op default until the publisher manifest + signed-build pipeline
ships) are merged.

**M2 — Schema converter library + CLI** is shipped end-to-end across
this PR's progressive slices:

- **Scaffolding** —
  [`Migration.Legacy/`](Migration.Legacy/) library project, public
  `MigrationOptions` / `MigrationReport` / `LegacySchemaVersion` /
  `IRowConverter<TLegacy,TV2>` / `ILegacySchemaInspector` /
  `ILegacyRowReader<TLegacy>` / `ILegacyRowWriter<TV2>` /
  `IMigrationRunner` contracts + the default `MigrationRunner`
  orchestrator + the reference `OrganizationRowConverter`.
- **Inspector** —
  [`LegacySchemaInspector`](Migration.Legacy/LegacySchemaInspector.cs)
  + connection-string-shape provider detection
  ([`LegacyDbProviderDetector`](Migration.Legacy/LegacyDbProviderDetector.cs))
  for SQLite / SQL Server / PostgreSQL; classifies a source DB as
  `Empty` / `UpstreamLegacy_2026_04` / `Unknown` and refuses to
  import on partial canonical sets.
- **Readers** — keyset-paginated, deterministic-order, async-streaming
  ADO.NET readers for all three canonical upstream tables:
  [`LegacyOrganizationReader`](Migration.Legacy/Readers/LegacyOrganizationReader.cs),
  [`LegacyDeviceReader`](Migration.Legacy/Readers/LegacyDeviceReader.cs),
  [`LegacyAspNetUserReader`](Migration.Legacy/Readers/LegacyAspNetUserReader.cs).
  Per-provider SQL is centralised in
  [`LegacyKeysetSql`](Migration.Legacy/Readers/LegacyKeysetSql.cs)
  so identifier-quoting + `LIMIT`-vs-`TOP` rules are one decision
  per provider rather than copy-pasted per reader.
- **Converters** — identity-preserving (PK round-tripped byte-stable
  per ROADMAP M1.3 so resumed runs are safe and the agent's
  persisted device id keeps matching post-import) row converters for
  every entity:
  [`OrganizationRowConverter`](Migration.Legacy/Converters/OrganizationRowConverter.cs),
  [`DeviceRowConverter`](Migration.Legacy/Converters/DeviceRowConverter.cs),
  [`AspNetUserRowConverter`](Migration.Legacy/Converters/AspNetUserRowConverter.cs).
  ASP.NET Identity columns (`PasswordHash`, `SecurityStamp`,
  `ConcurrencyStamp`, `LockoutEnd`, …) round-trip verbatim so
  existing user passwords + 2FA state survive the migration —
  the entire reason an importer exists rather than asking operators
  to re-invite users.
- **Writer contract + runner wiring** —
  [`ILegacyRowWriter<TV2>`](Migration.Legacy/ILegacyRowWriter.cs)
  + the third `MigrationRunner` constructor overload that pairs
  writer with converter by `EntityName + HandlesSchemaVersion`,
  invokes the writer on every `ConverterResult.Ok` when
  `MigrationOptions.DryRun=false`, accumulates
  `EntityReport.RowsWritten`, catches per-row writer exceptions
  into `RowsFailed` + capped `Errors` (re-throws
  `OperationCanceledException` so explicit cancellation is honoured),
  and demotes a writer-less entity to "dry-run for this entity only"
  with a single warning when the operator asked for a real import.
- **Concrete Postgres writers** — production
  [`LegacyOrganizationWriter`](Migration.Legacy/Writers/LegacyOrganizationWriter.cs),
  [`LegacyDeviceWriter`](Migration.Legacy/Writers/LegacyDeviceWriter.cs),
  [`LegacyUserWriter`](Migration.Legacy/Writers/LegacyUserWriter.cs)
  that all upsert via `INSERT … ON CONFLICT ("PK") DO UPDATE SET …`
  against the v2 Postgres schema. Idempotent by primary key (so
  resumed runs overwrite rather than duplicate). Gated by
  [`PostgresWriterRuntime.ValidateAndCreate`](Migration.Legacy/Writers/PostgresWriterRuntime.cs),
  which rejects non-Postgres connection-string shapes (the v2
  schema is Postgres-only — we refuse to silently write SQL Server
  inserts to a Postgres-conn-string field).
- **CLI** — new [`Migration.Cli/`](Migration.Cli/) console project
  publishing as `cmremote-migrate`. Surface:
  `cmremote migrate --from <sourceConn> --to <targetConn> [--dry-run]
  [--batch-size N]`. Composes the full converter / reader / writer
  triple set; honours Ctrl+C via a `CancellationTokenSource`;
  pretty-prints the per-entity `MigrationReport`; exit codes
  `0` clean, `1` row-level failures, `2` fatal error,
  `64` (BSD `EX_USAGE`) usage error. The wizard's M1.3 import step
  binds the same `MigrationRunner`, so CLI and UI share one codepath.

**Tests:** 103 across two test projects —
[`Tests/Migration.Legacy.Tests/`](Tests/Migration.Legacy.Tests/)
(90 tests; converter unit tests for all three entities, real
end-to-end SQLite reader tests with multi-page keyset pagination,
inspector classification, runner integration including writer wiring,
target-validator tests for the Postgres writer runtime) +
[`Tests/Migration.Cli.Tests/`](Tests/Migration.Cli.Tests/) (13
tests; argument parsing, exit-code mapping, end-to-end dry-run smoke
test against an in-memory SQLite source seeded with the canonical
upstream schema, report-printer formatting). The concrete Postgres
writers' `INSERT … ON CONFLICT` paths are not exercised in CI
(Postgres isn't available); they are covered by the
`PostgresWriterRuntime` validator tests + manual verification, with
a follow-up integration job tracked separately.

The next milestones are now:

1. **M3 dispatcher + ~~M4 dashboard~~.** (unchanged) The R6 wire
   surface + safety helpers are now in place on the Rust agent so
   the dispatcher's signed-build resolver lands against a stable
   `PackageProviderHandler` trait.
2. **R6 concrete handlers** — ✅ landed for Chocolatey end-to-end;
   landed for MSI / Executable behind the `ArtifactDownloader` trait
   (default = `RejectingDownloader` so MSI / Executable jobs surface
   a clean "this agent is not configured to download package
   artifacts" failure until the rustls-only `reqwest` (or equivalent)
   HTTPS client is wired). The runtime now registers
   `ChocolateyPackageProvider` / `UploadedMsiPackageProvider` /
   `ExecutablePackageProvider` against the `CompositePackageProvider`
   on every host; on non-Windows hosts each provider's `can_handle`
   returns `false` and `execute` returns the same structured failure
   shape. The agent-side `InstallAgentUpdate` slot is also now a
   working handler (decode → fetch via the same downloader →
   SHA-256 verify → hand off to a pluggable `AgentUpdateInstaller`,
   default = `StubAgentUpdateInstaller`).
3. **Live-Postgres integration coverage** (unchanged).

### 🟡 Track R — Rust agent + clean-room server *(now the lead track)*

The full module-by-module plan is in
[Clean-room redesign / separation track](#clean-room-redesign--separation-track-lead-track)
below. Summary of the new tempo:

- **No new feature work lands on the legacy .NET agent** (`Agent/`) once the
  Rust workspace (slice **R0**) is in. The .NET agent enters
  maintenance-only mode immediately and is removed one release after the
  Rust agent reaches Windows parity (slice **R6**).
- The clean-room **server** rewrite (Modules 3–6) runs in parallel with the
  Rust agent slices, gated only by the wire-protocol freeze (Module 0) and
  the `Shared` re-derivation (Module 1).
- Any Package Manager work that has not already shipped (PRs C2, C3, D, E)
  is re-targeted at the clean-room codebase rather than added to the
  legacy one.

### 🟡 Track S — Security & supply-chain baseline *(cross-cutting — S1 + S2 + S3 + S4 + S7 shipped; S6 partially shipped)*

Security is called out as a top-priority, standalone track rather than
being left as scattered mentions inside the Rust slices. Items here gate
every other track: a Track R slice does not ship until the Track S gate
that would have caught the class of issue it might introduce is already
green.

**S1 — `SECURITY.md` + coordinated disclosure *(✅ shipped)*.** A
top-level [`SECURITY.md`](SECURITY.md) ships with:

- Names a single reporting channel (`security@crashmedia.ca`) and a
  GPG-fingerprint-published PGP key for encrypted reports.
- States the supported-versions matrix (currently: `v1-maintenance`
  branch = security fixes only; `main` = pre-release, best-effort).
- Pins a **90-day coordinated-disclosure window** with an explicit
  fast-track for actively-exploited vulnerabilities.
- Enables GitHub **private vulnerability reporting** on the repo so
  outside reporters have a UI path in addition to email.
- Points at the threat model (**S3**) for scope clarity: the Rust
  agent, the .NET server, the wire protocol, and the migration
  pipeline are all in scope; self-hosted deployments outside the
  upstream-supported Docker image are best-effort.

**S2 — Supply-chain CI gates *(✅ shipped — initial set)*.** Landed
before the next functional Rust dependency so the gate caught
`rmp-serde` (added for slice R1b) on the way in. Active gates:

- **Rust:** [`agent-rs/deny.toml`](agent-rs/deny.toml) drives
  `cargo-deny` with a licence allow-list, the RUSTSEC advisory DB,
  a banned-crate list (`openssl-sys`, `ring` — we use rustls-based
  TLS), and a crates.io-only source allow-list. `cargo-audit` runs
  the same RUSTSEC DB as a second opinion.
- **GitHub-native:** [`dependency-review`](.github/workflows/supply-chain.yml)
  runs on every PR with the same licence allow-list and
  `fail-on-severity: moderate`. The
  [OSSF Scorecard](.github/workflows/scorecard.yml) workflow publishes
  findings into the Security tab on push + weekly. Dependabot
  ([`.github/dependabot.yml`](.github/dependabot.yml)) raises grouped
  weekly version PRs and always-on security PRs for `cargo`
  (`agent-rs/`), `nuget`, `github-actions`, and `docker`
  (`docker-compose/`).
- **Scheduled sweep:** the supply-chain workflow runs weekly against
  `main` so an advisory published against an already-merged dependency
  fails CI within 7 days.

Still queued under S2 (not yet shipped): `cargo-vet` audit set,
.NET `packages.lock.json` + `RestoreLockedMode=true` in CI, and
`CODEOWNERS` gating on workflow / dependency manifests.

**S3 — Threat model document *(✅ shipped)*.**
[`docs/threat-model.md`](docs/threat-model.md) expands on the normative
*Security model* section in `docs/wire-protocol.md` with:

- A STRIDE-per-surface table: **agent↔server hub**, **server↔DB**,
  **server↔browser (Razor / Blazor circuits + cookies)**,
  **migration importer ↔ legacy SQLite/SqlServer/Postgres**,
  **agent-upgrade pipeline (signed-build fetch)**,
  **uploaded-MSI handling**, **WebRTC desktop transport**.
- Explicit trust boundaries and where input validation is required on
  each side of each boundary.
- A short *Non-goals* section so reporters know what we explicitly do
  not defend against (e.g. an operator who has root on the server
  host, a local user already in the `Administrators` group on an
  endpoint).
- Owners and review cadence (reviewed at the start of every module
  rewrite; re-reviewed when a trust boundary moves).

**S4 — Fuzzing and parser hardening *(✅ shipped)*.**

- A `cargo-fuzz` target per wire parser: `ConnectionInfo` JSON, hub
  envelopes (JSON and MessagePack). The
  targets live in a dedicated out-of-workspace crate at
  [`agent-rs/crates/cmremote-wire/fuzz/`](agent-rs/crates/cmremote-wire/fuzz/)
  so the nightly-only `libfuzzer-sys` dependency does not leak into
  the stable workspace. The corpus seeds from
  `docs/wire-protocol-vectors/` at workflow time; any crash found is
  triaged into a `tests/vectors.rs` regression case before the fix
  ships.
- Nightly scheduled `cargo-fuzz` runs (15 min per target) via
  [`.github/workflows/fuzz.yml`](.github/workflows/fuzz.yml). The
  workflow does not block PRs, uploads the minimised reproducer as a
  workflow artifact, and opens a deduplicated GitHub issue labelled
  `fuzz,security` on crash.
- A `proptest` suite on the same surfaces
  ([`crates/cmremote-wire/tests/proptest_parsers.rs`](agent-rs/crates/cmremote-wire/tests/proptest_parsers.rs))
  for fast-feedback property coverage on stable. The suite pins three
  invariants per type: JSON round-trip, MessagePack round-trip with
  byte-stable re-encode, and "arbitrary bytes never panic the
  decoder". It runs on every PR alongside the existing vector
  conformance tests.
- On the .NET side, the conformance runner queued for slice **R2a**
  replays the same vector corpus against the server dispatch layer
  so divergence is caught on both sides of the wire.

**S5 — Release integrity: SBOM + signed builds *(🔜)*.**

- Generate a **CycloneDX** SBOM for both the Rust agent
  (`cargo-cyclonedx`) and the .NET server (`dotnet-CycloneDX`) on
  every tagged release and attach it to the GitHub release assets.
- Sign release binaries with **Sigstore cosign** in keyless mode from
  the release workflow; publish both the signature and the
  Rekor log entry as release assets.
- Generate **SLSA v1.0** build provenance via the
  `slsa-framework/slsa-github-generator` reusable workflow.
- The agent installer (PR E / slice R8) refuses to install a build
  whose cosign signature does not verify against the published
  certificate identity, closing the loop between the release process
  and the agent-upgrade pipeline (M3) which already requires a
  SHA-256 match against the publisher manifest.

**S6 — Secret-hygiene enforcement *(🟡 partially shipped)*.**

- ✅ **gitleaks** ([`.github/workflows/gitleaks.yml`](.github/workflows/gitleaks.yml))
  runs as a PR gate (and weekly on `main`/`master`) so an
  accidentally-committed token fails the build instead of polluting
  the audit log; findings are also uploaded as SARIF into the
  Security tab so they show up alongside CodeQL and Scorecard.
- ✅ **CodeQL** ([`.github/workflows/codeql.yml`](.github/workflows/codeql.yml))
  covers both `csharp` (the .NET solution built explicitly against
  the .NET 8 SDK pinned in the csproj files) and `rust` (the
  `agent-rs/` workspace) on every PR, on push to `main`/`master`,
  and weekly on `main` so a new query published after a merge still
  surfaces against already-merged code within seven days. Uses the
  `security-extended` query pack.
- ✅ Redacting-`Debug` regression test for `ConnectionInfo`
  (`debug_redacts_server_verification_token` and
  `debug_redacts_organization_token` in
  [`crates/cmremote-wire/src/connection_info.rs`](agent-rs/crates/cmremote-wire/src/connection_info.rs))
  — already shipped with slice R1a; pinned here as the S6 deliverable.
- 🔜 Add a unit test under `cmremote-platform` that asserts
  `ConnectionInfo.json` is written with file-mode `0600` on Unix and
  an equivalent ACL check on Windows, once the agent's *write* path
  for `ConnectionInfo.json` lands (today's runtime only reads the
  file; writes will land with the enrolment slice).

**S7 — Runtime security posture *(✅ shipped — headers + per-org rate limits + signed MSI URLs + immutable audit log).***

- Default strict **CSP**, **HSTS** (`includeSubDomains; preload`),
  `X-Content-Type-Options: nosniff`, `Referrer-Policy:
  strict-origin-when-cross-origin`, and a `Permissions-Policy` that
  denies camera/microphone/geolocation by default on every response
  from the Razor server. The WebRTC viewer opts back in on the
  specific routes that need it. Shipped as
  [`Server/Middleware/SecurityHeadersMiddleware.cs`](Server/Middleware/SecurityHeadersMiddleware.cs)
  with `SecurityHeadersMiddlewareTests` covering the default-route
  headers, the `/Viewer` opt-in, and the no-overwrite composition
  contract. Wired into `Program.cs` immediately after `UseRouting`.
- Per-org **rate limits** on install-job dispatch (was PR D), shipped
  as `PackageInstallJobRateLimiter` (sliding-window, default
  240 / minute / org). Wraps both
  `PackageInstallJobService.QueueJobAsync` and `QueueBundleAsync`;
  bundle path charges the limiter for the full fan-out up-front and
  refuses partial inserts. A request that asks for more permits than
  the per-window budget is translated into a clean refusal rather
  than the underlying `ArgumentOutOfRangeException`. Tests in
  `PackageInstallJobRateLimiterTests`.
- **Uploaded-MSI download URLs** signed with a short TTL + device-scoped
  HMAC (was PR D), shipped as `SignedMsiUrlService` +
  `SignedMsiTokenFilter` + `UploadedMsiDownloadController`. Tokens are
  ASP.NET Core `IDataProtector` envelopes binding
  `{deviceId, sharedFileId, expiresAt}` to a purpose-pinned key so a
  leaked URL only works for the device + MSI it was minted for, only
  inside its TTL window. Minted in `CircuitConnection.DispatchJobAsync`
  alongside the legacy expiring token so old agents keep working
  during R6 rollout. Surfaced on `PackageInstallRequestDto` as
  `MsiSignedToken` + `MsiSignedDownloadUrl`. Tests in
  `SignedMsiUrlServiceTests` cover the device + file binding, the
  TTL, and tamper detection.
- An **immutable audit log** (was PR D) is re-scoped as a Track S
  deliverable and lands with Module 3 (`Server.Services`) so every
  subsequent module inherits it. Shipped as `AuditLogEntry` +
  `IAuditLogService` / `AuditLogService` with EF migrations across
  SQLite / SQL Server / PostgreSQL (`Add_AuditLog`). Each row carries
  `EntryHash = SHA-256(prev_hash || canonical_serialized_body)`;
  verification is one linear scan and reports the sequence number of
  the first tampered or broken-link row. Chains are per-organization
  so a multi-tenant deployment can be sharded / archived /
  GDPR-deleted per-org without breaking the chain for other orgs.
  `CircuitConnection.DispatchJobAsync` appends a
  `package.install.dispatch` row on every dispatch (the failure path
  is logged but cannot block the dispatch). Tests in
  `AuditLogServiceTests` cover the chain link, per-org isolation,
  tampered-summary detection, broken-link detection, and the
  canonicalised `DetailJson` form.

**Sequencing.** S1 and S2 land before any further functional work on
Track R. S4 lands before slice R2 *(shipped)*. S3 lands before Module 3.
S5 lands before slice R8. S6 is staged across slice R1b
(redacting-Debug test) and the server rewrite (gitleaks, CodeQL
schedule). S7's items move from PR D into the module in which they
naturally belong and are no longer deferred until after the rewrite.

### 🔜 PR M — Migration & cut-over from the legacy Docker image

When CMRemote v2 replaces the upstream Docker image in a deployment, the
operator must be able to drop the new image in *on top of* the existing
volume / database / agent fleet without losing data and without bricking
agents that happen to be offline that day. PR M delivers that path.

**M1 — First-boot setup wizard.** *(✅ shipped — scaffolding + all five steps.)*
The skeleton landed first: a `/setup` Razor page rendered through
a minimal `EmptyLayout`, an `ISetupStateService` backed by a fixed-Guid row
in `KeyValueRecords` for the `CMRemote.Setup.Completed` marker, a
`SetupRedirectMiddleware` that forwards uncompleted setups to `/setup`
(with framework / static / `.well-known` paths allowlisted and non-GET
requests answered with `503 + Retry-After: 30` so partially-upgraded
clients don't silently drop state), and a startup heuristic that
auto-writes the marker when the database already contains an
organisation, user, or device — so existing deployments are never
hijacked into the wizard on upgrade.

This slice then ships the operator-facing **M1.1** (Welcome /
preflight), **M1.2** (Database connection), and **M1.3** (Import
existing database) steps, plus a persistent
[`ISetupWizardProgressService`](Server/Services/Setup/ISetupWizardProgressService.cs)
backed by a sister `KeyValueRecords` row so a browser refresh
mid-install resumes at the correct step rather than dropping the
operator back to the welcome card. The wizard's index page now
routes off the persisted progress and shows each step as
*Completed* / *Next* / *Pending*.

**M1.1 (Welcome / preflight) — shipped.**
[`PreflightService`](Server/Services/Setup/PreflightService.cs) runs
three checks and surfaces them through
[`PreflightReport`](Server/Services/Setup/IPreflightService.cs):
writable data directory (atomic create-and-delete probe in the
directory the wizard plans to write
`appsettings.Production.json` to), TLS endpoint configured (advisory
warning if neither `ASPNETCORE_URLS` nor `Kestrel:Endpoints:*:Url`
contains an `https://` binding — HTTP-only is allowed because
CMRemote is routinely deployed behind a TLS-terminating reverse
proxy), and bind-port reachability (the wizard runs *inside* the
running server, so any configured URL has already bound; the check
surfaces the bound URLs so the operator can confirm the wizard is
reachable on the address they expect). Failures block continuing,
warnings do not.

**M1.2 (Database connection) — shipped.**
[`PostgresConnectionTester`](Server/Services/Setup/PostgresConnectionTester.cs)
performs a live `SELECT 1` round trip against the operator-supplied
connection string and returns a three-valued
[`ConnectionTestResult`](Server/Services/Setup/IDatabaseConnectionTester.cs)
(`Success` / `InvalidConnectionString` / `NetworkOrAuthFailure`) so
the wizard can distinguish "you typo'd the form" from "the server
is unreachable" and surface different remediation copy. The
operator's password is redacted from any error message returned to
the wizard.
[`ConnectionStringWriter`](Server/Services/Setup/ConnectionStringWriter.cs)
persists the validated string to `appsettings.Production.json`
through an atomic temp-file rename, sets file mode `0600` on Unix,
preserves any unrelated keys already in the file, sets
`ApplicationOptions:DbProvider=PostgreSql`, and triggers
`IConfigurationRoot.Reload` so subsequent requests pick up the new
value without a process restart.

**M1.3 (Import existing database) — shipped.**
[`SetupImportService`](Server/Services/Setup/SetupImportService.cs)
binds the same converter / reader / writer triple set as
`Migration.Cli/Program.cs::BuildRunner`, so the wizard and the
headless `cmremote-migrate` CLI exercise one runner end-to-end and
cannot drift. The service exposes a "Detect" affordance (dry-run
that exits after schema detection so the operator can confirm the
source string before committing), a "Dry-run import" affordance
(full converter pass with no target writes), and a real "Run
import" affordance (idempotent `INSERT … ON CONFLICT DO UPDATE`
through the M2 Postgres writers). On every real or dry run the
service also persists `migration-report.json` next to the wizard's
settings file so an operator post-mortem after the wizard closes
is straightforward. The wizard surfaces a wizard-namespace
[`WizardImportReport`](Server/Services/Setup/ISetupImportService.cs)
DTO so the Razor page does not need to reach into the aliased
`Migration.Legacy` namespace; the underlying `MigrationReport` is
still written verbatim to disk. The Migration.Legacy reference is
declared with `<Aliases>MigrationLegacy</Aliases>` so its
`Remotely.Migration` parent namespace cannot shadow
`Microsoft.EntityFrameworkCore.Migrations.Migration` inside the
auto-generated EF migration classes under
`Server/Migrations/**/*.cs`.

**M1.4 (Admin bootstrap) — shipped.**
[`AdminBootstrapService`](Server/Services/Setup/AdminBootstrapService.cs)
gates the step behind an `IsRequiredAsync` probe — when an org or
user already exists in the v2 schema (typically because M1.3
imported a populated upstream database) the wizard skips straight
to M1.5. When required, the service creates a first
[`Organization`](Shared/Entities/Organization.cs) flagged
`IsDefaultOrganization=true` and then hands a
[`RemotelyUser`](Shared/Entities/RemotelyUser.cs) with
`IsAdministrator=true`, `IsServerAdmin=true`, `EmailConfirmed=true`,
and `LockoutEnabled=true` to ASP.NET Identity's
`UserManager.CreateAsync(user, password)` so the configured
`IPasswordHasher` hashes the password identically to the rest of
the app and `SecurityStamp` / `ConcurrencyStamp` are stamped on
creation. `IdentityResult.Errors` (password-policy violations,
duplicate email, …) are propagated through
[`AdminBootstrapResult`](Server/Services/Setup/IAdminBootstrapService.cs)
so the wizard can render actionable copy. A failed `CreateAsync`
rolls the org row back so a re-attempt with a stronger password
does not leave a phantom org behind. A re-check of the
"no-users-yet" precondition is performed inside the operation
itself so a second wizard browser session cannot race in a parallel
admin. Organisation names are pre-truncated to the 25-char storage
cap so the operator gets a clean success rather than a thrown
`DbUpdateException`. The
[`/setup/admin`](Server/Components/Pages/Setup/SetupAdmin.razor)
Razor page renders the form, a "Skip" affordance shown when the
service reports the step is no longer required, and a confirm-password
field that the page validates client-side before invoking the
service.

**M1.5 (Done) — shipped.** The
[`/setup/done`](Server/Components/Pages/Setup/SetupDone.razor)
Razor page is the *only* surface that calls
`ISetupStateService.MarkSetupCompletedAsync`. Once the marker
lands, `SetupRedirectMiddleware` stops forwarding requests to
`/setup` and every `/setup/*` page renders the "Setup already
complete" copy — the wizard cannot be re-run without operator
intervention on the database. The page is idempotent (a refresh
re-finalises against the existing marker without overwriting the
original stamp, per the existing `MarkSetupCompletedAsync`
contract), advances wizard progress to `SetupWizardStep.Done`, and
links the operator to `/Account/Login?returnUrl=%2F` so they can
sign in with the admin credentials they just configured in M1.4
(or the imported credentials from M1.3, if they skipped M1.4).

The five steps of the wizard are:

1. **Welcome / preflight** *(✅ shipped — M1.1)* — checks writable
   data dir, TLS endpoint configured (advisory), and surfaces the
   bound URLs so the operator can confirm the wizard is reachable on
   the address they expect.
2. **Database connection** *(✅ shipped — M1.2)* — Postgres-only host
   / port / db / user / password / SSL-mode form. The wizard performs
   a live `SELECT 1` round trip and, on success, writes the connection
   string to `appsettings.Production.json` (file mode `0600` on Unix,
   atomic temp-file rename, unrelated keys preserved) and reloads
   configuration. It does not advance until the round trip succeeds.
3. **Import existing database** *(✅ shipped — M1.3, optional)* —
   shown for greenfield installs as a skip-able optional step. When
   an upstream connection string is supplied (SQLite file path, SQL
   Server, or Postgres) the wizard binds the same `MigrationRunner`
   the headless `cmremote-migrate` CLI uses; the operator can run a
   detection probe, a full dry-run, or the real idempotent import.
   Devices are imported with their existing IDs and shared secrets
   so already-deployed agents reconnect under the same record. The
   resulting `MigrationReport` is written to disk as
   `migration-report.json` next to the wizard's settings file.
4. **Admin bootstrap** *(✅ shipped — M1.4, only if no users were imported)* —
   creates the first organisation + server-admin account.
5. **Done** *(✅ shipped — M1.5)* — writes the
   `CMRemote.Setup.Completed` marker, advances wizard progress, and
   routes the operator to `/Account/Login` (with `returnUrl=/`) so
   they can sign in with the admin credentials they just configured
   (or the imported credentials from M1.3, if they skipped M1.4).
   The agent-upgrade sweep defined in **M3** will be queued from
   this step once M3 lands.

The wizard is non-blocking past step 3: if the operator skips the import
they can run it later from `/admin/migration`. **The operator is never
forced to wait for agents to upgrade before reaching the main panel.**

**M1 wizard tests.** Six new MSTest classes under
[`Tests/Server.Tests/`](Tests/Server.Tests/) — 44 tests in total —
cover every wizard service end-to-end:
[`SetupWizardProgressServiceTests`](Tests/Server.Tests/SetupWizardProgressServiceTests.cs)
(round-trip persistence, refuses-to-move-backwards rule, malformed
marker recovery, unknown-enum recovery),
[`PreflightServiceTests`](Tests/Server.Tests/PreflightServiceTests.cs)
(writable-dir probe, HTTP-only ASPNETCORE_URLS warning vs. HTTPS pass
and Kestrel:Endpoints HTTPS pass, bound-URL surfacing,
non-blocking-warning rule),
[`ConnectionStringWriterTests`](Tests/Server.Tests/ConnectionStringWriterTests.cs)
(file creation, key preservation, overwrite-on-second-write,
configuration round-trip, empty-string guard, Unix `0600` mode),
[`PostgresConnectionTesterTests`](Tests/Server.Tests/PostgresConnectionTesterTests.cs)
(empty / whitespace / malformed string distinguished from network
failure, missing-Host detected, RFC 5737 TEST-NET-2 timeout
exercising the network path with the password redacted from the
returned message),
[`SetupImportServiceTests`](Tests/Server.Tests/SetupImportServiceTests.cs)
(end-to-end dry-run against an in-memory SQLite source seeded with
the canonical upstream schema — the same fixture the
`Migration.Cli.Tests` smoke suite uses, so the wizard and CLI prove
the same runner composition is wired the same way; plus persistence
of `migration-report.json` and the empty-string argument guards),
and
[`AdminBootstrapServiceTests`](Tests/Server.Tests/AdminBootstrapServiceTests.cs)
(IsRequired greenfield/imported-org cases, happy-path create
asserting `IsAdministrator` + `IsServerAdmin` + hashed
`PasswordHash` + stamped `SecurityStamp` + lower-cased email,
weak-password rollback so a phantom org row is not left behind,
refuse-second-call once an admin exists, blank-input validation
errors, organisation-name truncation to the 25-char storage cap).

**M2 — Schema converter library + CLI.** *(✅ shipped.)*
A new [`Migration.Legacy/`](Migration.Legacy/) library project
(assembly `Remotely.Migration.Legacy`, references `Shared`) holds the
public contracts the wizard's import step (M1.3) and the headless
CLI both bind against — `MigrationOptions`, `MigrationReport`
(+ `EntityReport`, JSON round-trip, `ReportSchemaVersion`),
`LegacySchemaVersion` (open enum: `Unknown`, `Empty`,
`UpstreamLegacy_2026_04`, …), `IRowConverter<TLegacy,TV2>` with a
`ConverterResult<T>` `Ok`/`Skip`/`Fail` discriminated union,
`ILegacySchemaInspector`,
[`ILegacyRowReader<TLegacy>`](Migration.Legacy/ILegacyRowReader.cs),
[`ILegacyRowWriter<TV2>`](Migration.Legacy/ILegacyRowWriter.cs),
`IMigrationRunner` — plus the default
[`MigrationRunner`](Migration.Legacy/MigrationRunner.cs) orchestrator
that pairs converter / reader / writer by `EntityName +
HandlesSchemaVersion`, streams every entity end to end, and emits a
report even when detection fails or the inspector throws.

The concrete
[`LegacySchemaInspector`](Migration.Legacy/LegacySchemaInspector.cs)
opens the source connection through the connection-string-shape
provider detector
[`LegacyDbProviderDetector`](Migration.Legacy/LegacyDbProviderDetector.cs)
(SQLite on `Data Source=` / `Filename=`, SQL Server on `Server=` /
`Initial Catalog=`, PostgreSQL on `Host=`), runs the per-provider
table-list query, and classifies a source DB as `Empty`,
`UpstreamLegacy_2026_04`, or `Unknown` — partial canonical sets are
reported `Unknown` so the runner refuses to import rather than risk
silent data loss.

Per-entity **readers** keyset-paginate the three canonical upstream
tables in `MigrationOptions.BatchSize`-sized pages ordered by primary
key (deterministic so a resumed run sees the same sequence): SQLite
+ PostgreSQL share `LIMIT @batch`; SQL Server uses `TOP(@batch)`.
[`LegacyOrganizationReader`](Migration.Legacy/Readers/LegacyOrganizationReader.cs)
walks `Organizations`,
[`LegacyDeviceReader`](Migration.Legacy/Readers/LegacyDeviceReader.cs)
walks `Devices` (with provider-portable `DateTimeOffset`
materialisation that handles SQLite's TEXT representation),
[`LegacyAspNetUserReader`](Migration.Legacy/Readers/LegacyAspNetUserReader.cs)
walks `AspNetUsers` (cursor on `Id`, the lower-case ASP.NET Identity
spelling). The per-provider SQL is centralised in
[`LegacyKeysetSql`](Migration.Legacy/Readers/LegacyKeysetSql.cs) so
identifier-quoting + `LIMIT`-vs-`TOP` rules are one decision per
provider rather than copy-pasted per reader.

Per-entity **converters** preserve identity (PK round-tripped
byte-stable per ROADMAP M1.3 so resumed runs are safe and the
agent's persisted device id keeps matching post-import) and copy
scalar fields:
[`OrganizationRowConverter`](Migration.Legacy/Converters/OrganizationRowConverter.cs)
(name truncated at the v2 25-char cap, skip-on-missing-name,
fail-on-missing-id),
[`DeviceRowConverter`](Migration.Legacy/Converters/DeviceRowConverter.cs)
(skip-on-missing-org so half-deleted orgs don't bring down the run,
forces `IsOnline=false` so the panel doesn't claim devices are
online before the agent has re-handshaked, truncates `Alias` /
`Tags` / `Notes` to the v2 caps),
[`AspNetUserRowConverter`](Migration.Legacy/Converters/AspNetUserRowConverter.cs)
(round-trips `PasswordHash` / `SecurityStamp` / `ConcurrencyStamp`
/ `LockoutEnd` / `TwoFactorEnabled` verbatim so existing user
passwords + 2FA state survive the migration — the entire reason an
importer exists).

The runner accepts a writers collection; on every
`ConverterResult.Ok` it invokes the matching writer when
`MigrationOptions.DryRun=false`, accumulates
`EntityReport.RowsWritten`, catches per-row writer exceptions into
`RowsFailed` + capped `Errors` (re-throws
`OperationCanceledException` so explicit cancellation is honoured),
and demotes a writer-less entity to "dry-run for this entity only"
with a single warning when the operator asked for a real import.

Concrete **Postgres writers** all upsert via
`INSERT … ON CONFLICT ("PK") DO UPDATE SET …` against the v2 schema:
[`LegacyOrganizationWriter`](Migration.Legacy/Writers/LegacyOrganizationWriter.cs),
[`LegacyDeviceWriter`](Migration.Legacy/Writers/LegacyDeviceWriter.cs),
[`LegacyUserWriter`](Migration.Legacy/Writers/LegacyUserWriter.cs).
Idempotent by primary key (so resumed runs overwrite rather than
duplicate). Gated by
[`PostgresWriterRuntime.ValidateAndCreate`](Migration.Legacy/Writers/PostgresWriterRuntime.cs),
which rejects non-Postgres connection-string shapes — the v2 schema
is Postgres-only and we refuse to silently write a SQL Server
INSERT to a Postgres-conn-string field.

The new [`Migration.Cli/`](Migration.Cli/) console project publishes
as `cmremote-migrate` and exposes the same runner over the shell:
`cmremote migrate --from <sourceConn> --to <targetConn> [--dry-run]
[--batch-size N]`. Composes the full converter / reader / writer
triple set; honours Ctrl+C via a `CancellationTokenSource`;
pretty-prints the per-entity `MigrationReport`; exit codes `0`
clean, `1` row-level failures, `2` fatal error, `64` (BSD `EX_USAGE`)
usage error. The wizard's M1.3 import step binds the same
`MigrationRunner`, so CLI and UI share one codepath.

Tests: 103 across two projects.
[`Tests/Migration.Legacy.Tests/`](Tests/Migration.Legacy.Tests/)
(90 tests) covers the writer-contract wiring end-to-end (DryRun
honoured, real-run happy path, no-writer-for-converter demotion-with-warning,
per-row writer exception isolation, idempotency-across-reruns,
constructor null guards), the new converters
([`DeviceRowConverterTests`](Tests/Migration.Legacy.Tests/DeviceRowConverterTests.cs)
+ [`AspNetUserRowConverterTests`](Tests/Migration.Legacy.Tests/AspNetUserRowConverterTests.cs)
— identity preservation, skip/fail rules, length-cap truncation,
Identity-column round-trip), real end-to-end SQLite reader paths for
Organizations and Devices (multi-page keyset pagination across 3
pages of 12 rows at batch size 5, NULL-name pass-through to the
converter, scalar materialisation of every projected column
including the SQLite-TEXT `DateTimeOffset` round-trip), and the
Postgres writer runtime validator
([`PostgresWriterRuntimeTests`](Tests/Migration.Legacy.Tests/PostgresWriterRuntimeTests.cs)
— blank-string guard, non-Postgres-shape rejection, Postgres-shape
acceptance).
[`Tests/Migration.Cli.Tests/`](Tests/Migration.Cli.Tests/) (13
tests) covers argument parsing (happy path, short flags, missing
required, value-after-flag missing, non-positive / non-integer
batch, unknown flag), exit-code mapping (clean / row-failures /
fatal-trumps-row-failures), and an end-to-end dry-run smoke test
that builds the CLI's runner against an in-memory SQLite source
seeded with the canonical upstream schema and asserts every entity
streams through with the expected per-entity counts and zero
`RowsWritten` + zero exit code, plus a report-printer formatting
guard so the operator-visible plain-text columns don't drift
silently.

The concrete Postgres writers' `INSERT … ON CONFLICT` paths are not
exercised against a live DB in CI (Postgres isn't available in the
test runner); they are covered by the validator tests + manual
verification for now, with live-DB integration coverage tracked as
part of the M1.3 wizard work which already needs a Postgres test
container.

**M3 — Background agent-upgrade pipeline.** *(🟡 service + orchestrator
shipped; real dispatcher waits for slice R6/R8.)* Once the operator is
in the main panel, an `IHostedService`
([`AgentUpgradeOrchestrator`](Server/Services/AgentUpgrade/AgentUpgradeOrchestrator.cs))
drives the fleet upgrade asynchronously.

- A new `AgentUpgradeStatus` table tracks every device:
  `DeviceId, FromVersion, ToVersion, State, LastAttemptAt,
   LastAttemptError, AttemptCount, EligibleAt, CompletedAt`.
  States: `Pending → Skipped(Inactive) | Skipped(OptOut) | Scheduled
  → InProgress → Succeeded | Failed → (retry) Pending`. Shipped:
  [`AgentUpgradeStatus`](Shared/Entities/AgentUpgradeStatus.cs) +
  [`AgentUpgradeState`](Shared/Enums/AgentUpgradeState.cs) +
  EF migrations for SQLite / SQL Server / PostgreSQL with a unique
  index on `DeviceId` and a `(State, EligibleAt)` index that the
  orchestrator's "next batch of eligible work" sweep uses.
- **60-day inactivity cut-off.** On enrolment into the pipeline, any
  device whose `LastOnline` is older than `UtcNow − 60 days` is moved
  straight to `Skipped(Inactive)` and **not contacted**. The state is
  re-evaluated when the device next reconnects (see below). Shipped
  in
  [`AgentUpgradeService.EnrolDeviceAsync`](Server/Services/AgentUpgrade/AgentUpgradeService.cs)
  with the cut-off pinned by `IAgentUpgradeService.InactivityCutoff`.
- **Online devices** are upgraded in a bounded-concurrency queue
  (default 5 in flight per server, tunable via
  `AgentUpgrade:MaxConcurrency`). The upgrade itself reuses
  the **existing** PR E installer surface — the server publishes the
  new agent build, the running legacy agent fetches it over the
  authenticated hub, swaps binary + service definition, and reconnects.
  Success is observed when the device next sends a heartbeat tagged
  with the new agent version. Shipped: the bounded-concurrency sweep
  (`AgentUpgradeOrchestrator.SweepOnceAsync`) runs against an
  `IAgentUpgradeDispatcher` abstraction that's currently bound to a
  no-op default
  ([`NoopAgentUpgradeDispatcher`](Server/Services/AgentUpgrade/IAgentUpgradeDispatcher.cs))
  until the publisher manifest + signed-build fetch lands with slice
  R6 / R8; the rest of the state machine is exercised end-to-end
  against the no-op today.
- **Offline devices** are not contacted while offline. The
  `AgentHub.OnConnectedAsync` path checks the device's
  `AgentUpgradeStatus`: if the row is `Pending` *and* `LastOnline`
  (now updated) is within 60 days, the upgrade is **dispatched the
  instant the device connects**, before any user-facing job is
  delivered to it. If the row is `Skipped(Inactive)` and the device
  has now re-appeared, the row is flipped back to `Pending` and the
  same on-connect dispatch fires. Shipped:
  [`AgentHub.DeviceCameOnline`](Server/Hubs/AgentHub.cs) calls
  `EnrolDeviceAsync` + `MarkDeviceCameOnlineAsync` on every connect
  inside a try/catch that never blocks the connection on a pipeline
  hiccup.
- **Failure handling.** Failed upgrades are retried with exponential
  backoff (1 min → 2 min → 4 min → 8 min → … capped at 24 h, max 5
  attempts). After exhaustion the row stays `Failed` and surfaces in
  the admin **Agent upgrade** dashboard with the device id, last
  error, and a "Retry" button. Shipped: backoff math is
  `IAgentUpgradeService.ComputeBackoff`, the cap is
  `IAgentUpgradeService.MaxBackoff`, and exhaustion is enforced by
  `IAgentUpgradeService.MaxAttempts` — the `MarkFailedAsync` path
  drives Failed → Pending while incrementing `AttemptCount`.
- **Safety rails.** The orchestrator refuses to dispatch an upgrade
  while the device has an in-flight `PackageInstallJob`,
  `BundleRunJob`, script, or remote-control session. It also refuses
  to dispatch if the target build's SHA-256 / signature does not match
  the manifest written by the publisher. Shipped: the "in-flight
  PackageInstallJob" rail is enforced by
  `IAgentUpgradeService.HasInFlightJobAsync` + the orchestrator's
  pre-dispatch check (which leaves the row `Pending` so it's retried
  on the next sweep without burning a retry slot). The
  signature/SHA-256 rail is part of the dispatcher contract; the
  no-op default trivially satisfies it because it never resolves a
  target. The remaining script / remote-control / bundle rails land
  with the real dispatcher.

**M3 tests.** Two new MSTest classes under
[`Tests/Server.Tests/`](Tests/Server.Tests/) — 35 tests in total —
exercise the state machine, the orchestrator sweep, and the safety
rails:
[`AgentUpgradeServiceTests`](Tests/Server.Tests/AgentUpgradeServiceTests.cs)
covers the pure transition predicate (every legal/illegal pair), the
backoff math (first retry, doubling, cap, zero/negative input), 60-day
cut-off classification at enrolment, idempotent re-enrolment, on-connect
reactivation (SkippedInactive → Pending; SkippedOptOut left alone;
unknown device returns null), the reservation race
(`TryReserveAsync` refuses a second concurrent caller and refuses when
EligibleAt is in the future), terminal stamping (CompletedAt +
LastAttemptError clearance), the full retry-then-exhaust loop driven
through a virtual clock, operator overrides (ForceRetry resets attempts
even from terminal Failed; SetOptOut refused mid-InProgress), the
in-flight-job rail (Queued + Running detected, terminal jobs ignored,
blank input safe), the dashboard aggregate (every enum present, zero
buckets included), and the eligible-rows query (ordered by EligibleAt,
respects limit and zero-limit short-circuit).
[`AgentUpgradeOrchestratorTests`](Tests/Server.Tests/AgentUpgradeOrchestratorTests.cs)
drives `SweepOnceAsync` against a stub dispatcher: happy-path dispatch,
SkippedInactive devices not contacted, the on-connect reactivation
loop reaching dispatch on the next sweep, the in-flight-job rail
short-circuiting without burning a retry slot, dispatcher failure
requeuing with backoff, "no target available" rolling Scheduled back
to Pending without consuming a retry, MaxConcurrency observed under
load, and SweepBatchSize capping rows processed per sweep.

**M4 — Admin "Agent upgrade" dashboard.** *(✅ shipped.)*
[`/admin/agent-upgrade`](Server/Components/Pages/AgentUpgradeDashboard.razor)
is an org-admin Razor page (gated by `OrganizationAdminRequired`)
that surfaces the M3 pipeline to operators:

- **Summary cards** for every `AgentUpgradeState` (`Pending / Scheduled
  / InProgress / Succeeded / Failed / Skipped(Inactive) /
  Skipped(OptOut)`), driven by
  `IAgentUpgradeService.GetStateCountsAsync` so the dashboard cannot
  drift from the orchestrator's view of the world.
- **Searchable, paged device table** showing `DeviceName` + `DeviceId`,
  current state badge, `FromVersion → ToVersion`, last-online age,
  attempt count, and the most recent failure message. The table is
  fed by the new
  [`IAgentUpgradeService.GetRowsForOrganizationAsync`](Server/Services/AgentUpgrade/IAgentUpgradeService.cs)
  + `CountRowsForOrganizationAsync` pair, which left-joins the
  `Devices` table so a status row whose underlying device record has
  been deleted still surfaces (with `DeviceName` / `LastOnline` left
  null) instead of disappearing silently. Search is a case-insensitive
  substring match on `DeviceId` and `DeviceName`; ordering is by
  `CreatedAt` descending so the most recent enrolments surface first.
- **Per-row Retry / Skip / Force actions** wired through the new
  org-scoped overloads
  [`ForceRetryAsync(statusId, organizationId)`](Server/Services/AgentUpgrade/IAgentUpgradeService.cs)
  and `SetOptOutAsync(statusId, organizationId)`. The org check runs
  in the same DB context as the mutation so an operator cannot reach
  into another organisation's rows by guessing a status id, and the
  refusal-while-busy rail (`Skip` refused while `InProgress`) is
  preserved verbatim from the org-less overload.
- **CSV export** at `/api/agent-upgrade/export.csv`
  ([`AgentUpgradeExportController`](Server/API/AgentUpgradeExportController.cs)),
  same auth + org scope as the page. UTF-8 with BOM (so Excel opens
  it without the encoding wizard), RFC 4180 escaping for commas /
  quotes / newlines, ISO 8601 `'u'` UTC timestamps, hard-capped at
  50,000 rows per request to keep the response bounded.
- **Nav menu** entry under the Admin section in
  [`NavMenu.razor`](Server/Components/Layout/NavMenu.razor) so the
  dashboard is one click away.

**M4 tests.** Twelve new tests in
[`AgentUpgradeServiceTests`](Tests/Server.Tests/AgentUpgradeServiceTests.cs)
cover the listing query (org scoping, missing-device left-join,
case-insensitive search across DeviceId + DeviceName, newest-first
pagination, blank-org / non-positive-take guards), the count query
(matches the listing's filter, blank-org guard), and the org-scoped
operator action overloads (cross-org refused, blank-org refused,
unknown-id refused, in-progress opt-out refused, happy-path resets
attempts + clears LastAttemptError + stamps EligibleAt = now). Three
new tests in
[`AgentUpgradeExportControllerTests`](Tests/Server.Tests/AgentUpgradeExportControllerTests.cs)
pin the CSV format: header line + UTF-8 BOM, RFC 4180 escaping of
commas / quotes / newlines, and null-field empty rendering.

**M5 — Tests & docs.**
- `LegacyToV2ConverterTests` — golden-vector fixtures for the upstream
  schema (one per known upstream release) round-trip into v2.
- `AgentUpgradeOrchestratorTests` — deterministic clock-driven tests for
  the 60-day cut-off, the on-connect dispatch path, the retry/backoff
  state machine, and refusal-while-busy.
- `Setup-Wizard.md` operator guide + `Migration.md` admin guide.

---

## Band 2 — Feature work to carry forward

This band is the existing PR series. Items already shipped stay as
historical record; pending items (C2, C3, D, E) are **re-targeted at the
clean-room codebase** rather than the legacy one — they will land *after*
the relevant clean-room module owns the surface area they touch.

## ✅ PR A — Per-device installed-applications inventory + uninstall

- Org-scoped `PackageManagerEnabled` toggle on `Organization`.
- `PackageManagerRequired` authorization policy + requirement handler.
- Agent-side `IInstalledApplicationsProvider` (Windows registry + AppX) with
  silent uninstall via `msiexec` / cached `UninstallString` / `Remove-AppxPackage`.
- Server-side snapshot cache (`IInstalledApplicationsService`) with single-use
  uninstall tokens — raw uninstall strings never leave the agent.
- Per-device "Installed Applications" page (`/packages/devices/{deviceId}`).

## ✅ PR B — Package Manager shell + sub-nav + Chocolatey *(this PR)*

- Top-level **Package Manager** nav item with sub-menu (Install Packages,
  Deployment Bundles, Executable Builder, Uploaded MSIs, Devices, Job Status).
- Org-scoped `Package`, `DeploymentBundle`, `BundleItem`, `PackageInstallJob`,
  `PackageInstallResult` entities + EF migrations for SQLite/SqlServer/PostgreSql.
- `IPackageService` (CRUD + arg validation that rejects shell metacharacters).
- `IPackageInstallJobService` with an enforced state machine
  (`Queued → Running → Success | Failed | Cancelled`).
- Hub plumbing: `IAgentHubClient.InstallPackage`, `AgentHub.PackageInstallResult`
  with cross-org rejection, `CircuitConnection.QueueInstallPackage` /
  `QueueDeploymentBundle`.
- Agent: `IPackageProvider`, `ChocolateyPackageProvider` (Windows; safe argv,
  no shell), `ChocolateyOutputParser`, `NotSupportedPackageProvider`.
- Razor pages for browsing/creating packages, defining bundles, dispatching
  jobs, and watching job status (live-refreshed via the messenger bus).
- Tests: `ChocolateyOutputParserTests`, `PackageInstallJobServiceTests`
  (state-machine + cross-org), `PackageServiceTests` (validation).

---

## ✅ PR C1 — Silent MSI upload + install *(this PR)*

- **`UploadedMsi`** entity (org-scoped, FK to `SharedFile`). Upload via Razor form
  with **SHA-256** + magic-byte validation (`D0 CF 11 E0 A1 B1 1A E1` MSI / OLE2
  header), max-size cap (2 GiB), org-scoped dedupe by SHA-256.
- **`IUploadedMsiService`**: CRUD + tombstone-then-purge workflow so deletes
  cannot orphan in-flight `PackageInstallJob`s.
- **`MsiFileValidator`** in `Shared` — magic-byte + SHA-256 + filename
  sanitisation helpers shared by server (on upload) and agent (on download).
- **Agent: `MsiPackageInstaller`** — fetches via short-lived `X-Expiring-Token`,
  re-checks magic bytes, re-hashes SHA-256, runs:
  ```
  msiexec /i <file> /qn /norestart /L*v <log>
  ```
  Returns the verbose log tail on failure. Recognises `0`, `3010`, `1641` as
  success exit codes.
- **`CompositePackageProvider`** routes by `PackageProvider` so the hub keeps a
  single `IPackageProvider` dependency.
- **`CircuitConnection.DispatchJobAsync`** mints a 5-minute expiring token and
  populates `MsiSharedFileId` / `MsiAuthToken` / `MsiSha256` / `MsiFileName`
  on the wire when `Provider == UploadedMsi`.
- WebUI under **Uploaded MSIs**: list, upload, delete, register-as-Package, and
  *Send to device* (Windows-only, online devices).
- Deletes are **tombstoned** — only purged after no in-flight jobs reference them.
- EF migrations for SQLite, SQL Server, and PostgreSQL.

## 🔜 PR C2 — Executable Package Builder + Deployment Bundles

- **`ExecutablePackage`** entity: `Name`, `DownloadUrl` (or uploaded blob),
  `SilentArgs`, `SuccessExitCodes` (default `0,3010,1641`), optional `SHA-256`.
- **`DeploymentBundle`** is extended to accept ordered items of any of three
  kinds: Chocolatey id / `UploadedMsi` ref / `ExecutablePackage` ref, plus a
  `StopOnFirstFailure` flag.
- **Run bundle** issues a single `BundleRunJob` to the agent, which executes
  items sequentially and returns per-item structured results.
- WebUI: drag-and-drop ordering, per-item status badges, *retry-failed-only*.

## 🔜 PR C3 — Device lifecycle management (manual + automatic cleanup)

The current implementation does not allow operators to remove devices that are
not actively connected to the server. When a computer is wiped and reprovisioned
it returns with a new device ID, leaving the previous record behind. Over time
this clutters the database with "dead" devices that will never reconnect.

- **Manual delete**: add a *Delete device* action (org-admin scoped) on the
  Devices grid and the per-device page. Deletion must:
  - Tombstone (soft-delete) the device first, then hard-delete after any
    in-flight jobs (`PackageInstallJob`, `BundleRunJob`, MSI uploads, scripts,
    file transfers) referencing it have drained or been cancelled.
  - Cascade-clean dependent rows: installed-applications snapshots, uninstall
    tokens, alerts, scripts results, audit-log references (preserve audit rows
    but null the FK).
  - Refuse deletion while the device is `Online` unless the operator passes an
    explicit *Force* confirmation; a forced delete also revokes the agent's
    auth so it cannot silently re-register under the same record.
  - Be recorded in the audit log added in PR D (actor, device id, reason).
- **Automatic cleanup**: org-scoped setting
  `InactiveDeviceRetentionDays` (default *disabled*; min 7, max 3650).
  - Background `IHostedService` (e.g. `InactiveDeviceCleanupService`) sweeps
    nightly and tombstones devices whose `LastOnline` is older than the
    retention window, then hard-deletes after a grace period.
  - Per-device opt-out flag (`ExcludeFromAutoCleanup`) for stationary kiosks
    that are intentionally offline for long periods.
  - Surface the policy and last sweep timestamp on the Org settings page; emit
    an audit-log entry per automatic deletion.
- **Bulk action**: multi-select on the Devices grid with a *Delete selected
  offline devices* button, gated by the same authorization policy.
- Tests: service-level tests for the state machine (online refusal, tombstone
  → purge, FK cascade, audit emission) and a deterministic clock-driven test
  for the cleanup sweeper.

## 🔜 PR D — Hardening pass *before* the agent rewrite *(re-scoped — see Track S)*

> **Note (Apr 2026):** the items originally batched under PR D have been
> promoted into the cross-cutting **Track S — Security & supply-chain
> baseline** in Band 1 and individually pulled forward into the modules
> where they naturally belong:
> audit log → Module 3 (`Server.Services`); per-org install-job rate
> limits → Module 4 (`Server.Hubs`); signed uploaded-MSI download URLs
> → slice R6; full-surface CodeQL re-run → Track S / S6 (weekly
> scheduled run on `main`); CSP review → Track S / S7 (ships with the
> server rewrite). PR D remains in the roadmap as a historical pointer;
> it is no longer a single PR.

- **Audit log**: every install / uninstall / upload / bundle-run gets an
  immutable row recording actor, target device, package, result, and the tail
  of the agent log.
- **Rate-limit** per-org install jobs to prevent runaway dispatches.
- **Sign uploaded-MSI download URLs** with a short TTL + device-scoped HMAC so
  a leaked URL is unusable elsewhere.
- **CSP review** for the new Razor pages.
- Re-run **CodeQL** on the full feature surface.

## 🔜 PR E — Agent deployment redesign *(last, per project owner instruction)*

This addresses the long-standing complaint about the brittle, templated
PowerShell installer:

- Replace the templated `Install.ps1` with a **versioned native installer**
  (Windows: signed MSI; Linux: `.deb` / `.rpm`; macOS: notarized `.pkg`).
- Move the bootstrap config from query-string PowerShell substitution to a
  small, signed JSON manifest fetched over TLS by the installer itself.
- Emit a single one-liner deployment URL per organization; the URL points at a
  short-lived signed redirect, not at a giant, secret-bearing script.
- Remove the agent's dependence on PowerShell remoting, simplifying Linux/macOS
  parity.

---

## 🟡 Clean-room redesign / separation track *(lead track)*

> **Priority change (Apr 2026):** this track is no longer a "parallel,
> low-tempo" stream. It is the **lead** track. Per project-owner direction
> (the application is not yet in production), no further feature work
> lands on the legacy .NET agent and the Package Manager polish PRs (C2,
> C3, D, E) re-target at the clean-room codebase rather than extending
> the legacy one.

The original codebase that this fork descends from is licensed permissively but
the project owner wants **CMRemote** to stop being a downstream and become an
independently-derivable product. The goal is to rewrite each module from a
clean specification — preserving wire compatibility where it benefits users
(SignalR hub method names, DTO shapes) but **not** preserving copied
implementation. No copyrighted code from the upstream is to be retained.

### Approved language and project-shape decisions

After the language / new-project review, the following are now the working
direction for this track:

- **Agent → Rust.** The agent runs privileged on every endpoint 24/7 and is
  the single biggest win: a Rust rewrite removes the in-process
  `Microsoft.PowerShell.SDK` attack surface, drops idle RSS into the low MB
  range, ships as a single static binary in the low-MB range (vs. ~70–100 MB
  self-contained .NET), makes the unsafe boundary explicit and lintable
  (`cargo-geiger`, Miri, `cargo audit` / `deny` / `vet`, `cargo-fuzz`), and
  lets the PR B job state machines (`Queued → Running → Success | Failed |
  Cancelled`) be enforced by the type system.
- **Server → stay on .NET 8/9.** Razor + Blazor + EF Core + SignalR +
  Identity is a four-for-one win that no other ecosystem matches without a
  much larger rewrite. The clean-room risk on the server is *provenance*,
  not language; re-authoring C# files from spec satisfies it.
- **One repository, cut a `v2`.** Licence hygiene is solved by the in-tree
  provenance gates already listed under "Definition of done", not by repo
  location. We branch `v1-maintenance` for security-only fixes, make `main`
  the home of the clean-room rewrite behind the `scripts/check-provenance` CI
  gate, and tag the first all-clean build `v2.0.0`. A polyglot monorepo
  (Rust agent + .NET server) is the standard shape for this product class
  and actively helps wire-spec discipline.
- **Ship the legacy .NET agent in parallel** with the Rust agent until the
  Rust agent reaches feature parity on Windows. Then deprecate the .NET
  agent in a single release and remove it one release later.

### Guiding principles

1. **Spec-first**: each module gets a brief written spec (inputs, outputs,
   invariants) before code is touched.
2. **Clean-room author**: spec authors and re-implementers are different
   contributors where possible.
3. **No file copies**: rewritten files start empty; tests are written first to
   pin the contract.
4. **Attribution & license hygiene**: any retained third-party snippet must be
   re-vendored from its authoritative source with the original notice intact.
5. **Refactor for efficiency** as a side benefit of the rewrite — async-by-default,
   trim hot allocations on the hub, replace hand-rolled caches with
   `IMemoryCache` / `HybridCache`, fold duplicate registry-walking helpers, and
   cut non-load-bearing dependencies.

### Module-by-module plan

| # | Module | Strategy | Sequencing |
|---|---|---|---|
| 0 | ✅ **Wire Protocol Specification** *(this PR)* | Versioned spec (`docs/wire-protocol.md`) pinning the WebSocket-over-TLS transport, SignalR handshake / invocation / completion / ping / close envelopes, ConnectionInfo on-disk format, reconnect/backoff semantics, and a normative **Security model** (TLS floor, bearer-token + per-device verification-token handling, on-disk secret hygiene with `0600` enforcement, input validation, replay/ordering rules). Test-vector corpus under `docs/wire-protocol-vectors/` (connection-info valid/invalid, handshake, envelope) is already consumed by the Rust crate; the .NET conformance runner is queued for slice R2a. | **Earliest of all** — unblocks both the Shared rewrite and the Rust agent. |
| 1 | `Shared` (DTOs, enums, helpers) | Re-derive types from the wire spec from #0; rewrite serializer-friendly DTOs; replace ad-hoc `Result<T>` with a single tested implementation. | After #0. |
| 2a | **Agent contract freeze** | Lock the agent ↔ server method surface (`IAgentHubClient`, `AgentHub` callbacks) into the spec from #0 with a backwards-compat negotiation field. Add server-side conformance tests that replay the test-vector corpus. | After #0/#1. |
| 2b | **`agent-rs/` — Rust agent re-implementation** | New Cargo workspace under `agent-rs/`. Crates: `cmremote-wire` (DTOs + serde/rmp-serde), `cmremote-platform` (per-OS trait impls), `cmremote-agent` (binary). Implement in slices: connection/heartbeat → device info → script execution → installed-applications → package manager → desktop transport (last). Ship behind a feature flag and an opt-in `agent-channel` per device until parity. | After #2a; runs in parallel with #3–#6 once the protocol is frozen. |
| 2c | Legacy .NET agent (`Agent/`) | Maintenance-only while #2b ramps. Once the Rust agent reaches Windows parity, deprecate for one release, then remove. | Parallel with #2b. |
| 3 | `Server.Services` (data, auth, circuit, scripts) | Split monolithic `DataService` into focused services (`IDeviceQueryService`, `IDeviceCommandService`, `IUserDirectoryService`); rewrite each from spec. | After #1. |
| 4 | `Server.Hubs` (`AgentHub`, `ViewerHub`, `CircuitConnection`) | Rewrite the dispatch layer using a generated client interface; remove duplicate authorization checks; centralize org-scope assertions. | After #3. |
| 5 | `Desktop` / remote-control transport | Rewrite WebRTC / IceServer plumbing against a written protocol doc; consider switching to `Microsoft.MixedReality.WebRTC` or `SIPSorcery` to eliminate inherited code. The Rust agent's desktop transport (#2b last slice) tracks the same protocol doc. | Parallelizable with #4. |
| 6 | `Server` Razor UI | Rebuild the layout shell (`MainLayout`, `NavMenu`) from scratch with a CMRemote design system. **Adopt the crashmedia.ca colour scheme** — see [Band 3 — UI / brand alignment](#band-3--ui--brand-alignment) below for the palette and tokens that this rebuild must use. Per-page Razor logic is rewritten module-by-module. The Package Manager pages added in PR B are already CMRemote-original and stay (they are restyled against the new tokens but not re-authored). | Last — depends on stable services. |
| 7 | Installer / agent deployment | Covered by PR E above; the Rust agent simplifies this dramatically (single static binary → MSI / `.deb` / `.rpm` / `.pkg` wrappers). | After #2b reaches Windows parity. |

### Rust agent (`agent-rs/`) — slice-by-slice delivery plan

Implementation order for Module 2b. Each slice ships behind a per-device
`agent-channel` opt-in (`stable-dotnet` | `preview-rust`) so the legacy .NET
agent and the Rust agent can run side-by-side until parity.

| Slice | Scope | Exit criteria |
|---|---|---|
| **R0 — Workspace scaffold** ✅ | `agent-rs/Cargo.toml` workspace; crates `cmremote-wire`, `cmremote-platform`, `cmremote-agent`; structured logging (`tracing`); config loader for `ConnectionInfo.json` + CLI args; signal handling; CI (`cargo fmt`, `cargo clippy -D warnings`, `cargo test`). No network I/O yet. | Workspace builds clean on stable Rust. CI green. Provenance header on every file. |
| **R1a — Wire types + JSON test vectors** ✅ *(shipped in PR #5)* | `cmremote-wire`: `ConnectionInfo`, hub envelopes (`HubInvocation` / `HubCompletion` / `HubPing` / `HubClose`), JSON round-trip, and a hand-written redacting `Debug` for `ConnectionInfo` so the verification token cannot leak via logs or panics. Corpus consumption via `tests/vectors.rs` (positive + negative connection-info, handshake, envelope). | All JSON vectors round-trip byte-for-byte; `cargo test` green on all three OSes. |
| **R1b — MessagePack codec** ✅ | `rmp-serde` added to `cmremote-wire` with public `to_msgpack` / `from_msgpack` helpers funnelled through `WireError`. Every JSON vector in the corpus also round-trips byte-stably through MessagePack (`connection_info_valid_vectors_round_trip_through_msgpack`, `envelope_vectors_round_trip_through_msgpack`). Shipped alongside the Track S / S1–S2 security gates so the `cargo-deny` / `cargo-audit` / `dependency-review` stack caught the new dependency on the way in. Track S / S4 (fuzz targets + `proptest` suite + nightly workflow) followed in a separate PR and closed the slice R1 parser-hardening work. | All vectors round-trip byte-for-byte across both encodings; `cargo deny check` green on the new dep. |
| **R2 — Connection / heartbeat loop** ✅ | WebSocket transport (`tokio-tungstenite` over `rustls` + `aws-lc-rs`, no `ring`) speaking the SignalR JSON/MessagePack hub protocol re-derived from spec; `wss://`-only floor enforced at request build time; `Authorization: Bearer <OrganizationToken>` + `X-Device-Id` + `X-Protocol-Version` + optional `X-Server-Verification` headers attached on every upgrade; `Sec-WebSocket-Protocol` negotiation pinned to `json` / `messagepack`; SignalR handshake (typed `HandshakeRequest` / `HandshakeResponse`) over the new `0x1E`-record / varint-length frame readers shared with the .NET conformance runner; 15 s ping + 30 s idle-timeout heartbeat with `1011` close + reconnect; jittered exponential backoff (base 1 s → cap 60 s, full jitter, reset on successful handshake); top-level `transport::run_until_shutdown` driver wired into `runtime::run` with cooperative shutdown via `tokio::sync::watch`. Coverage: 37 unit tests across `transport::backoff` / `transport::connect` / `transport::session` (URL builder, header redaction, `wss://` floor, sub-protocol negotiation, framing, heartbeat shape, jitter bounds + reset semantics) plus 5 end-to-end loopback integration tests in `tests/transport_loopback.rs` (handshake round-trip, inbound record + server-Close → reconnect, local shutdown, typed handshake-rejection path, ping/idle-window relationship). | `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo deny check` green on the new TLS dependency tree. |
| **R2a — Hub dispatch surface** ✅ | `HubEnvelope` enum + `decode_envelope` in `cmremote-wire`; `InvocationTracker` per-connection dedup guard; `MethodName` allow-list enum (16 server→agent methods); `dispatch_record` router; `HubCompletion::ok/err` constructors; outbound-message injection channel wired into `run_session` (`mpsc::Receiver<Message>` arm in the `select!` loop); `HubClose { allowReconnect: false }` decoded → `SessionExit::Quarantined`; stubs for R6–R8 methods (`InstallPackage`, `ChangeWindowsSession`, `RemoteControl`, …) return `not_implemented`; wire-protocol method-surface vectors under `docs/wire-protocol-vectors/method-surface/` pin every method's argument and completion shape. Integration tests updated. | All 114 workspace tests green; clippy clean. |
| **R3 — Device information** ✅ | `DeviceSnapshot` (16 fields matching `DeviceClientDto`) replaces the R0 stub `HostDescriptor`; `DeviceInfoProvider::snapshot(device_id, org_id)` updated signature. `LinuxDeviceInfoProvider` (cfg `target_os = "linux"`) reads `/proc/meminfo` (total + used RAM), `/proc/stat` (CPU utilisation via two-sample delta), `/sys/class/net/*/address` (MAC addresses), `df -k` (drive list), `/etc/os-release` (OS description), `std::thread::available_parallelism()` (CPU count), hostname via `/etc/hostname` / env. `StdDeviceInfoProvider` delegates to `LinuxDeviceInfoProvider` on Linux. `TriggerHeartbeat` hub handler serialises the snapshot and sends it to the server. | `cargo test` green on Linux; smoke test confirms non-empty hostname + non-zero CPU count; `DeviceInfoProvider` trait object-safe. |
| **R4 — Process / script execution** ✅ | `ExecuteCommandArgs` + `ScriptResult` + `ScriptingShell` DTOs in `cmremote-wire`; `tokio::process::Command` executor in `cmremote-agent::handlers::script`; per-OS shell map (`bash`, `sh`, `zsh`, `pwsh` — `cmd` returns `not_supported` on non-Windows); 5-minute timeout via `tokio::time::timeout`; stdout + stderr captured; `ExecuteCommand` hub handler wired into the dispatcher. `"process"` feature added to workspace `tokio` dep. | Unit tests cover shell resolution, successful echo execution, timeout enforcement, unknown-shell error path. |
| **R5 — Installed-applications provider** ✅ | `InstalledApplicationsProvider` trait + `InstalledApp` DTO in `cmremote-platform`; `DpkgProvider` (cfg `target_os = "linux"`) runs `dpkg-query --show` and falls back to `rpm -qa`; `uninstall` calls `apt-get remove -y` or `rpm -e`; `NotSupportedAppsProvider` stub for Windows/macOS. `RequestInstalledApplications` and `UninstallApplication` hub handlers wired into the dispatcher. | Unit tests parse mock `dpkg-query` output; not-supported stub confirmed. |
| **R6 — Package manager (Chocolatey + MSI + Exe)** ✅ | `cmremote-wire`: `PackageProvider` / `PackageInstallAction` / `PackageInstallRequest` / `PackageInstallResult` PascalCase wire types — defaults to `Unknown` so a malformed payload fails closed. `cmremote-platform::packages`: `PackageProviderHandler` async trait + safety helpers — `is_safe_chocolatey_package_id`, `is_safe_chocolatey_version`, `is_safe_msi_file_name`, `is_msi_magic_bytes` (OLE2 `D0 CF 11 E0 A1 B1 1A E1`), `compute_sha256_hex`, `ct_eq_hex` constant-time compare, `is_chocolatey_success_exit_code` matching `Shared.PackageManager.ChocolateyOutputParser.SuccessfulExitCodes`. **Concrete drivers** ship behind injected `ProcessRunner` + `ArtifactDownloader` traits so the orchestration is unit-tested on Linux CI: `ChocolateyPackageProvider` (allow-listed argv, exit-code mapping including `3010` reboot-required, 30-min timeout, output cap), `UploadedMsiPackageProvider` (download → OLE2 magic + SHA-256 re-verify → `msiexec /i` argv → 60-min run → log-tail capture on failure → unconditional cache cleanup), `ExecutablePackageProvider` (download → SHA-256 verify → exec with operator-vetted silent-install args, install-only, refuses uninstall). The runtime registers all three via `CompositePackageProvider::register_default_handlers(...)`. The **rustls-only HTTPS [`ReqwestArtifactDownloader`](agent-rs/crates/cmremote-platform/src/packages/reqwest_download.rs)** is now wired in (`reqwest` 0.12 with `rustls-tls-webpki-roots-no-provider` + `aws-lc-rs` installed via `packages::install_default_crypto_provider`) — no `ring`, no `openssl-sys` (both banned in [`agent-rs/deny.toml`](agent-rs/deny.toml)). It honours the `ArtifactDownloader` security contract end-to-end: HTTPS-only (helper + `https_only(true)` belt-and-braces), per-call wall-clock timeout via `tokio::time::timeout`, `max_bytes` enforced via `Content-Length` pre-check **and** in-stream chunk accounting (catches servers that lie about size), partial-file cleanup on size overflow / timeout / transport error, and `auth_header` marked `set_sensitive(true)` so the value never reaches `tracing` / panic / `Debug`. The error redactor returns a fixed category string ("transport timeout" / "connection failed" / …) so a leaked log line cannot contain the URL or the token. Agent-side `InstallAgentUpdate(downloadUrl, version, sha256)` (slice M3) is a working handler that re-uses the same downloader to fetch + SHA-256 verify the staged artifact and hands off to a pluggable `AgentUpdateInstaller` (default `StubAgentUpdateInstaller` returns a structured "no installer configured" failure so the manifest dispatcher's audit trail stays honest). | Workspace builds clean on stable Rust; `cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace --all-targets` (254 tests) all green; `cargo deny check` clean — bans, advisories, licences, sources all OK. |
| **R7 — Desktop transport** *(🟡 wire surface, async trait, `NotSupportedDesktopTransport` safety stub, security-contract enforcement (R7.b), capture/encode trait surface (R7.c), method-surface vectors (R7.d), WebRTC crypto-provider ADR (R7.f), signalling DTOs / dispatch (R7.g), and input-injection / consent-prompt trait surface (R7.h) shipped; concrete WebRTC capture/encode driver and per-OS input/consent UI drivers pending — see "remaining work" below)* | `cmremote-wire::desktop`: `RemoteControlSessionRequest` / `RestartScreenCasterRequest` / `ChangeWindowsSessionRequest` / `InvokeCtrlAltDelRequest` / `DesktopTransportResult` — PascalCase DTOs mirroring the .NET `IAgentHubClient` shapes byte-for-byte so the same server can drive either agent fleet, defaults that fail closed, JSON + MessagePack round-trip tests, sensitive `access_key` documented as never-logged. `cmremote-platform::desktop`: [`DesktopTransportProvider`](agent-rs/crates/cmremote-platform/src/desktop/mod.rs) async trait (one method per hub call, all returning the same generic `DesktopTransportResult`) + `NotSupportedDesktopTransport` default that returns a structured failure naming the host OS — never panics, never echoes the access key. The agent's dispatcher routes `RemoteControl`, `RestartScreenCaster`, `ChangeWindowsSession`, and `InvokeCtrlAltDel` invocations through the provider instead of falling through to the generic `not_implemented` completion; malformed `arguments` payloads become structured failures rather than wire-level errors so the audit trail records the bad payload. **Slice R7.b** lifts the security contract out of the trait doc and into [`cmremote-platform::desktop::guards`](agent-rs/crates/cmremote-platform/src/desktop/guards.rs): `check_remote_control` / `check_restart_screen_caster` / `check_change_windows_session` enforce cross-org refusal (`request.org_id` must match the agent's own `ConnectionInfo::organization_id`), operator-string sanitisation (256-byte cap, refuse non-printable code points / embedded NUL / DEL, refuse the Unicode bidi-override range that drives "Trojan Source" attacks), and canonical lowercase-UUID `session_id` checks (the only shape the .NET hub ever emits). The `NotSupportedDesktopTransport` stub now runs the guards *before* its OS-not-supported branch, so a hostile request never reaches the per-OS error path; concrete WebRTC drivers will reuse the same helpers. The runtime constructs the stub with the agent's own org id from `ConnectionInfo`. **Slice R7.c** adds [`cmremote-platform::desktop::media`](agent-rs/crates/cmremote-platform/src/desktop/media.rs): `DesktopCapturer` + `VideoEncoder` async traits, `CapturedFrame` (BGRA, stride, μs timestamp) + `EncodedVideoChunk` (bytes, μs, keyframe flag) DTOs, `DesktopMediaError` (`NotSupported(HostOs)` / `Io` / `InvalidParameters`), and `NotSupportedDesktopCapturer` / `NotSupportedVideoEncoder` defaults that return structured errors. Same "trait first, real driver later" pattern slice R6 used for packages — keeps the seam reviewable without pulling in any banned dependencies. **Slice R7.d** ships frozen byte-stable wire-protocol vectors under [`docs/wire-protocol-vectors/method-surface/{remote-control,restart-screen-caster,change-windows-session,invoke-ctrl-alt-del}/`](docs/wire-protocol-vectors/method-surface/) — request + result vectors per method, all round-tripped through both JSON and MessagePack by [`cmremote-wire/tests/vectors.rs`](agent-rs/crates/cmremote-wire/tests/vectors.rs). The wire is now pinned: a future WebRTC driver swap cannot regress the contract. **Slice R7.f** ships [`docs/decisions/0001-webrtc-crypto-provider.md`](docs/decisions/0001-webrtc-crypto-provider.md) — a maintainer-facing ADR laying out the trade-offs between (a) admitting `ring` for the upstream [`webrtc`](https://crates.io/crates/webrtc) crate, (b) forking `webrtc` onto `aws-lc-rs`, and (c) writing a thinner SRTP / DTLS / SCTP stack on top of `aws-lc-rs` directly, plus the security-review questions maintainers must answer before any of those options is actionable. The `deny.toml` ban on `ring` stays in place; the policy bit is unflipped. **Slice R7.g** ships the WebRTC signalling contract: `SdpOffer` / `SdpAnswer` / `IceCandidate` PascalCase DTOs in [`cmremote-wire::desktop::signalling`](agent-rs/crates/cmremote-wire/src/desktop/signalling.rs) with explicit `Kind` discrimination, 16 KiB SDP-body cap (`MAX_SDP_BYTES`) and 1 KiB per-line cap (`MAX_SIGNALLING_STRING_LEN`); matching `check_sdp_offer` / `check_sdp_answer` / `check_ice_candidate` guard helpers that reuse the slice R7.b envelope checks; `on_sdp_offer` / `on_sdp_answer` / `on_ice_candidate` hooks on `DesktopTransportProvider` (default-impl returns "not implemented by this provider", `NotSupportedDesktopTransport` overrides to run guards then return the OS-not-supported failure); dispatch routing for the three new hub methods (`SendSdpOffer` / `SendSdpAnswer` / `SendIceCandidate`); and frozen byte-stable vectors under [`docs/wire-protocol-vectors/method-surface/signalling/`](docs/wire-protocol-vectors/method-surface/signalling/) (offer / answer / candidate / RFC-8838 end-of-candidates marker / success + failure result), all round-tripped through both JSON and MessagePack. Lets the .NET side land its half of the WebRTC negotiation surface against a stable contract while the agent-side WebRTC backend is still gated on the R7.f ADR. **Slice R7.h** adds the input-injection and on-host consent trait surface — same "trait first, real driver later" shape R7.c used for capture/encode. [`cmremote-platform::desktop::input`](agent-rs/crates/cmremote-platform/src/desktop/input.rs) ships `MouseInput` (move/down/up/scroll), `KeyboardInput` (down/up/type-text), and `Clipboard` (read/write text) async traits with `MouseButton` (numbered to match Win32 `XBUTTON*`), `ScrollAxis`, `KeyCode::Char` / `KeyCode::Named(NamedKey)` (every common modifier + arrows + `F(u8)`), and a `DesktopInputError` (`NotSupported(HostOs)` / `ConsentDenied` / `Io` / `InvalidParameters`) whose `Io` variant contractually MUST NOT carry typed-text or clipboard payload bytes. Three `NotSupported*` defaults fail closed naming the host OS. [`cmremote-platform::desktop::consent`](agent-rs/crates/cmremote-platform/src/desktop/consent.rs) ships a `ConsentPrompter` async trait, a `ConsentRequest` DTO whose [`sanitised`] constructor reuses [`super::guards::validate_operator_string`] so a hostile org name with bidi-overrides / NUL / DEL / over-length payload never reaches the on-host prompt, a `ConsentDecision` (`Granted` / `Denied { reason }` / `Timeout`), a `DEFAULT_CONSENT_TIMEOUT = 30 s` mirroring the .NET implementation, and a fail-closed [`DenyAllConsentPrompter`] default the runtime registers when no per-OS UI is wired in (a session reaches a host with no consent UI configured and is *refused*, not silently allowed). All R7.h types are exported through the `desktop` module's prelude. No new dependencies; no banned crates; no wire churn. **Remaining work for the final R7 acceptance bar** (each item now has a clearer next step thanks to R7.f / R7.g / R7.h): (1) maintainer decision on the [crypto-provider ADR](docs/decisions/0001-webrtc-crypto-provider.md), then the corresponding `agent-rs/deny.toml` change; (2) the actual WebRTC peer-connection driver wired through `DesktopTransportProvider` (the R7.g signalling hooks are the seam it plugs into), with per-OS capture (DXGI Desktop Duplication / ScreenCaptureKit / PipeWire) and encode (Media Foundation / VideoToolbox / VAAPI / libx264) implementations of the R7.c traits; (3) per-OS implementations of the R7.h input traits (`SendInput` on Windows, `uinput` / `XTest` on Linux, Quartz Event Services on macOS) and a real GUI consent prompter for each host (Win32 dialog / GTK / Cocoa) replacing `DenyAllConsentPrompter`; (4) the .NET hub-side glue that emits / consumes the R7.g signalling DTOs through the `SendDtoToClient` / `SendDtoToViewer` wrapper; (5) a reference benchmark harness running both the Rust agent and the .NET Desktop client on identical hardware to demonstrate the latency/FPS bar. Concrete WebRTC drivers (capture → encode → ICE → SCTP) plug in alongside the stub without any further dispatch or wire churn. Tracks Module 5's protocol doc. | Slices R7 (initial) + R7.b + R7.c + R7.d + R7.f + R7.g + R7.h: workspace tests green (345; was 325 → +20 R7.h tests across input traits — structured-error shape for mouse / keyboard / clipboard `NotSupported*`, object-safety, default-uses-current-host, `MouseButton` numeric repr pin, `KeyCode` equality — and consent — fail-closed `DenyAllConsentPrompter`, sanitised-constructor refusal of bidi-override / NUL / over-length / empty / control-character inputs, `with_timeout` zero-refusal, `is_granted` discriminator, prompter object-safety). Final acceptance bar (unchanged): latency / FPS within 10 % of the .NET/Desktop client on a reference workload. |
| **R8 — Installer wrappers** ✅ | **Publisher manifest** ([`docs/publisher-manifest.md`](docs/publisher-manifest.md), schema-versioned JSON, JSON-Schema validated, sample manifests under [`docs/publisher-manifest-samples/`](docs/publisher-manifest-samples/)) is the contract every concrete installer hangs off. **Linux** `.deb` (`cargo-deb`) and `.rpm` (`cargo-generate-rpm`) packaging metadata in `agent-rs/crates/cmremote-agent/Cargo.toml` against a hardened systemd unit (`NoNewPrivileges`, `ProtectSystem=strict`, `RestrictAddressFamilies`, `SystemCallFilter=@system-service`). **Windows MSI** scaffold via `cargo wix` ([`agent-rs/packaging/wix/main.wxs`](agent-rs/packaging/wix/main.wxs)) — perMachine install, `CMRemoteAgent` Windows service. **macOS `.pkg`** scaffold ([`agent-rs/packaging/macos/build-pkg.sh`](agent-rs/packaging/macos/build-pkg.sh)) — universal2 binary, `_cmremote` LaunchDaemon. **Release workflow** ([`.github/workflows/release.yml`](.github/workflows/release.yml)) on a `v*` tag builds every target, signs every artifact with **Sigstore cosign keyless** (this also discharges Track S / S5 for the Linux artifacts), emits the publisher manifest, validates against the JSON schema, and uploads everything as a draft release. **Server-side resolver**: `PublisherManifest` parser (constant-time SHA-256 hex compare; refuses path-traversal in file names), `PublisherManifestProvider` (HTTP/file fetch + per-channel cache), and `ManifestBackedAgentUpgradeDispatcher` (maps `Device.Platform` + `OSArchitecture` → `(target, format)`, picks the unique manifest entry, refuses already-on-target devices). **Server-side dispatch**: `DispatchAsync` looks up the device's connection ID via `IAgentHubSessionCache`, fails-fast (recoverably) on offline, refuses non-`https://` artifact URIs, pushes the new `IAgentHubClient.InstallAgentUpdate(downloadUrl, version, sha256)` hub method to the device, and polls the session cache for the device's `AgentVersion` to flip to the target version (the orchestrator's `DispatchTimeout` caps the total wait through the cancellation token). Wired into `Program.cs` so when at least one channel URL is configured the dispatcher drives real upgrades end-to-end. **Agent-side**: legacy .NET agent registers a logged no-op (it keeps PR E's polling updater path); Rust agent reserves an `InstallAgentUpdate` slot in `MethodName` so the wire layer accepts the method (the concrete download/verify/install driver shares R6's fetch-verify-execute path and lights up the slot once R6's drivers ship — flipping a `not_implemented` to a real handler is a one-file change). Tests in `PublisherManifestParserTests`, `PublisherManifestProviderTests` (filesystem + HTTP sources, cache TTL, channel-mismatch refusal, network-failure handling), and `ManifestBackedAgentUpgradeDispatcherTests` (platform→target routing for Windows/macOS/Linux deb/Linux rpm/aarch64/unknown, multi-match refusal, already-on-target short-circuit, RequireSignature gate, default-channel resolution, unknown-device tolerance, and the new dispatch path: offline → recoverable fail, hub-call exception → fail, non-`https://` URI → refuse, version-bump heartbeat → success, no-bump-before-timeout → cancellation). | One-liner deploy URL produces a working agent on each OS without PowerShell. |

### Definition of done for the separation track

- [ ] Every file under `Agent/`, `Server/`, `Shared/`, `Desktop*/`, and
      `agent-rs/` either has a `// Source: CMRemote, clean-room
      implementation` provenance header or is a vendored third-party file
      with its original notice.
- [ ] An explicit `THIRD_PARTY_NOTICES.md` enumerates every retained snippet,
      its origin, and its license.
- [ ] `git log --diff-filter=A` shows that all production source files in the
      repo were authored in the CMRemote tree (no inherited blob hashes from
      the upstream).
- [ ] CI gates the above with a `scripts/check-provenance` step.
- [ ] A short `LICENSE` change announces CMRemote as the copyright holder for
      the rewritten code, while preserving any third-party notices.

### Why this protects the project

The upstream license already permits forking, but copyright strikes typically
target *verbatim* copies of source files — not independent reimplementations
behind the same wire protocol. Following the spec-first / clean-room workflow
above makes any future challenge straightforward to rebut: each file's history
shows it was authored locally against a written contract, not copied.

---

## Band 3 — UI / brand alignment

When the Razor UI is rebuilt as part of clean-room **Module 6**, the
application's visual language is realigned with the public
**crashmedia.ca** site so the admin panel reads as part of the same
product family.

> ⚠️ The hex values in the table below were extracted from a remote
> palette-sampler service (the implementing PR could not pull
> crashmedia.ca directly from the build environment at the time the
> roadmap entry was written). **The implementing PR must re-sample the
> live site** at the time of the work and reconcile any drift before
> the tokens are baked into the design system.

### Provisional palette (crashmedia.ca, Apr 2026)

| Role | Token | Hex (provisional) |
|---|---|---|
| Brand primary | `--cm-brand-500` | `#6464f4` |
| Brand primary (hover) | `--cm-brand-600` | `#6463d7` |
| Brand accent (purple) | `--cm-accent-500` | `#875ce9` |
| Brand secondary (royal) | `--cm-brand-400` | `#7084f8` |
| Brand tint (light) | `--cm-brand-100` | `#bbbade` |
| Brand tint (lighter) | `--cm-brand-050` | `#babcfb` |
| Surface — page background | `--cm-surface-bg` | `#040515` |
| Surface — panel | `--cm-surface-panel` | `#2d313e` |
| Surface — raised | `--cm-surface-raised` | `#3c4452` |
| Surface — accented panel | `--cm-surface-accent` | `#2e305f` |
| Text — primary | `--cm-text-primary` | `#bbbade` |
| Text — muted | `--cm-text-muted` | `#7c8493` |
| Border / divider | `--cm-border` | `#545a67` |

### Scope of the alignment work *(part of Module 6)*

- Define the tokens above as CSS custom properties in a single
  `wwwroot/css/cm-tokens.css` (no per-component duplication).
- Replace the Bootstrap default theme variables (`$primary`,
  `$body-bg`, `$body-color`, `$border-color`, link colours, button
  hover/active states) with the CMRemote tokens via a Sass shim, so
  every existing Bootstrap component picks up the new palette
  automatically.
- Audit all `style=`/inline colour literals (`grep -RIn '#[0-9A-Fa-f]\{3,6\}'
  Server/`) and migrate them to `var(--cm-…)` references.
- Re-run a contrast pass against WCAG AA for the chosen text /
  surface combinations; adjust `--cm-text-muted` if it does not meet
  4.5:1 against `--cm-surface-bg`.
- Replace the existing favicon / brand mark with the CMRemote /
  crashmedia.ca mark in `wwwroot/favicon.ico` and the Razor layout
  `<header>` brand block.
- Tests / acceptance: a Playwright smoke test that renders the login
  page, the Devices grid, and the Package Manager landing page, and
  asserts that the computed background of `<body>` is
  `--cm-surface-bg` and the computed colour of the primary nav link
  is `--cm-brand-500`. (One assertion each; the snapshot's purpose is
  drift detection, not pixel-perfect comparison.)

### Sequencing

The colour-token work lands as the **first sub-PR of Module 6** so the
remaining per-page rewrites in that module can be reviewed against
the final palette rather than a moving target.
