// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Devices;

public interface IDeviceQueryService
{
    Task<Result<Device>> GetDevice(
        string deviceId,
        Action<IQueryable<Device>>? queryBuilder = null);

    Task<Result<Device>> GetDevice(string orgId, string deviceId);

    Device[] GetAllDevices(string orgId);

    Device[] GetDevicesForUser(string userName);

    bool DoesUserHaveAccessToDevice(string deviceId, RemotelyUser remotelyUser);

    bool DoesUserHaveAccessToDevice(string deviceId, string remotelyUserId);

    string[] FilterDeviceIdsByUserPermission(string[] deviceIds, RemotelyUser remotelyUser);

    string[] FilterUsersByDevicePermission(IEnumerable<string> userIds, string deviceId);

    Task<Result<DeviceGroup>> GetDeviceGroup(
        string deviceGroupId,
        bool includeDevices = false,
        bool includeUsers = false);
}
