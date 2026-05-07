// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Organizations;

public class OrganizationService : IOrganizationService
{
    private readonly IAppDbFactory _dbFactory;

    public OrganizationService(IAppDbFactory dbFactory)
    {
        _dbFactory = dbFactory;
    }

    public async Task<Result<Organization>> GetDefaultOrganization()
    {
        using var db = _dbFactory.GetContext();

        var org = await db.Organizations
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.IsDefaultOrganization);

        return org is null
            ? Result.Fail<Organization>("Organization not found.")
            : Result.Ok(org);
    }

    public async Task<Result<Organization>> GetOrganizationById(string organizationId)
    {
        using var db = _dbFactory.GetContext();

        var org = await db.Organizations.FindAsync(organizationId);

        return org is null
            ? Result.Fail<Organization>("Organization not found.")
            : Result.Ok(org);
    }

    public async Task<Result<Organization>> GetOrganizationByUserName(string userName)
    {
        if (string.IsNullOrWhiteSpace(userName))
        {
            return Result.Fail<Organization>("User name is required.");
        }

        using var db = _dbFactory.GetContext();

        var normalized = userName.ToLower();
        var user = await db.Users
            .AsNoTracking()
            .Include(x => x.Organization)
            .FirstOrDefaultAsync(x => x.UserName!.ToLower() == normalized);

        return user?.Organization is null
            ? Result.Fail<Organization>("User not found.")
            : Result.Ok(user.Organization);
    }

    public async Task<int> GetOrganizationCountAsync()
    {
        using var db = _dbFactory.GetContext();
        return await db.Organizations.CountAsync();
    }

    public int GetOrganizationCount()
    {
        using var db = _dbFactory.GetContext();
        return db.Organizations.Count();
    }

    public async Task<Result<string>> GetOrganizationNameById(string organizationId)
    {
        using var db = _dbFactory.GetContext();

        var org = await db.Organizations
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.ID == organizationId);

        return org is null
            ? Result.Fail<string>("Organization not found.")
            : Result.Ok(org.OrganizationName);
    }

    public async Task<Result<string>> GetOrganizationNameByUserName(string userName)
    {
        if (string.IsNullOrWhiteSpace(userName))
        {
            return Result.Fail<string>("Username cannot be empty.");
        }

        using var db = _dbFactory.GetContext();

        var user = await db.Users
            .AsNoTracking()
            .Include(x => x.Organization)
            .FirstOrDefaultAsync(x => x.UserName == userName);

        if (user is null)
        {
            return Result.Fail<string>("User not found.");
        }

        var orgName = $"{user.Organization?.OrganizationName}";
        return Result.Ok(orgName);
    }

    public async Task SetIsDefaultOrganization(string orgId, bool isDefault)
    {
        using var db = _dbFactory.GetContext();

        var organization = await db.Organizations.FindAsync(orgId);
        if (organization is null)
        {
            return;
        }

        if (isDefault)
        {
            await db.Organizations.ForEachAsync(x => x.IsDefaultOrganization = false);
        }

        organization.IsDefaultOrganization = isDefault;
        await db.SaveChangesAsync();
    }

    public async Task SetOrganizationPackageManagerEnabled(string orgId, bool isEnabled)
    {
        using var db = _dbFactory.GetContext();

        var organization = await db.Organizations.FindAsync(orgId);
        if (organization is null)
        {
            return;
        }

        organization.PackageManagerEnabled = isEnabled;

        if (!isEnabled)
        {
            // Drop any cached inventory so disabling the feature also
            // hides previously-collected app lists from the UI.
            var deviceIds = db.Devices
                .Where(d => d.OrganizationID == orgId)
                .Select(d => d.ID);

            var snapshots = db.DeviceInstalledApplicationsSnapshots
                .Where(s => deviceIds.Contains(s.DeviceId));
            db.DeviceInstalledApplicationsSnapshots.RemoveRange(snapshots);
        }

        await db.SaveChangesAsync();
    }

    public async Task<Result> UpdateOrganizationName(string orgId, string newName)
    {
        using var db = _dbFactory.GetContext();

        var org = await db.Organizations.FirstOrDefaultAsync(x => x.ID == orgId);
        if (org is null)
        {
            return Result.Fail("Organization not found.");
        }

        org.OrganizationName = newName;
        await db.SaveChangesAsync();
        return Result.Ok();
    }
}
