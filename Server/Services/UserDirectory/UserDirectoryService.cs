// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Server.Extensions;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.UserDirectory;

public class UserDirectoryService : IUserDirectoryService
{
    private readonly IAppDbFactory _dbFactory;
    private readonly ILogger<UserDirectoryService> _logger;

    public UserDirectoryService(
        IAppDbFactory dbFactory,
        ILogger<UserDirectoryService> logger)
    {
        _dbFactory = dbFactory;
        _logger = logger;
    }

    public async Task ChangeUserIsAdmin(string organizationId, string targetUserId, bool isAdmin)
    {
        using var db = _dbFactory.GetContext();

        var targetUser = await db.Users.FirstOrDefaultAsync(x =>
            x.OrganizationID == organizationId &&
            x.Id == targetUserId);

        if (targetUser is null)
        {
            return;
        }

        targetUser.IsAdministrator = isAdmin;
        await db.SaveChangesAsync();
    }

    public async Task<Result> CreateUser(string userEmail, bool isAdmin, string organizationId)
    {
        using var db = _dbFactory.GetContext();

        try
        {
            var organization = await db.Organizations
                .Include(x => x.RemotelyUsers)
                .FirstOrDefaultAsync(x => x.ID == organizationId);

            if (organization is null)
            {
                return Result.Fail("Organization not found.");
            }

            var normalizedEmail = userEmail.Trim().ToLower();
            var user = new RemotelyUser
            {
                UserName = normalizedEmail,
                Email = normalizedEmail,
                IsAdministrator = isAdmin,
                OrganizationID = organizationId,
                UserOptions = new RemotelyUserOptions(),
                LockoutEnabled = true
            };

            db.Users.Add(user);
            organization.RemotelyUsers.Add(user);
            await db.SaveChangesAsync();
            return Result.Ok();
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while creating user for organization {id}.", organizationId);
            return Result.Fail("An error occurred while creating user.");
        }
    }

    public async Task<Result> DeleteUser(string orgId, string targetUserId)
    {
        using var db = _dbFactory.GetContext();

        var organizationExists = await db.Organizations.AnyAsync(x => x.ID == orgId);
        if (!organizationExists)
        {
            return Result.Fail("Organization not found.");
        }

        var target = await db.Users
            .Include(x => x.DeviceGroups)
            .ThenInclude(x => x.Devices)
            .Include(x => x.Organization)
            .Include(x => x.Alerts)
            .Include(x => x.SavedScripts)
            .ThenInclude(x => x.ScriptRuns)
            .Include(x => x.SavedScripts)
            .ThenInclude(x => x.ScriptResults)
            .Include(x => x.ScriptSchedules)
            .ThenInclude(x => x.ScriptRuns)
            .ThenInclude(x => x.Results)
            .FirstOrDefaultAsync(x =>
                x.Id == targetUserId &&
                x.OrganizationID == orgId);

        if (target is null)
        {
            return Result.Fail("User not found.");
        }

        db.Users.Remove(target);
        await db.SaveChangesAsync();
        return Result.Ok();
    }

    public bool DoesUserExist(string userName)
    {
        if (string.IsNullOrWhiteSpace(userName))
        {
            return false;
        }

        using var db = _dbFactory.GetContext();
        var normalizedUserName = userName.Trim().ToLower();

        return db.Users
            .Where(x => x.UserName != null)
            .Any(x => x.UserName!.Trim().ToLower() == normalizedUserName);
    }

    public RemotelyUser[] GetAllUsersForServer()
    {
        using var db = _dbFactory.GetContext();

        return db.Users
            .AsNoTracking()
            .ToArray();
    }

    public async Task<RemotelyUser[]> GetAllUsersInOrganization(string orgId)
    {
        if (string.IsNullOrWhiteSpace(orgId))
        {
            return Array.Empty<RemotelyUser>();
        }

        using var db = _dbFactory.GetContext();

        var organization = await db.Organizations
            .AsNoTracking()
            .Include(x => x.RemotelyUsers)
            .FirstOrDefaultAsync(x => x.ID == orgId);

        return organization?.RemotelyUsers.ToArray() ?? Array.Empty<RemotelyUser>();
    }

    public async Task<Result<RemotelyUser>> GetUserById(string userId)
    {
        if (string.IsNullOrWhiteSpace(userId))
        {
            return Result.Fail<RemotelyUser>("User ID cannot be empty.");
        }

        using var db = _dbFactory.GetContext();

        var user = await db.Users
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.Id == userId);

        return user is null
            ? Result.Fail<RemotelyUser>("User not found.")
            : Result.Ok(user);
    }

    public async Task<Result<RemotelyUser>> GetUserByName(
        string userName,
        Action<IQueryable<RemotelyUser>>? queryBuilder = null)
    {
        if (string.IsNullOrWhiteSpace(userName))
        {
            return Result.Fail<RemotelyUser>("Username cannot be empty.");
        }

        using var db = _dbFactory.GetContext();
        var normalizedUserName = userName.ToLower().Trim();

        var user = await db.Users
            .AsNoTracking()
            .Apply(queryBuilder)
            .FirstOrDefaultAsync(x =>
                x.UserName!.ToLower().Trim() == normalizedUserName);

        return user is null
            ? Result.Fail<RemotelyUser>("User not found.")
            : Result.Ok(user);
    }

    public async Task<Result<RemotelyUserOptions>> GetUserOptions(string userName)
    {
        using var db = _dbFactory.GetContext();

        var user = await db.Users
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.UserName == userName);

        return user is null
            ? Result.Fail<RemotelyUserOptions>("User not found.")
            : Result.Ok(user.UserOptions ?? new RemotelyUserOptions());
    }

    public async Task SetDisplayName(RemotelyUser user, string displayName)
    {
        using var db = _dbFactory.GetContext();

        var storedUser = await db.Users.FirstOrDefaultAsync(x => x.Id == user.Id);
        if (storedUser is null)
        {
            return;
        }

        var options = storedUser.UserOptions ?? new RemotelyUserOptions();
        options.DisplayName = displayName;
        storedUser.UserOptions = options;
        db.Entry(storedUser).Property(x => x.UserOptions).IsModified = true;
        await db.SaveChangesAsync();
    }

    public async Task SetIsServerAdmin(string targetUserId, bool isServerAdmin, string callerUserId)
    {
        using var db = _dbFactory.GetContext();

        var caller = await db.Users.FindAsync(callerUserId);
        if (caller?.IsServerAdmin != true)
        {
            return;
        }

        var targetUser = await db.Users.FindAsync(targetUserId);
        if (targetUser is null || caller.Id == targetUser.Id)
        {
            return;
        }

        targetUser.IsServerAdmin = isServerAdmin;
        await db.SaveChangesAsync();
    }

    public async Task<Result> UpdateUserOptions(string userName, RemotelyUserOptions options)
    {
        using var db = _dbFactory.GetContext();

        var user = await db.Users.FirstOrDefaultAsync(x => x.UserName == userName);
        if (user is null)
        {
            return Result.Fail("User not found.");
        }

        user.UserOptions = options;
        await db.SaveChangesAsync();
        return Result.Ok();
    }
}
