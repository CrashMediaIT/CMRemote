// Source: CMRemote, clean-room implementation

using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Devices;

public interface IDeviceCommandService
{
    Task<Result<Device>> AddOrUpdateDevice(DeviceClientDto deviceDto);

    Task<Result<Device>> CreateDevice(DeviceSetupOptions options);

    Task UpdateDevice(string deviceId, string? tag, string? alias, string? deviceGroupId, string? notes);

    Task<Result<Device>> UpdateDevice(DeviceSetupOptions deviceOptions, string organizationId);

    Task UpdateTags(string deviceId, string tags);

    void DeviceDisconnected(string deviceId);

    Task SetAllDevicesNotOnline();
}
