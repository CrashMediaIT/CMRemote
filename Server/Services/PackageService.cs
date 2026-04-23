using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using System.Text.RegularExpressions;

namespace Remotely.Server.Services;

/// <summary>
/// Org-scoped CRUD for <see cref="Package"/> definitions and
/// <see cref="DeploymentBundle"/>s. All callers MUST pass the caller's
/// <c>OrganizationID</c> so org isolation is enforced consistently —
/// the service rejects cross-org reads/writes.
/// </summary>
public interface IPackageService
{
    Task<IReadOnlyList<Package>> GetPackagesForOrg(string organizationId);

    Task<Package?> GetPackage(string organizationId, Guid packageId);

    Task<Result<Package>> CreatePackage(string organizationId, string? userId, Package package);

    Task<Result> DeletePackage(string organizationId, Guid packageId);

    Task<IReadOnlyList<DeploymentBundle>> GetBundlesForOrg(string organizationId);

    Task<DeploymentBundle?> GetBundle(string organizationId, Guid bundleId);

    Task<Result<DeploymentBundle>> CreateBundle(string organizationId, string? userId, string name, string? description);

    Task<Result> DeleteBundle(string organizationId, Guid bundleId);

    Task<Result<BundleItem>> AddBundleItem(string organizationId, Guid bundleId, Guid packageId, int order, bool continueOnFailure);

    Task<Result> RemoveBundleItem(string organizationId, Guid bundleId, Guid bundleItemId);
}

public class PackageService : IPackageService
{
    // Reject anything that could let a value escape an argv slot on the
    // agent. Provider-specific argument parsers may be more permissive,
    // but server-side we conservatively reject shell metacharacters in
    // operator-supplied install arguments before persistence.
    private static readonly Regex _disallowedArgChars = new(
        @"[`$;&|<>\r\n\u0000]",
        RegexOptions.Compiled);

    private readonly IAppDbFactory _dbFactory;
    private readonly ILogger<PackageService> _logger;

    public PackageService(IAppDbFactory dbFactory, ILogger<PackageService> logger)
    {
        _dbFactory = dbFactory;
        _logger = logger;
    }

    public async Task<IReadOnlyList<Package>> GetPackagesForOrg(string organizationId)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Array.Empty<Package>();
        }

        using var db = _dbFactory.GetContext();
        return await db.Packages
            .AsNoTracking()
            .Where(p => p.OrganizationID == organizationId)
            .OrderBy(p => p.Name)
            .ToListAsync();
    }

    public async Task<Package?> GetPackage(string organizationId, Guid packageId)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || packageId == Guid.Empty)
        {
            return null;
        }

        using var db = _dbFactory.GetContext();
        return await db.Packages
            .AsNoTracking()
            .FirstOrDefaultAsync(p => p.Id == packageId && p.OrganizationID == organizationId);
    }

    public async Task<Result<Package>> CreatePackage(string organizationId, string? userId, Package package)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Result.Fail<Package>("Organization ID is required.");
        }
        if (package is null)
        {
            return Result.Fail<Package>("Package is required.");
        }
        if (string.IsNullOrWhiteSpace(package.Name))
        {
            return Result.Fail<Package>("Name is required.");
        }
        if (string.IsNullOrWhiteSpace(package.PackageIdentifier))
        {
            return Result.Fail<Package>("Package identifier is required.");
        }
        if (package.Provider == PackageProvider.Unknown)
        {
            return Result.Fail<Package>("Provider is required.");
        }
        if (!IsValidArguments(package.InstallArguments))
        {
            return Result.Fail<Package>(
                "Install arguments contain disallowed characters. Shell metacharacters are not permitted.");
        }
        if (package.Provider == PackageProvider.Chocolatey &&
            !IsValidChocoIdentifier(package.PackageIdentifier))
        {
            return Result.Fail<Package>(
                "Invalid Chocolatey package id. Use lowercase letters, digits, '.', '-' or '_'.");
        }
        if (package.Provider == PackageProvider.UploadedMsi)
        {
            if (!Guid.TryParse(package.PackageIdentifier, out var msiId))
            {
                return Result.Fail<Package>(
                    "Invalid Uploaded MSI reference. Expected the GUID of an UploadedMsi row.");
            }
            using var probeDb = _dbFactory.GetContext();
            var msiExists = await probeDb.UploadedMsis
                .AsNoTracking()
                .AnyAsync(m => m.Id == msiId &&
                               m.OrganizationID == organizationId &&
                               !m.IsTombstoned);
            if (!msiExists)
            {
                return Result.Fail<Package>(
                    "Uploaded MSI not found in this organization (or it has been deleted).");
            }
        }

        using var db = _dbFactory.GetContext();

        package.Id = Guid.NewGuid();
        package.OrganizationID = organizationId;
        package.CreatedAt = DateTimeOffset.UtcNow;
        package.CreatedByUserId = userId;
        // Strip nav so EF doesn't try to insert / re-attach the org.
        package.Organization = null;

        db.Packages.Add(package);
        await db.SaveChangesAsync();
        return Result.Ok(package);
    }

    public async Task<Result> DeletePackage(string organizationId, Guid packageId)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || packageId == Guid.Empty)
        {
            return Result.Fail("Organization ID and package ID are required.");
        }

        using var db = _dbFactory.GetContext();
        var package = await db.Packages
            .FirstOrDefaultAsync(p => p.Id == packageId && p.OrganizationID == organizationId);
        if (package is null)
        {
            return Result.Fail("Package not found.");
        }

        // Refuse to delete a package that's referenced by a bundle —
        // dropping it would leave the bundle in an inconsistent state.
        var inUse = await db.BundleItems.AnyAsync(b => b.PackageId == packageId);
        if (inUse)
        {
            return Result.Fail("Package is part of a deployment bundle. Remove it from the bundle first.");
        }

        db.Packages.Remove(package);
        await db.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task<IReadOnlyList<DeploymentBundle>> GetBundlesForOrg(string organizationId)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Array.Empty<DeploymentBundle>();
        }

        using var db = _dbFactory.GetContext();
        return await db.DeploymentBundles
            .AsNoTracking()
            .Include(b => b.Items)
            .ThenInclude(i => i.Package)
            .Where(b => b.OrganizationID == organizationId)
            .OrderBy(b => b.Name)
            .ToListAsync();
    }

    public async Task<DeploymentBundle?> GetBundle(string organizationId, Guid bundleId)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || bundleId == Guid.Empty)
        {
            return null;
        }

        using var db = _dbFactory.GetContext();
        return await db.DeploymentBundles
            .AsNoTracking()
            .Include(b => b.Items)
            .ThenInclude(i => i.Package)
            .FirstOrDefaultAsync(b => b.Id == bundleId && b.OrganizationID == organizationId);
    }

    public async Task<Result<DeploymentBundle>> CreateBundle(string organizationId, string? userId, string name, string? description)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Result.Fail<DeploymentBundle>("Organization ID is required.");
        }
        if (string.IsNullOrWhiteSpace(name))
        {
            return Result.Fail<DeploymentBundle>("Bundle name is required.");
        }

        using var db = _dbFactory.GetContext();
        var bundle = new DeploymentBundle
        {
            Id = Guid.NewGuid(),
            OrganizationID = organizationId,
            Name = name.Trim(),
            Description = description,
            CreatedAt = DateTimeOffset.UtcNow,
            CreatedByUserId = userId,
        };
        db.DeploymentBundles.Add(bundle);
        await db.SaveChangesAsync();
        return Result.Ok(bundle);
    }

    public async Task<Result> DeleteBundle(string organizationId, Guid bundleId)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || bundleId == Guid.Empty)
        {
            return Result.Fail("Organization ID and bundle ID are required.");
        }

        using var db = _dbFactory.GetContext();
        var bundle = await db.DeploymentBundles
            .FirstOrDefaultAsync(b => b.Id == bundleId && b.OrganizationID == organizationId);
        if (bundle is null)
        {
            return Result.Fail("Bundle not found.");
        }
        db.DeploymentBundles.Remove(bundle);
        await db.SaveChangesAsync();
        return Result.Ok();
    }

    public async Task<Result<BundleItem>> AddBundleItem(string organizationId, Guid bundleId, Guid packageId, int order, bool continueOnFailure)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || bundleId == Guid.Empty || packageId == Guid.Empty)
        {
            return Result.Fail<BundleItem>("Organization ID, bundle ID and package ID are required.");
        }

        using var db = _dbFactory.GetContext();
        var bundle = await db.DeploymentBundles
            .FirstOrDefaultAsync(b => b.Id == bundleId && b.OrganizationID == organizationId);
        if (bundle is null)
        {
            return Result.Fail<BundleItem>("Bundle not found.");
        }

        var package = await db.Packages
            .FirstOrDefaultAsync(p => p.Id == packageId && p.OrganizationID == organizationId);
        if (package is null)
        {
            return Result.Fail<BundleItem>("Package not found in this organization.");
        }

        var item = new BundleItem
        {
            Id = Guid.NewGuid(),
            DeploymentBundleId = bundleId,
            PackageId = packageId,
            Order = order,
            ContinueOnFailure = continueOnFailure,
        };
        db.BundleItems.Add(item);
        await db.SaveChangesAsync();
        return Result.Ok(item);
    }

    public async Task<Result> RemoveBundleItem(string organizationId, Guid bundleId, Guid bundleItemId)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || bundleId == Guid.Empty || bundleItemId == Guid.Empty)
        {
            return Result.Fail("Organization ID, bundle ID and item ID are required.");
        }

        using var db = _dbFactory.GetContext();
        var item = await db.BundleItems
            .Include(x => x.DeploymentBundle)
            .FirstOrDefaultAsync(x => x.Id == bundleItemId && x.DeploymentBundleId == bundleId);

        if (item is null || item.DeploymentBundle is null ||
            item.DeploymentBundle.OrganizationID != organizationId)
        {
            return Result.Fail("Bundle item not found.");
        }

        db.BundleItems.Remove(item);
        await db.SaveChangesAsync();
        return Result.Ok();
    }

    internal static bool IsValidArguments(string? arguments)
    {
        if (string.IsNullOrEmpty(arguments))
        {
            return true;
        }
        return !_disallowedArgChars.IsMatch(arguments);
    }

    internal static bool IsValidChocoIdentifier(string id)
    {
        if (string.IsNullOrWhiteSpace(id) || id.Length > 100)
        {
            return false;
        }
        // Chocolatey package ids are case-insensitive and limited to
        // letters, digits, '.', '-', '_' (matching the official spec).
        for (var i = 0; i < id.Length; i++)
        {
            var c = id[i];
            var ok = (c >= 'a' && c <= 'z') ||
                     (c >= 'A' && c <= 'Z') ||
                     (c >= '0' && c <= '9') ||
                     c == '.' || c == '-' || c == '_';
            if (!ok)
            {
                return false;
            }
        }
        return true;
    }
}
