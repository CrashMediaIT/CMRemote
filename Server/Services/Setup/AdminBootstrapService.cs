using Microsoft.AspNetCore.Identity;
using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Setup;

/// <inheritdoc cref="IAdminBootstrapService" />
public class AdminBootstrapService : IAdminBootstrapService
{
    private readonly IAppDbFactory _dbFactory;
    private readonly UserManager<RemotelyUser> _userManager;
    private readonly ILogger<AdminBootstrapService> _logger;

    public AdminBootstrapService(
        IAppDbFactory dbFactory,
        UserManager<RemotelyUser> userManager,
        ILogger<AdminBootstrapService> logger)
    {
        _dbFactory = dbFactory;
        _userManager = userManager;
        _logger = logger;
    }

    /// <inheritdoc />
    public async Task<bool> IsRequiredAsync(CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();

        // The wizard's M1.4 step is needed iff the database has no
        // operator-visible identity yet. Both checks must pass; an
        // imported org with zero users is still a "needs admin"
        // state because the operator cannot otherwise sign in.
        var anyUser = await db.Users.AsNoTracking().AnyAsync(cancellationToken);
        if (anyUser)
        {
            return false;
        }
        var anyOrg = await db.Organizations.AsNoTracking().AnyAsync(cancellationToken);
        return !anyOrg;
    }

    /// <inheritdoc />
    public async Task<AdminBootstrapResult> CreateInitialAdminAsync(
        string organizationName,
        string email,
        string password,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationName))
        {
            return AdminBootstrapResult.Failure("Organisation name is required.");
        }
        if (string.IsNullOrWhiteSpace(email))
        {
            return AdminBootstrapResult.Failure("Email address is required.");
        }
        if (string.IsNullOrWhiteSpace(password))
        {
            return AdminBootstrapResult.Failure("Password is required.");
        }

        var trimmedOrgName = organizationName.Trim();
        if (trimmedOrgName.Length > 25)
        {
            // Match the storage cap on Organization.OrganizationName so
            // the operator gets a friendly error before EF would throw.
            trimmedOrgName = trimmedOrgName[..25];
        }
        var normalizedEmail = email.Trim().ToLowerInvariant();

        // Re-check the "no users yet" precondition inside the
        // operation so a race against another wizard instance can't
        // produce two server-admin accounts. The probe is cheap and
        // the wizard is single-operator anyway.
        if (!await IsRequiredAsync(cancellationToken))
        {
            return AdminBootstrapResult.Failure(
                "An organisation or user already exists. The admin bootstrap step " +
                "has already been completed by another browser session.");
        }

        // Step 1: persist the org (RemotelyUser.OrganizationID is a
        // required FK, so the org must exist before the user is
        // created via UserManager).
        Organization org;
        using (var db = _dbFactory.GetContext())
        {
            org = new Organization
            {
                OrganizationName = trimmedOrgName,
                IsDefaultOrganization = true,
            };
            db.Organizations.Add(org);
            await db.SaveChangesAsync(cancellationToken);
        }

        // Step 2: build the user, hand it to UserManager so the
        // configured IPasswordHasher hashes the password the same
        // way the rest of the app does. UserManager also stamps
        // SecurityStamp / ConcurrencyStamp.
        var user = new RemotelyUser
        {
            UserName = normalizedEmail,
            Email = normalizedEmail,
            EmailConfirmed = true,
            IsAdministrator = true,
            IsServerAdmin = true,
            OrganizationID = org.ID,
            UserOptions = new RemotelyUserOptions(),
            LockoutEnabled = true,
        };

        var createResult = await _userManager.CreateAsync(user, password);
        if (!createResult.Succeeded)
        {
            // Roll back the org row we just inserted so a re-attempt
            // with a stronger password doesn't leave a phantom org
            // around. Best-effort: a delete failure is logged but
            // the original Identity error is still what the operator
            // sees and can act on.
            await TryRollbackOrganizationAsync(org.ID, cancellationToken);
            return AdminBootstrapResult.FromIdentityErrors(createResult.Errors);
        }

        _logger.LogInformation(
            "Admin bootstrap complete: org {OrgId} + server-admin user {UserId} created.",
            org.ID, user.Id);

        return AdminBootstrapResult.Success(user.Id);
    }

    private async Task TryRollbackOrganizationAsync(
        string organizationId,
        CancellationToken cancellationToken)
    {
        try
        {
            using var db = _dbFactory.GetContext();
            var org = await db.Organizations
                .FirstOrDefaultAsync(x => x.ID == organizationId, cancellationToken);
            if (org is not null)
            {
                db.Organizations.Remove(org);
                await db.SaveChangesAsync(cancellationToken);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex,
                "Failed to roll back organisation {OrgId} after a failed " +
                "admin-bootstrap attempt; the operator may need to clear it manually.",
                organizationId);
        }
    }
}
