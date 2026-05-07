// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Devices;

public class DeviceCommandService : IDeviceCommandService
{
    private readonly IAppDbFactory _dbFactory;
    private readonly IWebHostEnvironment _hostEnvironment;
    private readonly ILogger<DeviceCommandService> _logger;

    public DeviceCommandService(
        IAppDbFactory dbFactory,
        IWebHostEnvironment hostEnvironment,
        ILogger<DeviceCommandService> logger)
    {
        _dbFactory = dbFactory;
        _hostEnvironment = hostEnvironment;
        _logger = logger;
    }

    public async Task<Result<Device>> AddOrUpdateDevice(DeviceClientDto deviceDto)
    {
        using var dbContext = _dbFactory.GetContext();

        var device = await dbContext.Devices.FindAsync(deviceDto.ID);

        if (device is null)
        {
            device = new Device
            {
                OrganizationID = deviceDto.OrganizationID,
                ID = deviceDto.ID,
            };
            await dbContext.Devices.AddAsync(device);
        }

        device.CurrentUser = deviceDto.CurrentUser;
        device.DeviceName = deviceDto.DeviceName;
        device.Drives = deviceDto.Drives;
        device.CpuUtilization = deviceDto.CpuUtilization;
        device.UsedMemory = deviceDto.UsedMemory;
        device.UsedStorage = deviceDto.UsedStorage;
        device.Is64Bit = deviceDto.Is64Bit;
        device.IsOnline = true;
        device.OSArchitecture = deviceDto.OSArchitecture;
        device.OSDescription = deviceDto.OSDescription;
        device.Platform = deviceDto.Platform;
        device.ProcessorCount = deviceDto.ProcessorCount;
        device.PublicIP = deviceDto.PublicIP;
        device.TotalMemory = deviceDto.TotalMemory;
        device.TotalStorage = deviceDto.TotalStorage;
        device.AgentVersion = deviceDto.AgentVersion;
        device.MacAddresses = deviceDto.MacAddresses ?? Array.Empty<string>();
        device.LastOnline = DateTimeOffset.Now;

        if (_hostEnvironment.IsDevelopment() && dbContext.Organizations.Any())
        {
            var org = await dbContext.Organizations.FirstAsync();
            device.Organization = org;
            device.OrganizationID = org.ID;
        }

        if (!await dbContext.Organizations.AnyAsync(x => x.ID == device.OrganizationID))
        {
            _logger.LogInformation(
                "Unable to add device {deviceName} because organization {organizationID}" +
                "does not exist.  Device ID: {ID}.",
                device.DeviceName,
                device.OrganizationID,
                device.ID);

            return Result.Fail<Device>("Organization does not exist.");
        }

        await dbContext.SaveChangesAsync();
        return Result.Ok(device);
    }

    public async Task<Result<Device>> CreateDevice(DeviceSetupOptions options)
    {
        using var dbContext = _dbFactory.GetContext();

        try
        {
            if (options is null ||
                string.IsNullOrWhiteSpace(options.DeviceID) ||
                string.IsNullOrWhiteSpace(options.OrganizationID) ||
                dbContext.Devices.Any(x => x.ID == options.DeviceID))
            {
                return Result.Fail<Device>("Required parameters are missing or incorrect.");
            }

            var device = new Device()
            {
                ID = options.DeviceID,
                OrganizationID = options.OrganizationID
            };

            if (!string.IsNullOrWhiteSpace(options.DeviceAlias))
            {
                device.Alias = options.DeviceAlias;
            }

            if (!string.IsNullOrWhiteSpace(options.DeviceGroupName))
            {
                var group = dbContext.DeviceGroups.FirstOrDefault(x =>
                    x.Name.ToLower() == options.DeviceGroupName.ToLower() &&
                    x.OrganizationID == device.OrganizationID);
                device.DeviceGroup = group;
            }

            dbContext.Devices.Add(device);

            await dbContext.SaveChangesAsync();

            return Result.Ok(device);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while creating device for organization {id}.", options?.OrganizationID);
            return Result.Fail<Device>("An error occurred while creating the device.");
        }
    }

    public async Task UpdateDevice(string deviceId, string? tag, string? alias, string? deviceGroupId, string? notes)
    {
        using var dbContext = _dbFactory.GetContext();

        var device = await dbContext.Devices
            .Include(x => x.DeviceGroup)
            .FirstOrDefaultAsync(x => x.ID == deviceId);

        if (device is null)
        {
            return;
        }

        if (string.IsNullOrWhiteSpace(deviceGroupId))
        {
            device.DeviceGroup?.Devices?.RemoveAll(x => x.ID == deviceId);
            device.DeviceGroup = null;
            device.DeviceGroupID = null;
        }
        else
        {
            device.DeviceGroupID = deviceGroupId;
        }

        device.Tags = tag;
        device.Alias = alias;
        device.Notes = notes;
        await dbContext.SaveChangesAsync();
    }

    public async Task<Result<Device>> UpdateDevice(DeviceSetupOptions deviceOptions, string organizationId)
    {
        using var dbContext = _dbFactory.GetContext();

        var device = await dbContext.Devices.FindAsync(deviceOptions.DeviceID);
        if (device == null || device.OrganizationID != organizationId)
        {
            return Result.Fail<Device>("Device not found.");
        }

        var group = await dbContext.DeviceGroups.FirstOrDefaultAsync(x =>
          x.Name.ToLower() == $"{deviceOptions.DeviceGroupName}".ToLower() &&
          x.OrganizationID == device.OrganizationID);
        device.DeviceGroup = group;

        device.Alias = deviceOptions.DeviceAlias;
        await dbContext.SaveChangesAsync();
        return Result.Ok(device);
    }

    public async Task UpdateTags(string deviceId, string tags)
    {
        using var dbContext = _dbFactory.GetContext();

        var device = await dbContext.Devices.FindAsync(deviceId);
        if (device == null)
        {
            return;
        }

        device.Tags = tags;
        await dbContext.SaveChangesAsync();
    }

    public void DeviceDisconnected(string deviceId)
    {
        using var dbContext = _dbFactory.GetContext();

        var device = dbContext.Devices.Find(deviceId);
        if (device != null)
        {
            device.LastOnline = DateTimeOffset.Now;
            device.IsOnline = false;
            dbContext.SaveChanges();
        }
    }

    public async Task SetAllDevicesNotOnline()
    {
        using var dbContext = _dbFactory.GetContext();

        await dbContext.Devices.ForEachAsync(x =>
        {
            x.IsOnline = false;
        });
        await dbContext.SaveChangesAsync();
    }
}
