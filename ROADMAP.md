# CMRemote Roadmap

This roadmap tracks the work required to (1) finish the Package Manager feature
set on top of the existing codebase, (2) harden the result, and (3) replace the
remaining upstream-derived code so that **CMRemote** stands on its own as an
independent, clean-room re-architecture rather than a downstream of the original
project. The clean-room work is intentionally scheduled **last** so functional
features ship to users first.

> **Status legend**
> ✅ shipped  &nbsp;·&nbsp;  🟡 in progress  &nbsp;·&nbsp;  🔜 planned

---

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

## 🔜 PR D — Hardening pass *before* the agent rewrite

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

## 🟡 Clean-room redesign / separation track *(parallel, low-tempo)*

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
| 0 | **Wire Protocol Specification** | Write a versioned spec (`docs/wire-protocol.md`) that pins SignalR hub method names, MessagePack/JSON DTO shapes, framing, auth/handshake, and reconnection semantics. Generate a *test-vector corpus* (golden hex dumps + JSON fixtures) consumed by both the C# and Rust implementations. | **Earliest of all** — unblocks both the Shared rewrite and the Rust agent. |
| 1 | `Shared` (DTOs, enums, helpers) | Re-derive types from the wire spec from #0; rewrite serializer-friendly DTOs; replace ad-hoc `Result<T>` with a single tested implementation. | After #0. |
| 2a | **Agent contract freeze** | Lock the agent ↔ server method surface (`IAgentHubClient`, `AgentHub` callbacks) into the spec from #0 with a backwards-compat negotiation field. Add server-side conformance tests that replay the test-vector corpus. | After #0/#1. |
| 2b | **`agent-rs/` — Rust agent re-implementation** | New Cargo workspace under `agent-rs/`. Crates: `cmremote-wire` (DTOs + serde/rmp-serde), `cmremote-platform` (per-OS trait impls), `cmremote-agent` (binary). Implement in slices: connection/heartbeat → device info → script execution → installed-applications → package manager → desktop transport (last). Ship behind a feature flag and an opt-in `agent-channel` per device until parity. | After #2a; runs in parallel with #3–#6 once the protocol is frozen. |
| 2c | Legacy .NET agent (`Agent/`) | Maintenance-only while #2b ramps. Once the Rust agent reaches Windows parity, deprecate for one release, then remove. | Parallel with #2b. |
| 3 | `Server.Services` (data, auth, circuit, scripts) | Split monolithic `DataService` into focused services (`IDeviceQueryService`, `IDeviceCommandService`, `IUserDirectoryService`); rewrite each from spec. | After #1. |
| 4 | `Server.Hubs` (`AgentHub`, `ViewerHub`, `CircuitConnection`) | Rewrite the dispatch layer using a generated client interface; remove duplicate authorization checks; centralize org-scope assertions. | After #3. |
| 5 | `Desktop` / remote-control transport | Rewrite WebRTC / IceServer plumbing against a written protocol doc; consider switching to `Microsoft.MixedReality.WebRTC` or `SIPSorcery` to eliminate inherited code. The Rust agent's desktop transport (#2b last slice) tracks the same protocol doc. | Parallelizable with #4. |
| 6 | `Server` Razor UI | Rebuild the layout shell (`MainLayout`, `NavMenu`) from scratch with a CMRemote design system; per-page Razor logic is rewritten module-by-module. The Package Manager pages added in PR B are already CMRemote-original and stay. | Last — depends on stable services. |
| 7 | Installer / agent deployment | Covered by PR E above; the Rust agent simplifies this dramatically (single static binary → MSI / `.deb` / `.rpm` / `.pkg` wrappers). | After #2b reaches Windows parity. |

### Rust agent (`agent-rs/`) — slice-by-slice delivery plan

Implementation order for Module 2b. Each slice ships behind a per-device
`agent-channel` opt-in (`stable-dotnet` | `preview-rust`) so the legacy .NET
agent and the Rust agent can run side-by-side until parity.

| Slice | Scope | Exit criteria |
|---|---|---|
| **R0 — Workspace scaffold** *(this PR)* | `agent-rs/Cargo.toml` workspace; crates `cmremote-wire`, `cmremote-platform`, `cmremote-agent`; structured logging (`tracing`); config loader for `ConnectionInfo.json` + CLI args; signal handling; CI (`cargo fmt`, `cargo clippy -D warnings`, `cargo test`). No network I/O yet. | Workspace builds clean on stable Rust. CI green. Provenance header on every file. |
| **R1 — Wire types + test vectors** | `cmremote-wire`: `ConnectionInfo`, `DeviceClientDto`, `HubMessage` envelope, MessagePack + JSON round-trip. Consume the test-vector corpus from Module 0. | All vectors round-trip byte-for-byte. |
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
