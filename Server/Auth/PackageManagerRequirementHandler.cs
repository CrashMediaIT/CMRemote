using Microsoft.AspNetCore.Authorization;
using Remotely.Server.Services;

namespace Remotely.Server.Auth;

/// <summary>
/// Authorization handler for <see cref="PackageManagerRequirement"/>.
/// Requires the caller to be an authenticated organization administrator
/// AND for their organization to have <c>PackageManagerEnabled = true</c>.
/// </summary>
public class PackageManagerRequirementHandler : AuthorizationHandler<PackageManagerRequirement>
{
    private readonly IDataService _dataService;

    public PackageManagerRequirementHandler(IDataService dataService)
    {
        _dataService = dataService;
    }

    protected override async Task HandleRequirementAsync(AuthorizationHandlerContext context, PackageManagerRequirement requirement)
    {
        if (context.User.Identity?.IsAuthenticated != true ||
            string.IsNullOrWhiteSpace(context.User.Identity.Name))
        {
            context.Fail();
            return;
        }

        var userResult = await _dataService.GetUserByName(context.User.Identity.Name);

        if (!userResult.IsSuccess || !userResult.Value.IsAdministrator)
        {
            context.Fail();
            return;
        }

        var orgResult = await _dataService.GetOrganizationById(userResult.Value.OrganizationID);
        if (!orgResult.IsSuccess || !orgResult.Value.PackageManagerEnabled)
        {
            context.Fail();
            return;
        }

        context.Succeed(requirement);
    }
}
