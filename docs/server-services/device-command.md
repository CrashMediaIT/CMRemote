# M3-S4 — `IDeviceCommandService`

## Scope

`IDeviceCommandService` owns the device write-side surface previously
exposed by `IDataService`: agent register/refresh, admin-driven creation
and editing, tag updates, and connection-state mutations.

| Legacy `IDataService` method | New method |
|---|---|
| `AddOrUpdateDevice(DeviceClientDto)` | `IDeviceCommandService.AddOrUpdateDevice(DeviceClientDto)` |
| `CreateDevice(DeviceSetupOptions)` | `IDeviceCommandService.CreateDevice(DeviceSetupOptions)` |
| `UpdateDevice(string, string?, string?, string?, string?)` | `IDeviceCommandService.UpdateDevice(deviceId, tag, alias, deviceGroupId, notes)` |
| `UpdateDevice(DeviceSetupOptions, string)` | `IDeviceCommandService.UpdateDevice(DeviceSetupOptions, organizationId)` |
| `UpdateTags(string, string)` | `IDeviceCommandService.UpdateTags(deviceId, tags)` |
| `DeviceDisconnected(string)` | `IDeviceCommandService.DeviceDisconnected(deviceId)` |
| `SetAllDevicesNotOnline()` | `IDeviceCommandService.SetAllDevicesNotOnline()` |

`SetServerVerificationToken`, `RemoveDevices`, and `WriteEvent` for device
events stay on `IDataService` for now and move with later slices.

## Dependencies

- `IAppDbFactory` for per-call `AppDb` creation.
- `IHostEnvironment` to preserve the development-mode org-attach behaviour
  in `AddOrUpdateDevice`.
- `ILogger<DeviceCommandService>` for the existing diagnostic messages on
  the unknown-organization and create-device failure paths.

No schema, transaction, DTO, authorization-policy, or wire-protocol changes
are part of this slice.

## Invariants

- `AddOrUpdateDevice` continues to upsert by `Device.ID`, refresh every
  inventory column written today (CPU, memory, disks, OS metadata,
  agent version, public IP, MAC list, online flag, `LastOnline`), validate
  the target organization (failure: `"Organization does not exist."`), and
  in `Development` reuse the first available organization.
- `CreateDevice` rejects null/blank ids and existing ids with
  `"Required parameters are missing or incorrect."`, attaches the named
  device group when present, and logs + returns
  `"An error occurred while creating the device."` on exceptions.
- The `(deviceId, tag, alias, deviceGroupId, notes)` overload of
  `UpdateDevice` keeps no-op behaviour for missing devices, clears the
  device-group association when `deviceGroupId` is blank (also detaching
  the device from the previously-tracked group's collection), and
  otherwise updates the listed fields verbatim.
- The `(DeviceSetupOptions, organizationId)` overload returns
  `"Device not found."` when the device does not belong to the supplied
  organization, otherwise updates `Alias` and the optional device-group
  by name.
- `UpdateTags` is a no-op for missing devices; otherwise it overwrites
  `Tags` with the supplied string.
- `DeviceDisconnected` is a no-op for missing devices; otherwise it sets
  `IsOnline=false` and `LastOnline=DateTimeOffset.Now`.
- `SetAllDevicesNotOnline` flips `IsOnline=false` for every row in the
  table — used by startup recovery.

## Caller migration

The following callers are re-pointed from `IDataService` to
`IDeviceCommandService` for the methods listed above:

- `Server/API/AgentUpdateController.cs`
- `Server/API/DevicesController.cs`
- `Server/Components/Pages/EditDevice.razor.cs`
- `Server/Hubs/AgentHub.cs`
- `Server/Hubs/CircuitConnection.cs`
- `Server/Program.cs` (the startup `SetAllDevicesNotOnline` hook)
- `Tests/Server.Tests/AgentHubTests.cs`
- `Tests/Server.Tests/CircuitConnectionTests.cs`
- `Tests/Server.Tests/DataServiceTests.cs`
- `Tests/Server.Tests/TestData.cs` (seed pipeline)

`DataService.AddOrUpdateDevice` is removed; a single
`AddOrUpdateDeviceForTests` shim is **not** introduced — `TestData` calls
`IDeviceCommandService` directly.

## Out of scope

- Removal of devices (`RemoveDevices`) — stays on `IDataService` until the
  M3 alerts slice (M3-S7) can lift the cascade-delete coupling with
  `Alerts`/`InstalledApplicationSnapshot`.
- `SetServerVerificationToken` — stays on `IDataService` until the agent
  registration slice (M3-S6).
- Device-group queries/mutations and per-user listings — handled in S3
  and the upcoming S5.
