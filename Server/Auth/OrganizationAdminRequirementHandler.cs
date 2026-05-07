using Microsoft.AspNetCore.Authorization;
using Remotely.Server.Services;
using Remotely.Server.Services.UserDirectory;

namespace Remotely.Server.Auth;

public class OrganizationAdminRequirementHandler : AuthorizationHandler<OrganizationAdminRequirement>
{
    private readonly IUserDirectoryService _userDirectoryService;

    public OrganizationAdminRequirementHandler(IUserDirectoryService userDirectoryService)
    {
        _userDirectoryService = userDirectoryService;
    }

    protected override async Task HandleRequirementAsync(AuthorizationHandlerContext context, OrganizationAdminRequirement requirement)
    {
        if (context.User.Identity?.IsAuthenticated != true ||
            string.IsNullOrWhiteSpace(context.User.Identity.Name))
        {
            context.Fail();
            return;
        }

        var userResult = await _userDirectoryService.GetUserByName(context.User.Identity.Name);

        if (!userResult.IsSuccess ||
            !userResult.Value.IsAdministrator)
        {
            context.Fail();
            return;
        }

        context.Succeed(requirement);
    }
}
