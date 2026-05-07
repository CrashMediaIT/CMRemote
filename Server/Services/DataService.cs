using Remotely.Shared.Models;
using Microsoft.AspNetCore.Components.Forms;
using Microsoft.AspNetCore.Identity;
using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Server.Extensions;
using Remotely.Server.Models;
using Remotely.Shared;
using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Utilities;
using Remotely.Shared.ViewModels;
using System.Text.Json;

namespace Remotely.Server.Services;

// TODO: Separate this into domain-specific services.
public interface IDataService
{
    Task AddAlert(string deviceId, string organizationId, string alertMessage, string? details = null);

    Task<Result<DeviceGroup>> AddDeviceGroup(string orgId, DeviceGroup deviceGroup);
    Task<Result> AddDeviceToGroup(string deviceId, string groupId);
    Task<Result<InviteLink>> AddInvite(string orgId, InviteViewModel invite);

    Task<Result> AddOrUpdateSavedScript(SavedScript script, string userId);

    Task AddOrUpdateScriptSchedule(ScriptSchedule schedule);

    Task<Result<ScriptResult>> AddScriptResult(ScriptResultDto dto);
    Task<Result> AddScriptResultToScriptRun(string scriptResultId, int scriptRunId);

    Task AddScriptRun(ScriptRun scriptRun);

    Task<string> AddSharedFile(IBrowserFile file, string organizationId, Action<double, string> progressCallback);

    Task<string> AddSharedFile(IFormFile file, string organizationId);

    bool AddUserToDeviceGroup(string orgId, string groupId, string userName, out string resultMessage);

    Task CleanupOldRecords();

    Task<Result<ApiToken>> CreateApiToken(string userName, string tokenName, string secretHash);

    Task DeleteAlert(Alert alert);

    Task DeleteAllAlerts(string orgId, string? userName = null);

    Task<Result> DeleteApiToken(string userName, string tokenId);

    Task<Result> DeleteDeviceGroup(string orgId, string deviceGroupId);

    Task<Result> DeleteInvite(string orgId, string inviteId);

    Task DeleteSavedScript(Guid scriptId);

    Task DeleteScriptSchedule(int scriptScheduleId);

    Task<Result<Alert>> GetAlert(string alertId);

    Alert[] GetAlerts(string userId);

    ApiToken[] GetAllApiTokens(string userId);

    ScriptResult[] GetAllCommandResults(string orgId);

    ScriptResult[] GetAllCommandResultsForUser(string orgId, string userName, string deviceId);

    InviteLink[] GetAllInviteLinks(string organizationId);

    ScriptResult[] GetAllScriptResults(string orgId, string deviceId);

    ScriptResult[] GetAllScriptResultsForUser(string orgId, string userName);

    Task<Result<ApiToken>> GetApiKey(string keyId);

    Task<Result<BrandingInfo>> GetBrandingInfo(string organizationId);

    int GetDeviceCount();

    int GetDeviceCount(RemotelyUser user);

    DeviceGroup[] GetDeviceGroups(string username);

    DeviceGroup[] GetDeviceGroupsForOrganization(string organizationId);

    List<Device> GetDevices(IEnumerable<string> deviceIds);

    Task<IEnumerable<ScriptRun>> GetPendingScriptRuns(string deviceId);

    Task<List<SavedScript>> GetQuickScripts(string userId);

    Task<Result<SavedScript>> GetSavedScript(Guid scriptId);

    Task<Result<SavedScript>> GetSavedScript(string userId, Guid scriptId);

    Task<List<SavedScript>> GetSavedScriptsWithoutContent(string userId, string organizationId);

    Task<Result<ScriptResult>> GetScriptResult(string resultId);

    Task<Result<ScriptResult>> GetScriptResult(string resultId, string orgId);

    Task<List<ScriptSchedule>> GetScriptSchedules(string organizationId);

    Task<List<ScriptSchedule>> GetScriptSchedulesDue();

    List<string> GetServerAdmins();

    Task<SettingsModel> GetSettings();

    Task<Result<SharedFile>> GetSharedFiled(string fileId);

    int GetTotalDevices();

    Task<Result> JoinViaInvitation(string userName, string inviteId);

    void RemoveDevices(string[] deviceIds);

    Task<bool> RemoveUserFromDeviceGroup(string orgId, string groupId, string userId);

    Task<Result> RenameApiToken(string userName, string tokenId, string tokenName);

    Task ResetBranding(string organizationId);

    Task SaveSettings(SettingsModel settings);

    void SetServerVerificationToken(string deviceId, string verificationToken);

    Task<bool> TempPasswordSignIn(string email, string password);

    Task UpdateBrandingInfo(
        string organizationId,
        string productName,
        byte[] iconBytes);

    Task<bool> ValidateApiKey(string keyId, string apiSecret, string requestPath, string remoteIP);
}

public class DataService : IDataService
{
    private readonly IAppDbFactory _appDbFactory;
    private readonly IHostEnvironment _hostEnvironment;
    private readonly ILogger<DataService> _logger;
    private readonly SemaphoreSlim _settingsLock = new(1, 1);

    public DataService(
        IHostEnvironment hostEnvironment,
        IAppDbFactory appDbFactory,
        ILogger<DataService> logger)
    {
        _hostEnvironment = hostEnvironment;
        _appDbFactory = appDbFactory;
        _logger = logger;
    }

    public async Task AddAlert(string deviceId, string organizationId, string alertMessage, string? details = null)
    {
        using var dbContext = _appDbFactory.GetContext();

        var users = dbContext.Users
           .Include(x => x.Alerts)
           .Where(x => x.OrganizationID == organizationId);

        if (!string.IsNullOrWhiteSpace(deviceId))
        {
            var filteredUserIDs = FilterUsersByDevicePermissionInternal(
                dbContext,
                users.Select(x => x.Id),
                deviceId);

            users = users.Where(x => filteredUserIDs.Contains(x.Id));
        }

        await users.ForEachAsync(x =>
        {
            var alert = new Alert()
            {
                CreatedOn = DateTimeOffset.Now,
                DeviceID = deviceId,
                Message = alertMessage,
                OrganizationID = organizationId,
                Details = details
            };
            x.Alerts ??= new List<Alert>();
            x.Alerts.Add(alert);
        });

        await dbContext.SaveChangesAsync();
    }

    public async Task<Result<DeviceGroup>> AddDeviceGroup(string orgId, DeviceGroup deviceGroup)
    {
        using var dbContext = _appDbFactory.GetContext();

        var organization = dbContext.Organizations
            .Include(x => x.DeviceGroups)
            .FirstOrDefault(x => x.ID == orgId);

        if (organization is null)
        {
            return Result.Fail<DeviceGroup>("Organization not found.");
        }

        if (dbContext.DeviceGroups.Any(x =>
            x.OrganizationID == orgId &&
            x.Name.ToLower() == deviceGroup.Name.ToLower()))
        {
            return Result.Fail<DeviceGroup>("Device group already exists.");
        }

        dbContext.Attach(deviceGroup);
        deviceGroup.Organization = organization;
        deviceGroup.OrganizationID = orgId;

        organization.DeviceGroups ??= new List<DeviceGroup>();
        organization.DeviceGroups.Add(deviceGroup);
        await dbContext.SaveChangesAsync();
        return Result.Ok(deviceGroup);
    }

    public async Task<Result> AddDeviceToGroup(string deviceId, string groupId)
    {
        using var context = _appDbFactory.GetContext();
        var device = await context.Devices.FirstOrDefaultAsync(x => x.ID == deviceId);

        if (device is null)
        {
            return Result.Fail("Device not found.");
        }

        var group = await context.DeviceGroups.FirstOrDefaultAsync(x => 
            x.OrganizationID == device.OrganizationID &&
            x.ID == groupId);

        if (group is null)
        {
            return Result.Fail("Group not found.");
        }

        group.Devices ??= new();
        group.Devices.Add(device);
        device.DeviceGroup = group;
        device.DeviceGroupID = group.ID;
        await context.SaveChangesAsync();

        return Result.Ok();
    }

    public async Task<Result<InviteLink>> AddInvite(string orgId, InviteViewModel invite)
    {
        using var dbContext = _appDbFactory.GetContext();

        var organization = dbContext.Organizations
            .Include(x => x.InviteLinks)
            .FirstOrDefault(x => x.ID == orgId);

        if (organization is null)
        {
            return Result.Fail<InviteLink>("Organization not found.");
        }

        var inviteLink = new InviteLink()
        {
            InvitedUser = invite.InvitedUser?.ToLower(),
            DateSent = DateTimeOffset.Now,
            IsAdmin = invite.IsAdmin,
            Organization = organization,
            OrganizationID = organization.ID,
        };

        organization.InviteLinks ??= new List<InviteLink>();
        organization.InviteLinks.Add(inviteLink);
        await dbContext.SaveChangesAsync();
        return Result.Ok(inviteLink);
    }

    public async Task<Result> AddOrUpdateSavedScript(SavedScript script, string userId)
    {
        using var dbContext = _appDbFactory.GetContext();

        dbContext.SavedScripts.Update(script);

        if (script.Creator is null)
        {
            var user = await dbContext.Users.FindAsync(userId);
            if (user is null)
            {
                return Result.Fail("User not found.");
            }

            script.CreatorId = user.Id;
            script.Creator = user;
            script.OrganizationID = user.OrganizationID;
        }

        await dbContext.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task AddOrUpdateScriptSchedule(ScriptSchedule schedule)
    {
        using var dbContext = _appDbFactory.GetContext();

        var existingSchedule = await dbContext.ScriptSchedules
            .Include(x => x.Creator)
            .Include(x => x.Devices)
            .Include(x => x.DeviceGroups)
            .FirstOrDefaultAsync(x => x.Id == schedule.Id);

        if (existingSchedule is null)
        {
            dbContext.Update(schedule);
        }
        else
        {
            var entry = dbContext.Entry(existingSchedule);
            entry.CurrentValues.SetValues(schedule);

            existingSchedule.Devices.Clear();
            if (schedule.Devices?.Any() == true)
            {
                var deviceIds = schedule.Devices.Select(x => x.ID);
                var newDevices = await dbContext.Devices
                    .Where(x => deviceIds.Contains(x.ID))
                    .ToListAsync();
                existingSchedule.Devices.AddRange(newDevices);
            }

            existingSchedule.DeviceGroups.Clear();
            if (schedule.DeviceGroups?.Any() == true)
            {
                var deviceGroupIds = schedule.DeviceGroups.Select(x => x.ID);
                var newDeviceGroups = await dbContext.DeviceGroups
                    .Where(x => deviceGroupIds.Contains(x.ID))
                    .ToListAsync();
                existingSchedule.DeviceGroups.AddRange(newDeviceGroups);
            }
        }

        await dbContext.SaveChangesAsync();
    }

    public async Task<Result<ScriptResult>> AddScriptResult(ScriptResultDto dto)
    {
        using var dbContext = _appDbFactory.GetContext();

        var device = dbContext.Devices.Find(dto.DeviceID);

        if (device is null)
        {
            return Result.Fail<ScriptResult>("Device not found.");
        }

        var scriptResult = new ScriptResult
        {
            DeviceID = dto.DeviceID,
            OrganizationID = device.OrganizationID
        };

        var entry = dbContext.Attach(scriptResult);
        entry.CurrentValues.SetValues(dto);
        entry.State = EntityState.Added;
        await dbContext.ScriptResults.AddAsync(scriptResult);
        await dbContext.SaveChangesAsync();
        return Result.Ok(scriptResult);
    }

    public async Task<Result> AddScriptResultToScriptRun(string scriptResultId, int scriptRunId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var run = await dbContext.ScriptRuns
            .Include(x => x.Results)
            .FirstOrDefaultAsync(x => x.Id == scriptRunId);

        if (run is null)
        {
            return Result.Fail("Run not found.");
        }

        var result = await dbContext.ScriptResults.FindAsync(scriptResultId);

        if (result is null)
        {
            return Result.Fail("Results not found.");
        }

        run.Results ??= new List<ScriptResult>();
        run.Results.Add(result);

        await dbContext.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task AddScriptRun(ScriptRun scriptRun)
    {
        using var dbContext = _appDbFactory.GetContext();

        dbContext.Attach(scriptRun);
        dbContext.ScriptRuns.Add(scriptRun);
        await dbContext.SaveChangesAsync();
    }

    public async Task<string> AddSharedFile(IBrowserFile file, string organizationId, Action<double, string> progressCallback)
    {
        var fileSize = file.Size;
        var fileName = file.Name;

        var fileContents = new byte[fileSize];
        var stream = file.OpenReadStream(AppConstants.MaxUploadFileSize);

        var bytesRead = 0;
        while (bytesRead < fileSize)
        {
            var segmentEnd = Math.Min(50_000, fileSize - bytesRead);
            var read = await stream.ReadAsync(fileContents.AsMemory(bytesRead, (int)segmentEnd));
            if (read == 0)
            {
                break;
            }
            bytesRead += read;
            progressCallback.Invoke((double)bytesRead / fileSize, fileName);
        }

        progressCallback.Invoke(1, fileName);

        return await AddSharedFileImpl(file.Name, fileContents, file.ContentType, organizationId);
    }

    public async Task<string> AddSharedFile(IFormFile file, string organizationId)
    {
        var fileContents = new byte[file.Length];
        using var stream = file.OpenReadStream();
        await stream.ReadAsync(fileContents.AsMemory(0, (int)file.Length));

        return await AddSharedFileImpl(file.Name, fileContents, file.ContentType, organizationId);
    }

    public bool AddUserToDeviceGroup(string orgId, string groupId, string userName, out string resultMessage)
    {
        using var dbContext = _appDbFactory.GetContext();

        resultMessage = string.Empty;

        var deviceGroup = dbContext.DeviceGroups
            .Include(x => x.Users)
            .FirstOrDefault(x =>
                x.ID == groupId &&
                x.OrganizationID == orgId);

        if (deviceGroup == null)
        {
            resultMessage = "Device group not found.";
            return false;
        }

        userName = userName.Trim().ToLower();

        var user = dbContext.Users
            .Include(x => x.DeviceGroups)
            .FirstOrDefault(x =>
                x.UserName!.ToLower() == userName &&
                x.OrganizationID == orgId);

        if (user == null)
        {
            resultMessage = "User not found.";
            return false;
        }

        deviceGroup.Devices ??= new List<Device>();
        user.DeviceGroups ??= new List<DeviceGroup>();

        if (deviceGroup.Users.Any(x => x.Id == user.Id))
        {
            resultMessage = "User already in group.";
            return false;
        }

        deviceGroup.Users.Add(user);
        user.DeviceGroups.Add(deviceGroup);
        dbContext.SaveChanges();
        resultMessage = user.Id;
        return true;
    }

    public async Task CleanupOldRecords()
    {
        var settings = await GetSettings();
        using var dbContext = _appDbFactory.GetContext();

        if (settings.DataRetentionInDays < 0)
        {
            return;
        }

        var expirationDate = DateTimeOffset.Now - TimeSpan.FromDays(settings.DataRetentionInDays);

        var scriptRuns = await dbContext.ScriptRuns
            .Include(x => x.Results)
            .Include(x => x.Devices)
            .Where(x => x.RunAt < expirationDate)
            .ToArrayAsync();

        foreach (var run in scriptRuns)
        {
            run.Devices?.Clear();
            run.Results?.Clear();
        }

        dbContext.RemoveRange(scriptRuns);

        var commandResults = dbContext.ScriptResults
                                .Where(x => x.TimeStamp < expirationDate);

        dbContext.RemoveRange(commandResults);

        var sharedFiles = dbContext.SharedFiles
                                .Where(x => x.Timestamp < expirationDate);

        dbContext.RemoveRange(sharedFiles);

        await dbContext.SaveChangesAsync();
    }

    public async Task<Result<ApiToken>> CreateApiToken(string userName, string tokenName, string secretHash)
    {
        using var dbContext = _appDbFactory.GetContext();

        var user = dbContext.Users.FirstOrDefault(x => x.UserName == userName);

        if (user is null)
        {
            return Result.Fail<ApiToken>("User not found.");
        }
        
        var newToken = new ApiToken()
        {
            Name = tokenName,
            OrganizationID = user.OrganizationID,
            Secret = secretHash
        };
        dbContext.ApiTokens.Add(newToken);
        await dbContext.SaveChangesAsync();
        return Result.Ok(newToken);
    }

    public async Task DeleteAlert(Alert alert)
    {
        using var dbContext = _appDbFactory.GetContext();

        dbContext.Alerts.Remove(alert);
        await dbContext.SaveChangesAsync();
    }

    public async Task DeleteAllAlerts(string orgId, string? userName = null)
    {
        using var dbContext = _appDbFactory.GetContext();

        var alerts = dbContext.Alerts.Where(x => x.OrganizationID == orgId);

        if (!string.IsNullOrWhiteSpace(userName))
        {
            var user = await dbContext.Users
                .AsNoTracking()
                .FirstOrDefaultAsync(x => x.UserName == userName);

            if (user is not null)
            {
                alerts = alerts.Where(x => x.UserID == user.Id);
            }
        }

        dbContext.Alerts.RemoveRange(alerts);
        await dbContext.SaveChangesAsync();
    }

    public async Task<Result> DeleteApiToken(string userName, string tokenId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var user = dbContext.Users.FirstOrDefault(x => x.UserName == userName);

        if (user is null)
        {
            return Result.Fail("User not found.");
        }

        var token = dbContext.ApiTokens.FirstOrDefault(x =>
            x.OrganizationID == user.OrganizationID &&
            x.ID == tokenId);

        if (token is null)
        {
            return Result.Fail("Token not found.");
        }

        dbContext.ApiTokens.Remove(token);
        await dbContext.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task<Result> DeleteDeviceGroup(string orgId, string deviceGroupID)
    {
        using var dbContext = _appDbFactory.GetContext();

        var deviceGroup = dbContext.DeviceGroups
            .Include(x => x.Devices)
            .Include(x => x.Users)
            .ThenInclude(x => x.DeviceGroups)
            .FirstOrDefault(x =>
                x.ID == deviceGroupID &&
                x.OrganizationID == orgId);

        if (deviceGroup is null)
        {
            return Result.Fail("Device group not found.");
        }

        deviceGroup.Devices.ForEach(x =>
        {
            x.DeviceGroup = null;
            x.DeviceGroupID = null;
        });

        deviceGroup.Users.ForEach(x =>
        {
            x.DeviceGroups.Remove(deviceGroup);
        });

        deviceGroup.Devices.Clear();
        deviceGroup.Users.Clear();

        dbContext.DeviceGroups.Remove(deviceGroup);

        await dbContext.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task<Result> DeleteInvite(string orgId, string inviteID)
    {
        using var dbContext = _appDbFactory.GetContext();

        var invite = dbContext.InviteLinks.FirstOrDefault(x =>
            x.OrganizationID == orgId &&
            x.ID == inviteID);

        if (invite is null)
        {
            return Result.Fail("Invite not found.");
        }

        var user = dbContext.Users.FirstOrDefault(x => x.UserName == invite.InvitedUser);

        if (user is null)
        {
            return Result.Fail("User not found.");
        }

        if (string.IsNullOrWhiteSpace(user.PasswordHash))
        {
            dbContext.Remove(user);
        }

        dbContext.Remove(invite);
        await dbContext.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task DeleteSavedScript(Guid scriptId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var schedules = await dbContext.ScriptSchedules
            .Where(x => x.SavedScriptId == scriptId)
            .ToListAsync();

        if (schedules.Count > 0)
        {
            dbContext.ScriptSchedules.RemoveRange(schedules);
        }

        var script = await dbContext.SavedScripts
            .Include(x => x.ScriptResults)
            .Include(x => x.ScriptRuns)
            .FirstOrDefaultAsync(x => x.Id == scriptId);

        if (script is not null)
        {
            dbContext.SavedScripts.Remove(script);
        }

        await dbContext.SaveChangesAsync();
    }

    public async Task DeleteScriptSchedule(int scriptScheduleId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var schedule = await dbContext.ScriptSchedules
            .Include(x => x.ScriptRuns)
            .ThenInclude(x => x.Results)
            .Include(x => x.Devices)
            .Include(x => x.DeviceGroups)
            .FirstOrDefaultAsync(x => x.Id == scriptScheduleId);

        if (schedule is not null)
        {
            dbContext.ScriptSchedules.Remove(schedule);
            await dbContext.SaveChangesAsync();
        }
    }

    public async Task<Result<Alert>> GetAlert(string alertId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var alert = await dbContext.Alerts
            .AsNoTracking()
            .Include(x => x.Device)
            .Include(x => x.User)
            .FirstOrDefaultAsync(x => x.ID == alertId);

        if (alert is null)
        {
            return Result.Fail<Alert>("Alert not found.");
        }

        return Result.Ok(alert);
    }

    public Alert[] GetAlerts(string userId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.Alerts
            .AsNoTracking()
            .Include(x => x.Device)
            .Include(x => x.User)
            .Where(x => x.UserID == userId)
            .OrderByDescending(x => x.CreatedOn)
            .ToArray();
    }

    public ApiToken[] GetAllApiTokens(string userId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var user = dbContext.Users.FirstOrDefault(x => x.Id == userId);

        if (user is null)
        {
            return Array.Empty<ApiToken>();
        }

        return dbContext.ApiTokens
            .AsNoTracking()
            .Where(x => x.OrganizationID == user.OrganizationID)
            .OrderByDescending(x => x.LastUsed)
            .ToArray();
    }

    public ScriptResult[] GetAllCommandResults(string orgId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.ScriptResults
            .AsNoTracking()
            .Where(x => x.OrganizationID == orgId)
            .OrderByDescending(x => x.TimeStamp)
            .ToArray();
    }

    public ScriptResult[] GetAllCommandResultsForUser(string orgId, string userName, string deviceId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.ScriptResults
            .AsNoTracking()
            .Where(x => x.OrganizationID == orgId &&
                x.SenderUserName == userName &&
                x.DeviceID == deviceId)
            .OrderByDescending(x => x.TimeStamp)
            .ToArray();
    }

    public InviteLink[] GetAllInviteLinks(string organizationId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.InviteLinks
            .AsNoTracking()
            .Where(x => x.OrganizationID == organizationId)
            .ToArray();
    }

    public ScriptResult[] GetAllScriptResults(string orgId, string deviceId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.ScriptResults
            .AsNoTracking()
            .Where(x => x.OrganizationID == orgId && x.DeviceID == deviceId)
            .OrderByDescending(x => x.TimeStamp)
            .ToArray();
    }

    public ScriptResult[] GetAllScriptResultsForUser(string orgId, string userName)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.ScriptResults
            .AsNoTracking()
            .Where(x => x.OrganizationID == orgId && x.SenderUserName == userName)
            .OrderByDescending(x => x.TimeStamp)
            .ToArray();
    }

    public async Task<Result<ApiToken>> GetApiKey(string keyId)
    {
        if (string.IsNullOrWhiteSpace(keyId))
        {
            return Result.Fail<ApiToken>("Key ID cannot be empty.");
        }

        using var dbContext = _appDbFactory.GetContext();

        var token = await dbContext.ApiTokens
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.ID == keyId);

        if (token is null)
        {
            return Result.Fail<ApiToken>("API key not found.");
        }

        return Result.Ok(token);
    }

    public async Task<Result<BrandingInfo>> GetBrandingInfo(string organizationId)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Result.Fail<BrandingInfo>("Organization ID cannot be empty.");
        }

        using var dbContext = _appDbFactory.GetContext();

        var organization = await dbContext.Organizations
          .AsNoTracking()
          .Include(x => x.BrandingInfo)
          .FirstOrDefaultAsync(x => x.ID == organizationId);

        if (organization is null)
        {
            return Result.Fail<BrandingInfo>("Organization not found.");
        }

        if (organization.BrandingInfo is null)
        {
            var brandingInfo = new BrandingInfo()
            {
                OrganizationId = organizationId
            };

            dbContext.BrandingInfos.Add(brandingInfo);
            organization.BrandingInfo = brandingInfo;

            await dbContext.SaveChangesAsync();
        }
        return Result.Ok(organization.BrandingInfo);
    }

    public int GetDeviceCount()
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.Devices.Count();
    }

    public int GetDeviceCount(RemotelyUser user)
    {
        using var dbContext = _appDbFactory.GetContext();

        if (user.IsAdministrator)
        {
            return GetDeviceCount();
        }

        return dbContext.Users
            .AsNoTracking()
            .Include(x => x.DeviceGroups)
            .ThenInclude(x => x.Devices)
            .Where(x => x.Id == user.Id)
            .SelectMany(x => x.DeviceGroups)
            .SelectMany(x => x.Devices)
            .Count();
    }

    public DeviceGroup[] GetDeviceGroups(string username)
    {
        using var dbContext = _appDbFactory.GetContext();

        var user = dbContext.Users
            .AsNoTracking()
            .FirstOrDefault(x => x.UserName == username);

        if (user is null)
        {
            return Array.Empty<DeviceGroup>();
        }
        var userId = user.Id;

        var groupIds = dbContext.DeviceGroups
            .AsNoTracking()
            .Include(x => x.Users)
            .ThenInclude(x => x.DeviceGroups)
            .Where(x =>
                x.OrganizationID == user.OrganizationID &&
                (
                    user.IsAdministrator ||
                    x.Users.Any(x => x.Id == userId)
                )
            )
            .Select(x => x.ID)
            .ToHashSet();

        if (groupIds.Any())
        {
            return dbContext.DeviceGroups
                .AsNoTracking()
                .Where(x => groupIds.Contains(x.ID))
                .OrderBy(x => x.Name)
                .ToArray();
        }

        return Array.Empty<DeviceGroup>();
    }

    public DeviceGroup[] GetDeviceGroupsForOrganization(string organizationId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.DeviceGroups
            .AsNoTracking()
            .Include(x => x.Users)
            .Where(x => x.OrganizationID == organizationId)
            .OrderBy(x => x.Name)
            .ToArray();
    }

    public List<Device> GetDevices(IEnumerable<string> deviceIds)
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.Devices
            .AsNoTracking()
            .Where(x => deviceIds.Contains(x.ID))
            .ToList();
    }

    public async Task<IEnumerable<ScriptRun>> GetPendingScriptRuns(string deviceId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var device = await dbContext.Devices
            .AsNoTracking()
            .Include(x => x.ScriptRuns)
            .ThenInclude(x => x.Results)
            .Include(x => x.ScriptResults)
            .FirstOrDefaultAsync(x => x.ID == deviceId);

        if (device is null)
        {
            return Enumerable.Empty<ScriptRun>();
        }

        device.ScriptResults ??= new();
        var scriptResultsLookup = device.ScriptResults
            .Select(x => x.ScriptRunId)
            .Distinct()
            .ToHashSet();

        return device.ScriptRuns
            .OrderByDescending(x => x.RunAt)
            .DistinctBy(x => x.SavedScriptId)
            .Where(x => !scriptResultsLookup.Contains(x.Id))
            .ToArray();
    }

    public async Task<List<SavedScript>> GetQuickScripts(string userId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return await dbContext.SavedScripts
            .Where(x => x.CreatorId == userId && x.IsQuickScript)
            .ToListAsync();
    }

    public async Task<Result<SavedScript>> GetSavedScript(string userId, Guid scriptId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var script = await dbContext.SavedScripts
            .AsNoTracking()
            .Include(x => x.Creator)
            .FirstOrDefaultAsync(x =>
                x.Id == scriptId &&
                (x.IsPublic || x.CreatorId == userId));

        if (script is null)
        {
            return Result.Fail<SavedScript>("Script not found.");
        }
        return Result.Ok(script);
    }

    public async Task<Result<SavedScript>> GetSavedScript(Guid scriptId)
    {
        using var dbContext = _appDbFactory.GetContext();
        var script = await dbContext.SavedScripts
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.Id == scriptId);

        if (script is null)
        {
            return Result.Fail<SavedScript>("Script not found.");
        }
        return Result.Ok(script);
    }

    public async Task<List<SavedScript>> GetSavedScriptsWithoutContent(string userId, string organizationId)
    {
        using var dbContext = _appDbFactory.GetContext();

        return await dbContext.SavedScripts
            .AsNoTracking()
            .Include(x => x.Creator)
            .Where(x => 
                x.Creator!.OrganizationID == organizationId &&
                (x.IsPublic || x.CreatorId == userId))
            .Select(x => new SavedScript()
            {
                Creator = x.Creator,
                CreatorId = x.CreatorId,
                FolderPath = x.FolderPath,
                Id = x.Id,
                IsPublic = x.IsPublic,
                IsQuickScript = x.IsQuickScript,
                Name = x.Name,
                OrganizationID = x.OrganizationID
            })
            .ToListAsync();
    }

    public async Task<Result<ScriptResult>> GetScriptResult(string resultId, string orgId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var result = await dbContext.ScriptResults
            .AsNoTracking()
            .FirstOrDefaultAsync(x =>
                x.OrganizationID == orgId &&
                x.ID == resultId);

        if (result is null)
        {
            return Result.Fail<ScriptResult>("Script result not found.");
        }
        return Result.Ok(result);
    }

    public async Task<Result<ScriptResult>> GetScriptResult(string resultId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var result = await dbContext.ScriptResults.FindAsync(resultId);

        if (result is null)
        {
            return Result.Fail<ScriptResult>("Script result not found.");
        }
        return Result.Ok(result);
    }

    public async Task<List<ScriptSchedule>> GetScriptSchedules(string organizationId)
    {
        using var dbContext = _appDbFactory.GetContext();
        return await dbContext.ScriptSchedules
            .AsNoTracking()
            .Include(x => x.Creator)
            .Include(x => x.Devices)
            .Include(x => x.DeviceGroups)
            .Where(x => x.OrganizationID == organizationId)
            .ToListAsync();
    }

    public async Task<List<ScriptSchedule>> GetScriptSchedulesDue()
    {
        using var dbContext = _appDbFactory.GetContext();

        var now = Time.Now;

        return await dbContext.ScriptSchedules
            .AsNoTracking()
            .Include(x => x.Devices)
            .Include(x => x.DeviceGroups)
            .ThenInclude(x => x.Devices)
            .Where(x => x.NextRun < now)
            .ToListAsync();
    }

    public List<string> GetServerAdmins()
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.Users
            .AsNoTracking()
            .Where(x => x.IsServerAdmin && x.UserName != null)
            .Select(x => x.UserName!)
            .ToList();
    }

    public async Task<SettingsModel> GetSettings()
    {
        await _settingsLock.WaitAsync();
        try
        {
            using var dbContext = _appDbFactory.GetContext();
            return await dbContext.GetAppSettings();
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while getting settings from database.");
            return new();
        }
        finally
        {
            _settingsLock.Release();
        }
    }

    public async Task<Result<SharedFile>> GetSharedFiled(string fileId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var file = await dbContext.SharedFiles.FindAsync(fileId);

        if (file is null)
        {
            return Result.Fail<SharedFile>("File not found.");
        }
        return Result.Ok(file);
    }

    public int GetTotalDevices()
    {
        using var dbContext = _appDbFactory.GetContext();

        return dbContext.Devices.Count();
    }

    public async Task<Result> JoinViaInvitation(string userName, string inviteId)
    {
        if (string.IsNullOrWhiteSpace(userName))
        {
            return Result.Fail("Username cannot be empty.");
        }
        if (string.IsNullOrWhiteSpace(inviteId))
        {
            return Result.Fail("Invite ID cannot be empty.");
        }

        using var dbContext = _appDbFactory.GetContext();

        var invite = await dbContext.InviteLinks
            .FirstOrDefaultAsync(x =>
                x.InvitedUser!.ToLower() == userName.ToLower() &&
                x.ID == inviteId);

        if (invite is null)
        {
            return Result.Fail("Invite not found.");
        }

        var user = await dbContext.Users
            .FirstOrDefaultAsync(x => x.UserName == userName);

        if (user is null)
        {
            return Result.Fail("User not found.");
        }

        var organization = await dbContext.Organizations
            .Include(x => x.RemotelyUsers)
            .FirstOrDefaultAsync(x => x.ID == invite.OrganizationID);

        if (organization is null)
        {
            return Result.Fail("Organization not found.");
        }

        user.Organization = organization;
        user.OrganizationID = organization.ID;
        user.IsAdministrator = invite.IsAdmin;
        organization.RemotelyUsers.Add(user);

        await dbContext.SaveChangesAsync();

        dbContext.InviteLinks.Remove(invite);
        dbContext.SaveChanges();
        return Result.Ok();
    }

    public void RemoveDevices(string[] deviceIDs)
    {
        using var dbContext = _appDbFactory.GetContext();

        var devices = dbContext.Devices
            .Include(x => x.ScriptResults)
            .Include(x => x.ScriptRuns)
            .Include(x => x.ScriptSchedules)
            .Include(x => x.DeviceGroup)
            .Include(x => x.Alerts)
            .Where(x => deviceIDs.Contains(x.ID));

        dbContext.Devices.RemoveRange(devices);
        dbContext.SaveChanges();
    }

    public async Task<bool> RemoveUserFromDeviceGroup(string orgID, string groupID, string userID)
    {
        using var dbContext = _appDbFactory.GetContext();

        var deviceGroup = await dbContext.DeviceGroups
            .Include(x => x.Users)
            .ThenInclude(x => x.DeviceGroups)
            .FirstOrDefaultAsync(x =>
                x.ID == groupID &&
                x.OrganizationID == orgID);

        if (deviceGroup?.Users?.Any(x => x.Id == userID) != true)
        {
            return false;
        }

        var user = deviceGroup.Users.FirstOrDefault(x => x.Id == userID);

        if (user is null)
        {
            return false;
        }

        user.DeviceGroups.Remove(deviceGroup);
        deviceGroup.Users.Remove(user);

        await dbContext.SaveChangesAsync();
        return true;
    }

    public async Task<Result> RenameApiToken(string userName, string tokenId, string tokenName)
    {
        using var dbContext = _appDbFactory.GetContext();

        var user = await dbContext.Users.FirstOrDefaultAsync(x => x.UserName == userName);
        if (user is null)
        {
            return Result.Fail("User not found.");
        }

        var token = await dbContext.ApiTokens.FirstOrDefaultAsync(x =>
            x.OrganizationID == user.OrganizationID &&
            x.ID == tokenId);

        if (token is null)
        {
            return Result.Fail("API token not found.");
        }

        token.Name = tokenName;
        await dbContext.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task ResetBranding(string organizationId)
    {
        using var dbContext = _appDbFactory.GetContext();

        var organization = await dbContext.Organizations
           .Include(x => x.BrandingInfo)
           .FirstOrDefaultAsync(x => x.ID == organizationId);

        if (organization?.BrandingInfo is null)
        {
            return;
        }

        var entry = dbContext.Entry(organization.BrandingInfo);
        entry.CurrentValues.SetValues(BrandingInfo.Default);
        
        await dbContext.SaveChangesAsync();
    }

    public async Task SaveSettings(SettingsModel settings)
    {
        await _settingsLock.WaitAsync();
        try
        {
            using var dbContext = _appDbFactory.GetContext();
            var record = await dbContext.KeyValueRecords.FindAsync(SettingsModel.DbKey);
            if (record is null)
            {
                record = new()
                {
                    Key = SettingsModel.DbKey,
                };
                await dbContext.KeyValueRecords.AddAsync(record);
                await dbContext.SaveChangesAsync();
            }
            record.Value = JsonSerializer.Serialize(settings);
            await dbContext.SaveChangesAsync();
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while saving settings to database.");
        }
        finally
        {
            _settingsLock.Release();
        }
    }

    public void SetServerVerificationToken(string deviceID, string verificationToken)
    {
        using var dbContext = _appDbFactory.GetContext();

        var device = dbContext.Devices.Find(deviceID);
        if (device != null)
        {
            device.ServerVerificationToken = verificationToken;
            dbContext.SaveChanges();
        }
    }

    public async Task<bool> TempPasswordSignIn(string email, string password)
    {
        if (string.IsNullOrWhiteSpace(password))
        {
            return false;
        }

        using var dbContext = _appDbFactory.GetContext();

        var user = await dbContext.Users
            .FirstOrDefaultAsync(x => x.UserName == email);

        if (user?.TempPassword != password)
        {
            return false;
        }

        user.TempPassword = string.Empty;
        await dbContext.SaveChangesAsync();
        return true;
    }

    public async Task UpdateBrandingInfo(
        string organizationId,
        string productName,
        byte[] iconBytes)
    {
        using var dbContext = _appDbFactory.GetContext();

        var organization = await dbContext.Organizations
            .Include(x => x.BrandingInfo)
            .FirstOrDefaultAsync(x => x.ID == organizationId);

        if (organization is null)
        {
            return;
        }

        organization.BrandingInfo ??= new BrandingInfo();

        organization.BrandingInfo.Product = productName;

        if (iconBytes?.Any() == true)
        {
            organization.BrandingInfo.Icon = iconBytes;
        }

        await dbContext.SaveChangesAsync();
    }

    public async Task<bool> ValidateApiKey(string keyId, string apiSecret, string requestPath, string remoteIP)
    {
        using var dbContext = _appDbFactory.GetContext();

        var hasher = new PasswordHasher<string>();
        var token = await dbContext.ApiTokens.FirstOrDefaultAsync(x => x.ID == keyId);

        var isValid = 
            !string.IsNullOrWhiteSpace(token?.Secret) &&
            hasher.VerifyHashedPassword(string.Empty, token.Secret, apiSecret) == PasswordVerificationResult.Success;

        if (token is not null)
        {
            token.LastUsed = DateTimeOffset.Now;
            await dbContext.SaveChangesAsync();
        }

        _logger.LogInformation(
            "API token used.  Token: {keyId}.  Path: {requestPath}.  Validated: {isValid}.  Remote IP: {remoteIP}", 
            keyId,
            requestPath,
            isValid,
            remoteIP);

        return isValid;
    }

    private async Task<string> AddSharedFileImpl(
        string fileName,
        byte[] fileContents,
        string contentType,
        string organizationId)
    {
        var settings = await GetSettings();
        using var dbContext = _appDbFactory.GetContext();

        var expirationDate = DateTimeOffset.Now.AddDays(-settings.DataRetentionInDays);
        var expiredFiles = dbContext.SharedFiles.Where(x => x.Timestamp < expirationDate);
        dbContext.RemoveRange(expiredFiles);

        var sharedFile = new SharedFile()
        {
            FileContents = fileContents,
            FileName = fileName,
            ContentType = contentType,
            OrganizationID = organizationId
        };

        dbContext.SharedFiles.Add(sharedFile);

        await dbContext.SaveChangesAsync();
        return sharedFile.ID;
    }

    private string[] FilterUsersByDevicePermissionInternal(AppDb dbContext, IEnumerable<string> userIDs, string deviceID)
    {
        var device = dbContext.Devices
             .Include(x => x.DeviceGroup)
             .ThenInclude(x => x!.Users)
             .FirstOrDefault(x => x.ID == deviceID);

        if (device is null)
        {
            return Array.Empty<string>();
        }

        var orgUsers = dbContext.Users
            .Where(user =>
                user.OrganizationID == device.OrganizationID &&
                userIDs.Contains(user.Id));

        if (string.IsNullOrWhiteSpace(device.DeviceGroupID))
        {
            return orgUsers
                .Select(x => x.Id)
                .ToArray();
        }

        var allowedUsers = device?.DeviceGroup?.Users?.Select(x => x.Id) ?? Array.Empty<string>();

        return orgUsers
            .Where(user =>
                user.IsAdministrator ||
                allowedUsers.Contains(user.Id)
            )
            .Select(x => x.Id)
            .ToArray();
    }
}