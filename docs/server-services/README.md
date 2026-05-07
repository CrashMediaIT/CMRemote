# `Server/Services` — clean-room split (Module 3)

This directory holds the per-service contracts produced by the **Module 3**
clean-room slice of the [ROADMAP](../../ROADMAP.md). The goal of Module 3
is to retire the 2,300-LOC, ~85-method `IDataService` god-object in
`Server/Services/DataService.cs` and replace it with a set of focused
services, each authored from a written specification rather than copied from
the legacy file.

## Why a written spec per service?

The clean-room provenance gate (see ROADMAP "Definition of done for the
separation track" and the future `scripts/check-provenance` step) requires
that every production source file under `Server/` either carry a
`// Source: CMRemote, clean-room implementation` header or be a vendored
third-party file with its original notice. To make the provenance defensible
when the new services land, each new service is:

1. Specified here first (inputs, outputs, invariants, org-scope rules,
   audit-log emissions, error semantics).
2. Test-pinned in `Tests/Server.Tests/` against that spec, using the shared
   [`ServiceTestFixture`](../../Tests/Server.Tests/Infrastructure/ServiceTestFixture.cs).
3. Implemented from the spec — **not** by copy/paste from
   `DataService.cs` — and tagged with the provenance header.
4. Wired in the same PR by removing the corresponding methods from
   `IDataService` and re-pointing every caller. There is no facade /
   forwarder phase; parallel surfaces are explicitly forbidden so the
   surface cannot drift.

## Slice plan

| Slice | Spec doc | Service(s) | Methods migrated from `IDataService` |
|---|---|---|---|
| **M3-S0** | this file | — (scaffold + fixture only) | 0 |
| **M3-S1** | `user-directory.md` | `IUserDirectoryService` | ~12 (Create/Delete/Get user, options, admin flags, display name) |
| **M3-S2** | `organizations.md` | `IOrganizationService` | ~9 (org lookup, name, default, package-manager toggle) |
| **M3-S3** | `device-query.md` | `IDeviceQueryService` *(read-side)* | ~9 (`GetDevice` ×2, `GetAllDevices`, permission filters, `GetDeviceGroup`) |
| **M3-S4** | `device-command.md` | `IDeviceCommandService` *(write-side)* | ~6 (`AddOrUpdateDevice`, `CreateDevice`, `UpdateDevice`, `UpdateTags`, `DeviceDisconnected`, `SetAllDevicesNotOnline`) |
| **M3-S5** | `device-groups.md` | `IDeviceGroupService` | ~5 (group CRUD + user/device membership) |
| **M3-S6** | `scripts.md` | `IScriptCatalogService`, `IScriptResultService` | ~12 (saved scripts, results, runs, schedules) |
| **M3-S7** | `alerts.md` | `IAlertService` | 5 (Add/Delete/Get alerts) |
| **M3-S8** | `identity-tokens.md` | `IInviteService`, `IApiTokenService` | ~9 (invites + API tokens, incl. `ValidateApiKey`) |
| **M3-S9** | `branding-and-misc.md` | `IBrandingService`, `ISharedFileService`, `ISystemSettingsService` | remainder; **`IDataService` and `DataService` deleted** |

The roadmap (lines 1252–1255) calls out three services by name
(`IDeviceQueryService`, `IDeviceCommandService`, `IUserDirectoryService`);
the additional services in the table above are the natural seams discovered
when the 85-method surface was triaged. Splitting at these seams keeps each
PR in the 600–1200 LOC band and keeps every service's spec doc small enough
to be reviewable as a single document.

## What does **not** change in Module 3

- Database schema, EF migrations, connection strings, or transactions.
- Wire protocol or any DTO shapes (`Shared/Dtos/**` is untouched).
- Authorization policies (only their *call sites* move).
- The Rust agent, `Migration.Legacy`, `AgentUpgrade`, the Setup wizard, or
  Package Manager — they only touch `IDataService` as consumers and will be
  re-pointed mechanically in their owning slice.

This is a refactor for provenance + maintainability, not a feature
redesign. Behaviour-equivalence is the bar; each slice's tests must pin the
invariants the legacy method already enforced (org-scope, soft-delete,
audit-log emission, cross-org refusal, idempotency, etc.).

## Conventions

### Provenance header

Every new file under `Server/Services/<slice>/` and its corresponding test
file under `Tests/Server.Tests/<slice>/` must start with:

```csharp
// Source: CMRemote, clean-room implementation
```

This matches the convention applied to the rest of the clean-room track
(see `docs/threat-model.md`, `SECURITY.md`, `ROADMAP.md`, and existing
services such as `Server/Services/Setup/AdminBootstrapService.cs`).

### Database access

Every new service depends on `Remotely.Server.Data.IAppDbFactory` exactly as
`DataService` does today
([`Server/Data/AppDbFactory.cs`](../../Server/Data/AppDbFactory.cs)). The
factory hands out a fresh `AppDb` per call; the service owns the lifetime
with `using var db = _dbFactory.GetContext();`. Do **not** introduce a new
abstraction (e.g. `IDbContextFactory<AppDb>`) in this module — the
`IAppDbFactory` indirection is what `Server/Program.cs` registers and what
every other service in the codebase uses.

### Spec-doc template

Each `docs/server-services/<slice>.md` should contain:

1. **Scope** — which methods this service owns and which classes/services
   replace which legacy `IDataService` methods (one-to-one mapping table).
2. **Dependencies** — `IAppDbFactory`, `UserManager<RemotelyUser>`,
   `IAuditLogService`, etc.
3. **Invariants** — org-scope, null-input handling, soft-delete behaviour,
   audit emissions, idempotency.
4. **Public API** — interface signatures with one-paragraph
   description per method.
5. **Caller migration** — list every file that must be re-pointed, so the
   PR review can spot-check coverage.
6. **Out of scope** — explicit non-goals so the slice doesn't grow.

### Test fixture

Tests use the shared
[`ServiceTestFixture`](../../Tests/Server.Tests/Infrastructure/ServiceTestFixture.cs)
helper, which wraps the existing `IoCActivator` + `TestData` pattern so a
per-service test class only needs:

```csharp
[TestInitialize]
public async Task Init()
{
    _fixture = await ServiceTestFixture.CreateSeededAsync();
    _service = new MyService(_fixture.DbFactory, /* ... */);
}
```

`CreateEmptyAsync()` skips the `TestData.Init()` seed for tests that prefer
to seed their own rows. The fixture exposes `DbFactory`, `Services` (the
shared `IServiceProvider`), and `Data` (the seeded `TestData`, when
applicable). It deliberately reuses the assembly-wide service provider set
up by `IoCActivator` so the in-memory EF database remains shared and reset
between tests, matching the existing convention.
