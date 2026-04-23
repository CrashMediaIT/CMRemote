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

### Current focus *(end of slice R1b / S4 / M1 scaffold, Apr 2026)*

Module 0 (wire-protocol spec + JSON test-vector corpus), slice **R1a**
(`cmremote-wire` JSON round-trip + redacting `Debug`), slice **R1b**
(MessagePack codec + byte-stable corpus round-trip), the first
security-gate items **S1** (`SECURITY.md` + coordinated-disclosure
policy), **S2** (supply-chain CI via `cargo-deny`, `cargo-audit`,
`dependency-review`, OSSF Scorecard, Dependabot), **S3**
([threat model document](docs/threat-model.md)), **S4** (fuzzing
and parser hardening — `proptest` suite on stable + `cargo-fuzz`
targets under `agent-rs/crates/cmremote-wire/fuzz/` seeded from the
corpus + nightly scheduled workflow
[`fuzz.yml`](.github/workflows/fuzz.yml)), and the **M1 scaffolding**
slice (empty `/setup` flow + `CMRemote.Setup.Completed` marker stored
in `KeyValueRecords`, redirect middleware that routes uncompleted
setups to `/setup`, and a startup heuristic that auto-marks any
already-populated database so existing deployments are not hijacked
into the wizard) are merged. The slice R1b codec and the S1–S2
supply-chain gates were shipped in one PR by design so the gate caught
the new `rmp-serde` dependency on the way in; S3 followed in the next
PR; S4 landed next, unblocking slice R2; the M1 scaffolding landed
alongside, unblocking M2.

The next milestones, ordered so security work continues to land
alongside functional work rather than behind it, are:

1. **R2 — Connection / heartbeat loop** (Rust agent connects to a dev
   server over WebSocket and reconnects cleanly across restarts). Now
   unblocked — S4's fuzz + proptest coverage gates R2's new parser
   surfaces on the way in.
2. **M2 — Schema converter library** (`CMRemote.Migration.Legacy`
   project + `cmremote migrate` CLI). Now unblocked by the M1
   scaffold; lands the per-version row-converter functions that the
   wizard's import step (M1.3) and headless / scripted migrations
   both consume.

Both milestones are parallelizable.

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

### 🟡 Track S — Security & supply-chain baseline *(cross-cutting — S1 + S2 + S3 + S4 shipped)*

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

**S6 — Secret-hygiene enforcement *(🔜, gated into CI)*.**

- Add **gitleaks** as a PR gate (pre-commit hook + CI job) so
  accidentally-committed tokens fail the build, not the audit log.
- Add a unit test under `cmremote-platform` that asserts
  `ConnectionInfo.json` is written with file-mode `0600` on Unix (the
  spec already requires this; the test pins it) and an equivalent
  ACL check on Windows.
- Extend `ConnectionInfo`'s redacting `Debug` (shipped in slice R1a)
  with a compile-time test (`trybuild` or a straight unit test) that
  formatting the struct never contains the verification-token bytes.
- Periodic **CodeQL** (already in the build workflow for .NET; extend
  to Rust via the official action) scheduled weekly on `main` in
  addition to per-PR runs.

**S7 — Runtime security posture *(🔜, lands with server rewrite)*.**

- Default strict **CSP**, **HSTS** (`includeSubDomains; preload`),
  `X-Content-Type-Options: nosniff`, `Referrer-Policy:
  strict-origin-when-cross-origin`, and a `Permissions-Policy` that
  denies camera/microphone/geolocation by default on every response
  from the Razor server. The WebRTC viewer opts back in on the
  specific routes that need it.
- Per-org **rate limits** on install-job dispatch (was PR D); pulled
  forward to land with Module 4 (`Server.Hubs`) rather than waiting
  until after the agent rewrite.
- **Uploaded-MSI download URLs** signed with a short TTL + device-scoped
  HMAC (was PR D); pulled forward to land with slice R6 so the Rust
  agent never sees an unsigned variant.
- An **immutable audit log** (was PR D) is re-scoped as a Track S
  deliverable and lands with Module 3 (`Server.Services`) so every
  subsequent module inherits it.

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

**M1 — First-boot setup wizard.** *(🟡 in progress — scaffolding shipped.)*
The skeleton landed in this slice: a `/setup` Razor page rendered through
a minimal `EmptyLayout`, an `ISetupStateService` backed by a fixed-Guid row
in `KeyValueRecords` for the `CMRemote.Setup.Completed` marker, a
`SetupRedirectMiddleware` that forwards uncompleted setups to `/setup`
(with framework / static / `.well-known` paths allowlisted and non-GET
requests answered with `503 + Retry-After: 30` so partially-upgraded
clients don't silently drop state), and a startup heuristic that
auto-writes the marker when the database already contains an
organisation, user, or device — so existing deployments are never
hijacked into the wizard on upgrade. The placeholder page lays out the
five steps below so subsequent slices can land incrementally; the steps
themselves are not yet implemented and the page exposes a
*"Mark setup complete"* action so an operator can dismiss the wizard
while the real wizard logic is still being built.

On first start, if no `appsettings` database connection string is configured **and** no
`CMRemote.Setup.Completed` marker row exists, every request is redirected
to `/setup`. The wizard is a small server-rendered flow (no auth — it is
only reachable while the marker is unset; the wizard refuses to load once
the marker is written):

1. **Welcome / preflight** — checks writable data dir, TLS certs, and that
   the bind ports are free.
2. **Database connection** — Postgres-only host / port / db / user /
   password / SSL-mode form. The wizard performs a live `SELECT 1` round
   trip and, on success, writes the connection string to
   `appsettings.Production.json` (file mode `600`) and reloads
   configuration. It does not proceed until the round trip succeeds.
3. **Import existing database** *(optional, shown only when an upstream
   schema is detected on a separate connection — SQLite file path or
   SQL Server / Postgres conn-string from the legacy image)*. Reads the
   legacy schema in batches, maps rows through versioned converter
   functions into the v2 Postgres schema (organisations, users, devices,
   shared files, scripts, alerts, audit). Idempotent, resumable, with a
   live progress page and a written `migration-report.json` artefact.
   Devices are imported with their existing IDs and shared secrets so
   already-deployed agents reconnect under the same record.
4. **Admin bootstrap** *(only if no users were imported)* — creates the
   first organisation + server-admin account.
5. **Done** — writes the `CMRemote.Setup.Completed` marker, signs the
   operator into the main panel, and queues the agent-upgrade sweep
   defined in **M3**.

The wizard is non-blocking past step 3: if the operator skips the import
they can run it later from `/admin/migration`. **The operator is never
forced to wait for agents to upgrade before reaching the main panel.**

**M2 — Schema converter library.** A new `CMRemote.Migration.Legacy`
project owns the read-side schema reflection and the per-version row
converters. It exposes a CLI (`cmremote migrate --from <conn> --to
<conn>`) so headless / scripted migrations are possible and so the
wizard's import step is a thin UI over the same code.

**M3 — Background agent-upgrade pipeline.** Once the operator is in the
main panel, an `IHostedService`
(`AgentUpgradeOrchestrator`) drives the fleet upgrade asynchronously.

- A new `AgentUpgradeStatus` table tracks every device:
  `DeviceId, FromVersion, ToVersion, State, LastAttemptAt,
   LastAttemptError, AttemptCount, EligibleAt, CompletedAt`.
  States: `Pending → Skipped(Inactive) | Skipped(OptOut) | Scheduled
  → InProgress → Succeeded | Failed → (retry) Pending`.
- **60-day inactivity cut-off.** On enrolment into the pipeline, any
  device whose `LastOnline` is older than `UtcNow − 60 days` is moved
  straight to `Skipped(Inactive)` and **not contacted**. The state is
  re-evaluated when the device next reconnects (see below).
- **Online devices** are upgraded in a bounded-concurrency queue
  (default 5 in flight per server, tunable). The upgrade itself reuses
  the **existing** PR E installer surface — the server publishes the
  new agent build, the running legacy agent fetches it over the
  authenticated hub, swaps binary + service definition, and reconnects.
  Success is observed when the device next sends a heartbeat tagged
  with the new agent version.
- **Offline devices** are not contacted while offline. The
  `AgentHub.OnConnectedAsync` path checks the device's
  `AgentUpgradeStatus`: if the row is `Pending` *and* `LastOnline`
  (now updated) is within 60 days, the upgrade is **dispatched the
  instant the device connects**, before any user-facing job is
  delivered to it. If the row is `Skipped(Inactive)` and the device
  has now re-appeared, the row is flipped back to `Pending` and the
  same on-connect dispatch fires.
- **Failure handling.** Failed upgrades are retried with exponential
  backoff (max 5 attempts, capped at 24 h). After exhaustion the row
  stays `Failed` and surfaces in the admin **Agent upgrade** dashboard
  with the device id, last error, and a "Retry" button.
- **Safety rails.** The orchestrator refuses to dispatch an upgrade
  while the device has an in-flight `PackageInstallJob`,
  `BundleRunJob`, script, or remote-control session. It also refuses
  to dispatch if the target build's SHA-256 / signature does not match
  the manifest written by the publisher.

**M4 — Admin "Agent upgrade" dashboard.**
`/admin/agent-upgrade` shows totals (`Pending / Scheduled / InProgress /
Succeeded / Failed / Skipped(Inactive) / Skipped(OptOut)`), a searchable
device table with state + last error + last-online age, per-device
*Retry* / *Skip* / *Force* actions, and a CSV export. The dashboard is
read-mostly and is not on the critical path of any other admin task.

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
| **R2 — Connection / heartbeat loop** | WebSocket transport (`tokio-tungstenite`) speaking the SignalR JSON/MessagePack hub protocol re-derived from spec; reconnect with jittered backoff; heartbeat; graceful shutdown. | Agent stays connected to a CMRemote dev server for ≥ 24 h; reconnects across forced server restarts. |
| **R3 — Device information** | Cross-platform device-info collector behind `cmremote-platform::DeviceInfoProvider`. Windows uses `windows-rs`; Linux reads `/proc` + `/etc/os-release`; macOS uses `sysctl`. Reports back over the hub. | Server displays a Rust-agent device with parity fields vs. .NET agent. |
| **R4 — Process / script execution** | `argv`-only command execution (no shell). Per-OS shells: `pwsh`, `cmd`, `bash`, `zsh`. Output streamed back as chunked hub messages. **In-process PowerShell SDK is removed.** | All existing script tests pass against the Rust agent. |
| **R5 — Installed-applications provider** | Rust impls of the PR A `IInstalledApplicationsProvider` contract: Windows registry (`HKLM\…\Uninstall` + Wow6432Node) + AppX (via `Get-AppxPackage` shell-out for now). Linux/macOS: `NotSupported` stub matching the .NET behaviour. | Per-device snapshot identical to .NET-agent output for a reference Windows VM. |
| **R6 — Package manager (Chocolatey + MSI + Exe)** | Re-implement the PR B `IPackageProvider` contract and the PR C1/C2 MSI/Exe installers. Streaming structured progress; SHA-256 verification on download; signed short-lived URLs honoured. | All `ChocolateyOutputParserTests` / job-state tests pass against the Rust agent in integration mode. |
| **R7 — Desktop transport** | Last and largest. WebRTC capture/encode behind a thin trait so we can swap backends. Tracks Module 5's protocol doc. | Latency / FPS within 10 % of the .NET/Desktop client on a reference workload. |
| **R8 — Installer wrappers** | Windows MSI (WiX or `cargo-wix`), Linux `.deb` / `.rpm` (`cargo-deb` / `cargo-generate-rpm`), macOS notarized `.pkg`. Replaces PR E's templated PowerShell installer for the Rust channel. | One-liner deploy URL produces a working agent on each OS without PowerShell. |

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
