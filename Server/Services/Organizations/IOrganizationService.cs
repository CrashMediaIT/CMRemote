// Source: CMRemote, clean-room implementation

using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.Organizations;

public interface IOrganizationService
{
    Task<Result<Organization>> GetDefaultOrganization();

    Task<Result<Organization>> GetOrganizationById(string organizationId);

    Task<Result<Organization>> GetOrganizationByUserName(string userName);

    int GetOrganizationCount();

    Task<int> GetOrganizationCountAsync();

    Task<Result<string>> GetOrganizationNameById(string organizationId);

    Task<Result<string>> GetOrganizationNameByUserName(string userName);

    Task SetIsDefaultOrganization(string orgId, bool isDefault);

    /// <summary>
    /// Toggles the per-org Package Manager opt-in. Disabling clears all
    /// existing inventory snapshots for the org so previously-cached app
    /// lists aren't visible after the feature is turned off.
    /// </summary>
    Task SetOrganizationPackageManagerEnabled(string orgId, bool isEnabled);

    Task<Result> UpdateOrganizationName(string orgId, string newName);
}
