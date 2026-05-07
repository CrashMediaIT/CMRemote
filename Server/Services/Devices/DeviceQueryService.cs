// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Server.Extensions;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Devices;

public class DeviceQueryService : IDeviceQueryService
{
    private readonly IAppDbFactory _dbFactory;

    public DeviceQueryService(IAppDbFactory dbFactory)
    {
        _dbFactory = dbFactory;
    }

    public async Task<Result<Device>> GetDevice(
        string deviceId,
        Action<IQueryable<Device>>? queryBuilder = null)
    {
        using var db = _dbFactory.GetContext();

        var device = await db.Devices
            .AsNoTracking()
            .Apply(queryBuilder)
            .FirstOrDefaultAsync(x => x.ID == deviceId);

        return device is null
            ? Result.Fail<Device>("Device not found.")
            : Result.Ok(device);
    }

    public async Task<Result<Device>> GetDevice(string orgId, string deviceId)
    {
        using var db = _dbFactory.GetContext();

        var device = await db.Devices
            .AsNoTracking()
            .FirstOrDefaultAsync(x =>
                x.OrganizationID == orgId &&
                x.ID == deviceId);

        return device is null
            ? Result.Fail<Device>("Device not found.")
            : Result.Ok(device);
    }

    public Device[] GetAllDevices(string orgId)
    {
        using var db = _dbFactory.GetContext();

        return db.Devices
            .AsNoTracking()
            .Where(x => x.OrganizationID == orgId)
            .ToArray();
    }

    public Device[] GetDevicesForUser(string userName)
    {
        if (string.IsNullOrWhiteSpace(userName))
        {
            return Array.Empty<Device>();
        }

        using var db = _dbFactory.GetContext();

        var user = db.Users
            .AsNoTracking()
            .FirstOrDefault(x => x.UserName == userName);

        if (user is null)
        {
            return Array.Empty<Device>();
        }

        if (user.IsAdministrator)
        {
            return db.Devices
                .AsNoTracking()
                .Where(x => x.OrganizationID == user.OrganizationID)
                .ToArray();
        }

        return db.Users
            .AsNoTracking()
            .Include(x => x.DeviceGroups)
            .ThenInclude(x => x.Devices)
            .Where(x => x.UserName == userName)
            .SelectMany(x => x.DeviceGroups)
            .SelectMany(x => x.Devices)
            .ToArray();
    }

    public bool DoesUserHaveAccessToDevice(string deviceId, RemotelyUser remotelyUser)
    {
        if (remotelyUser is null)
        {
            return false;
        }

        using var db = _dbFactory.GetContext();

        return db.Devices
            .Include(x => x.DeviceGroup)
            .ThenInclude(x => x!.Users)
            .Any(device =>
                device.OrganizationID == remotelyUser.OrganizationID &&
                device.ID == deviceId &&
                (
                    remotelyUser.IsAdministrator ||
                    device.DeviceGroup!.Users.Any(user => user.Id == remotelyUser.Id
                )));
    }

    public bool DoesUserHaveAccessToDevice(string deviceId, string remotelyUserId)
    {
        using var db = _dbFactory.GetContext();

        var remotelyUser = db.Users.Find(remotelyUserId);

        if (remotelyUser is null)
        {
            return false;
        }

        return DoesUserHaveAccessToDevice(deviceId, remotelyUser);
    }

    public string[] FilterDeviceIdsByUserPermission(string[] deviceIds, RemotelyUser remotelyUser)
    {
        using var db = _dbFactory.GetContext();

        return db.Devices
            .Include(x => x.DeviceGroup)
            .ThenInclude(x => x!.Users)
            .Where(device =>
                device.OrganizationID == remotelyUser.OrganizationID &&
                deviceIds.Contains(device.ID) &&
                (
                    remotelyUser.IsAdministrator ||
                    device.DeviceGroup!.Users.Any(user => user.Id == remotelyUser.Id
                )))
            .Select(x => x.ID)
            .ToArray();
    }

    public string[] FilterUsersByDevicePermission(IEnumerable<string> userIds, string deviceId)
    {
        using var db = _dbFactory.GetContext();

        var device = db.Devices
             .Include(x => x.DeviceGroup)
             .ThenInclude(x => x!.Users)
             .FirstOrDefault(x => x.ID == deviceId);

        if (device is null)
        {
            return Array.Empty<string>();
        }

        var orgUsers = db.Users
            .Where(user =>
                user.OrganizationID == device.OrganizationID &&
                userIds.Contains(user.Id));

        if (string.IsNullOrWhiteSpace(device.DeviceGroupID))
        {
            return orgUsers
                .Select(x => x.Id)
                .ToArray();
        }

        var allowedUsers = device.DeviceGroup?.Users?.Select(x => x.Id) ?? Array.Empty<string>();

        return orgUsers
            .Where(user =>
                user.IsAdministrator ||
                allowedUsers.Contains(user.Id)
            )
            .Select(x => x.Id)
            .ToArray();
    }

    public async Task<Result<DeviceGroup>> GetDeviceGroup(
        string deviceGroupId,
        bool includeDevices = false,
        bool includeUsers = false)
    {
        using var db = _dbFactory.GetContext();

        var query = db.DeviceGroups
            .AsNoTracking()
            .AsQueryable();

        if (includeDevices)
        {
            query = query.Include(x => x.Devices);
        }
        if (includeUsers)
        {
            query = query.Include(x => x.Users);
        }

        var group = await query.FirstOrDefaultAsync(x => x.ID == deviceGroupId);

        return group is null
            ? Result.Fail<DeviceGroup>("Device group not found.")
            : Result.Ok(group);
    }
}
