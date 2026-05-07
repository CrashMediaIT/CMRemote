# M3-S2 — `IOrganizationService`

## Scope

`IOrganizationService` owns the organization-directory surface previously
exposed by `IDataService`. This slice moves only organization lookup, the
default-organization toggle, the per-org Package Manager opt-in, and the
organization-name update. Branding, invitations, device groups, devices,
and shared files remain in later slices.

| Legacy `IDataService` method | New method |
|---|---|
| `GetDefaultOrganization()` | `IOrganizationService.GetDefaultOrganization()` |
| `GetOrganizationById(string)` | `IOrganizationService.GetOrganizationById(...)` |
| `GetOrganizationByUserName(string)` | `IOrganizationService.GetOrganizationByUserName(...)` |
| `GetOrganizationCountAsync()` | `IOrganizationService.GetOrganizationCountAsync()` |
| `GetOrganizationCount()` | `IOrganizationService.GetOrganizationCount()` |
| `GetOrganizationNameById(string)` | `IOrganizationService.GetOrganizationNameById(...)` |
| `GetOrganizationNameByUserName(string)` | `IOrganizationService.GetOrganizationNameByUserName(...)` |
| `SetIsDefaultOrganization(string, bool)` | `IOrganizationService.SetIsDefaultOrganization(...)` |
| `SetOrganizationPackageManagerEnabled(string, bool)` | `IOrganizationService.SetOrganizationPackageManagerEnabled(...)` |
| `UpdateOrganizationName(string, string)` | `IOrganizationService.UpdateOrganizationName(...)` |

## Dependencies

- `IAppDbFactory` for per-call `AppDb` creation.

No schema, transaction, DTO, authorization-policy, or wire-protocol changes
are part of this slice.

## Invariants

- All lookups are non-tracking reads.
- `GetDefaultOrganization` returns the single organization flagged
  `IsDefaultOrganization`; if none is flagged it fails with
  `Organization not found.`.
- `GetOrganizationByUserName` and `GetOrganizationNameByUserName` reject
  blank/whitespace user names with explicit `Result.Fail`.
- `GetOrganizationByUserName` performs a case-insensitive comparison on
  `UserName`; `GetOrganizationNameByUserName` uses the stored value as-is to
  preserve the existing behaviour.
- `SetIsDefaultOrganization` is idempotent for unknown ids (no throw); when
  setting `true`, every other organization is first cleared so that at most
  one organization is marked default.
- `SetOrganizationPackageManagerEnabled` is idempotent for unknown ids;
  when disabling the feature, all
  `DeviceInstalledApplicationsSnapshots` rows for the organization's devices
  are removed in the same `SaveChanges` call so the cached inventory is not
  visible after the feature is turned off.
- `UpdateOrganizationName` fails with `Organization not found.` for an
  unknown id and otherwise returns `Result.Ok` after persisting the new
  name verbatim (no trimming).

## Caller migration

The following callers are re-pointed from `IDataService` to
`IOrganizationService` for the methods listed above:

- `Server/API/BrandingController.cs`
- `Server/API/ClientDownloadsController.cs`
- `Server/API/CustomBinariesController.cs`
- `Server/API/HealthCheckController.cs`
- `Server/API/OrganizationManagementController.cs`
- `Server/API/RemoteControlController.cs`
- `Server/Auth/PackageManagerRequirementHandler.cs`
- `Server/Components/Devices/DeviceCard.razor.cs`
- `Server/Components/Layout/NavMenu.razor`
- `Server/Components/Pages/Branding.razor`
- `Server/Components/Pages/Downloads.razor`
- `Server/Components/Pages/ManageOrganization.razor.cs`
- `Server/Hubs/CircuitConnection.cs`
- `Tests/Server.Tests/DataServiceTests.cs`

`IDataService` no longer exposes either organization-count overload after
this slice; both move to `IOrganizationService`.

## Out of scope

- Branding (`GetBrandingInfo`, `UpdateBrandingInfo`, `ResetBranding`) —
  M3-S9.
- Device groups (`AddDeviceGroup`, `DeleteDeviceGroup`,
  `GetDeviceGroupsForOrganization`, etc.) — M3-S5.
- Invitations and `JoinViaInvitation` — M3-S8.
- Settings (`GetSettings`, `SaveSettings`) — M3-S9.
