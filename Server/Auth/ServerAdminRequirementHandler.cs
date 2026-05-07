#nullable enable

using Remotely.Server.Services.UserDirectory;
using Microsoft.AspNetCore.Authorization;

namespace Remotely.Server.Auth;

public class ServerAdminRequirementHandler : AuthorizationHandler<ServerAdminRequirement>
{
    private readonly IUserDirectoryService _userDirectoryService;

    public ServerAdminRequirementHandler(IUserDirectoryService userDirectoryService)
    {
        _userDirectoryService = userDirectoryService;
    }

    protected override async Task HandleRequirementAsync(AuthorizationHandlerContext context, ServerAdminRequirement requirement)
    {
        if (context.User.Identity?.IsAuthenticated != true ||
            string.IsNullOrWhiteSpace(context.User.Identity.Name))
        {
            context.Fail();
            return;
        }

        var userResult = await _userDirectoryService.GetUserByName(context.User.Identity.Name);

        if (!userResult.IsSuccess ||
            !userResult.Value.IsServerAdmin)
        {
            context.Fail();
            return;
        }

        context.Succeed(requirement);
    }
}
