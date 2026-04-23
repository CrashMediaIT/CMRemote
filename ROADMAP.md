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

## 🔜 PR C1 — Silent MSI upload + install

- **`UploadedMsi`** entity (org-scoped). Upload via Razor form → stored under
  `SharedFiles` with **SHA-256** + magic-byte validation
  (`D0 CF 11 E0 A1 B1 1A E1` MSI / OLE2 header), max-size cap, antivirus-friendly
  streaming.
- **Agent: `MsiPackageInstaller`** — downloads via signed short-lived URL,
  verifies SHA-256, runs:
  ```
  msiexec /i <file> /qn /norestart /L*v <log>
  ```
  and uploads the verbose log on failure.
- WebUI under **Uploaded MSIs**: list, upload, delete, *Send to device* button
  (Windows-only, online devices).
- Deletes are **tombstoned** — only purged after no in-flight jobs reference them.

## 🔜 PR C2 — Executable Package Builder + Deployment Bundles

- **`ExecutablePackage`** entity: `Name`, `DownloadUrl` (or uploaded blob),
  `SilentArgs`, `SuccessExitCodes` (default `0,3010,1641`), optional `SHA-256`.
- **`DeploymentBundle`** is extended to accept ordered items of any of three
  kinds: Chocolatey id / `UploadedMsi` ref / `ExecutablePackage` ref, plus a
  `StopOnFirstFailure` flag.
- **Run bundle** issues a single `BundleRunJob` to the agent, which executes
  items sequentially and returns per-item structured results.
- WebUI: drag-and-drop ordering, per-item status badges, *retry-failed-only*.

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
| 1 | `Shared` (DTOs, enums, helpers) | Re-derive types from the SignalR wire spec; rewrite serializer-friendly DTOs; replace ad-hoc `Result<T>` with a single tested implementation. | Earliest — unblocks everything else. |
| 2 | `Agent.Services` (uninstaller, updater, app launcher) | Rewrite per-OS providers behind narrow interfaces. Keep the existing `IPackageProvider` / `IInstalledApplicationsProvider` contracts (added in PR A/B) — those are already CMRemote-original. | After Shared. |
| 3 | `Server.Services` (data, auth, circuit, scripts) | Split monolithic `DataService` into focused services (`IDeviceQueryService`, `IDeviceCommandService`, `IUserDirectoryService`); rewrite each from spec. | After Agent. |
| 4 | `Server.Hubs` (`AgentHub`, `ViewerHub`, `CircuitConnection`) | Rewrite the dispatch layer using a generated client interface; remove duplicate authorization checks; centralize org-scope assertions. | After Server.Services. |
| 5 | `Desktop` / remote-control transport | Rewrite WebRTC / IceServer plumbing against a written protocol doc; consider switching to `Microsoft.MixedReality.WebRTC` or `SIPSorcery` to eliminate inherited code. | Parallelizable with #4. |
| 6 | `Server` Razor UI | Rebuild the layout shell (`MainLayout`, `NavMenu`) from scratch with a CMRemote design system; per-page Razor logic is rewritten module-by-module. The Package Manager pages added in PR B are already CMRemote-original and stay. | Last — depends on stable services. |
| 7 | Installer / agent deployment | Covered by PR E above. | After #2. |

### Definition of done for the separation track

- [ ] Every file under `Agent/`, `Server/`, `Shared/`, and `Desktop*/` either has
      a `// Source: CMRemote, clean-room implementation` provenance header or
      is a vendored third-party file with its original notice.
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
