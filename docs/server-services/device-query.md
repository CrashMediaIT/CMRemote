# M3-S3 — `IDeviceQueryService`

## Scope

`IDeviceQueryService` owns the device read-side surface previously exposed
by `IDataService`: per-device lookups, the org-wide device list, the
per-user device list, the two `DoesUserHaveAccessToDevice` overloads, the
two device/user permission filters, and the single-device-group lookup.

| Legacy `IDataService` method | New method |
|---|---|
| `GetDevice(string deviceId, Action<IQueryable<Device>>?)` | `IDeviceQueryService.GetDevice(deviceId, queryBuilder)` |
| `GetDevice(string orgId, string deviceId)` | `IDeviceQueryService.GetDevice(orgId, deviceId)` |
| `GetAllDevices(string orgId)` | `IDeviceQueryService.GetAllDevices(...)` |
| `GetDevicesForUser(string userName)` | `IDeviceQueryService.GetDevicesForUser(...)` |
| `DoesUserHaveAccessToDevice(string, RemotelyUser)` | `IDeviceQueryService.DoesUserHaveAccessToDevice(...)` |
| `DoesUserHaveAccessToDevice(string, string)` | `IDeviceQueryService.DoesUserHaveAccessToDevice(...)` |
| `FilterDeviceIdsByUserPermission(string[], RemotelyUser)` | `IDeviceQueryService.FilterDeviceIdsByUserPermission(...)` |
| `FilterUsersByDevicePermission(IEnumerable<string>, string)` | `IDeviceQueryService.FilterUsersByDevicePermission(...)` |
| `GetDeviceGroup(string, bool, bool)` | `IDeviceQueryService.GetDeviceGroup(...)` |

The remaining device-group surface (`GetDeviceGroups`,
`GetDeviceGroupsForOrganization`, the `AddDeviceGroup` /
`DeleteDeviceGroup` / user-membership mutations) is in the **M3-S5**
device-groups slice, not this one.

## Dependencies

- `IAppDbFactory` for per-call `AppDb` creation.

No schema, transaction, DTO, authorization-policy, or wire-protocol changes
are part of this slice.

## Invariants

- All lookups are non-tracking reads.
- `GetDevice(deviceId, queryBuilder)` applies the optional builder before
  the `FirstOrDefaultAsync`, preserving the existing `IQueryable<Device>`
  shaping hook used by `DeviceDetails`, `Terminal`, and the API
  controllers.
- `DoesUserHaveAccessToDevice` returns `true` when (a) the user is the
  device's organization administrator, or (b) the device's `DeviceGroup`
  contains the user. The behaviour for devices with no group is preserved:
  non-administrators get `false`.
- `DoesUserHaveAccessToDevice(string deviceId, string userId)` resolves the
  user once; missing users return `false` instead of throwing.
- `FilterDeviceIdsByUserPermission` preserves the same admin / group-member
  rule and never returns devices outside the caller's organization.
- `FilterUsersByDevicePermission` returns the org-scoped user-id slice that
  has access to a single device. For ungrouped devices, every org user is
  returned. Administrators always pass through.
- `GetDevicesForUser`:
  - Returns `Array.Empty<Device>()` when `userName` is blank or unknown.
  - Returns every device in the user's organization when the user is an
    administrator.
  - Otherwise returns the union of devices in groups the user belongs to.
- `GetDeviceGroup` includes `Devices` and/or `Users` only when the matching
  flag is set, and fails with `Device group not found.` for missing ids.

## Caller migration

The following callers are re-pointed from `IDataService` to
`IDeviceQueryService` for the methods listed above:

- `Server/API/AgentUpdateController.cs`
- `Server/API/DevicesController.cs`
- `Server/API/RemoteControlController.cs`
- `Server/API/ScriptResultsController.cs`
- `Server/API/ScriptingController.cs`
- `Server/Components/Devices/DeviceCard.razor.cs`
- `Server/Components/Devices/DevicesFrame.razor.cs`
- `Server/Components/Devices/Terminal.razor.cs`
- `Server/Components/Pages/DeviceDetails.razor.cs`
- `Server/Components/Pages/GetSupport.razor`
- `Server/Components/Pages/PackageManager/DeviceInstalledApps.razor.cs`
- `Server/Components/Pages/PackageManager/Devices.razor.cs`
- `Server/Components/Pages/PackageManager/InstallPackages.razor.cs`
- `Server/Components/Pages/PackageManager/UploadedMsis.razor.cs`
- `Server/Components/Pages/ServerConfig.razor` / `.razor.cs`
- `Server/Components/Scripts/RunScript.razor.cs`
- `Server/Components/Scripts/ScriptSchedules.razor.cs`
- `Server/Hubs/AgentHub.cs`
- `Server/Hubs/CircuitConnection.cs`
- `Tests/Server.Tests/CircuitConnectionTests.cs`
- `Tests/Server.Tests/DataServiceTests.cs`

`DataService` keeps a private `FilterUsersByDevicePermissionInternal`
helper for `AddAlert`'s in-process filtering until M3-S7
(alerts/notifications) lifts that callsite.

## Out of scope

- Device write-side mutations (`AddOrUpdateDevice`, `CreateDevice`,
  `UpdateDevice`, `UpdateTags`, `DeviceDisconnected`,
  `SetAllDevicesNotOnline`, `RemoveDevices`, `SetServerVerificationToken`)
  — M3-S4.
- Device-group mutations and org/user listings — M3-S5.
- Per-device installed-apps inventory queries — already lives in
  `IInstalledApplicationsService` (M2 work).
