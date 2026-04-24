using Microsoft.AspNetCore.Mvc;
using Remotely.Server.Auth;
using Remotely.Server.Services;

namespace Remotely.Server.API;

/// <summary>
/// Serves operator-uploaded MSI bytes for the agent over a signed,
/// short-lived URL (ROADMAP.md "Track S / S7 — Runtime security
/// posture: signed download URLs ... with a short TTL and a
/// device-scoped HMAC").
///
/// <para>Distinct from <see cref="FileSharingController"/>:</para>
/// <list type="bullet">
///   <item><see cref="FileSharingController"/> serves any shared file
///         to anyone with a valid <see cref="ExpiringTokenFilter"/>
///         token — the existing behaviour for the operator-driven file
///         transfer feature.</item>
///   <item>This controller serves only MSIs that the dispatcher has
///         minted a signed token for, gated by
///         <see cref="SignedMsiTokenFilter"/>. The token binds the
///         download to a specific (device, shared-file) pair, so a
///         leaked URL has bounded blast radius (it can be replayed by
///         that one device for that one MSI for the few minutes the
///         token is valid).</item>
/// </list>
///
/// <para>Both endpoints can co-exist during the R6 rollout: old agents
/// keep using <c>/api/filesharing/{id}</c>, new agents use
/// <c>/api/uploaded-msi/{id}/download</c>. Once R6 lands the
/// <c>FileSharingController.Get</c> path will be locked down to admin
/// users only (not agents).</para>
/// </summary>
[Route("api/uploaded-msi")]
[ApiController]
public class UploadedMsiDownloadController : ControllerBase
{
    private readonly IDataService _dataService;

    public UploadedMsiDownloadController(IDataService dataService)
    {
        _dataService = dataService;
    }

    [HttpGet("{id}/download")]
    [ServiceFilter(typeof(SignedMsiTokenFilter))]
    public async Task<IActionResult> Download(string id)
    {
        if (string.IsNullOrWhiteSpace(id))
        {
            return BadRequest();
        }

        var sharedFileResult = await _dataService.GetSharedFiled(id);
        if (!sharedFileResult.IsSuccess)
        {
            return NotFound();
        }

        var sharedFile = sharedFileResult.Value;
        var contentType = sharedFile.ContentType ?? "application/octet-stream";
        return File(sharedFile.FileContents, contentType, sharedFile.FileName);
    }
}
