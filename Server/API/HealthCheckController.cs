using Microsoft.AspNetCore.Mvc;
using Remotely.Server.Auth;
using Remotely.Server.Services;
using Remotely.Server.Services.Organizations;

namespace Remotely.Server.API;

/// <summary>
/// Can only be accessed from the local machine.  The sole purpose
/// is to provide a healthcheck endpoint for Docker that exercises
/// the database connection.
/// </summary>
[Route("api/[controller]")]
[ApiController]
[ServiceFilter(typeof(LocalOnlyFilter))]
public class HealthCheckController : ControllerBase
{
    private readonly IOrganizationService _organizationService;

    public HealthCheckController(IOrganizationService organizationService)
    {
        _organizationService = organizationService;
    }

    [HttpGet]
    public async Task<IActionResult> Get()
    {
         _ = await _organizationService.GetOrganizationCountAsync();
        return NoContent();
    }
}
