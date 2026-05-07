using Microsoft.AspNetCore.Authorization;
using Remotely.Server.Services;
using Remotely.Server.Services.Organizations;
using Remotely.Server.Services.UserDirectory;

namespace Remotely.Server.Auth;

/// <summary>
/// Authorization handler for <see cref="PackageManagerRequirement"/>.
/// Requires the caller to be an authenticated organization administrator
/// AND for their organization to have <c>PackageManagerEnabled = true</c>.
/// </summary>
public class PackageManagerRequirementHandler : AuthorizationHandler<PackageManagerRequirement>
{
    private readonly IOrganizationService _organizationService;
    private readonly IUserDirectoryService _userDirectoryService;

    public PackageManagerRequirementHandler(
        IOrganizationService organizationService,
        IUserDirectoryService userDirectoryService)
    {
        _organizationService = organizationService;
        _userDirectoryService = userDirectoryService;
    }

    protected override async Task HandleRequirementAsync(AuthorizationHandlerContext context, PackageManagerRequirement requirement)
    {
        if (context.User.Identity?.IsAuthenticated != true ||
            string.IsNullOrWhiteSpace(context.User.Identity.Name))
        {
            context.Fail();
            return;
        }

        var userResult = await _userDirectoryService.GetUserByName(context.User.Identity.Name);

        if (!userResult.IsSuccess || !userResult.Value.IsAdministrator)
        {
            context.Fail();
            return;
        }

        var orgResult = await _organizationService.GetOrganizationById(userResult.Value.OrganizationID);
        if (!orgResult.IsSuccess || !orgResult.Value.PackageManagerEnabled)
        {
            context.Fail();
            return;
        }

        context.Succeed(requirement);
    }
}
