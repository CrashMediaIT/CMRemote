using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Mvc.Filters;
using Remotely.Server.Services;
using System.Net;

namespace Remotely.Server.Auth;

/// <summary>
/// Authorization filter for the MSI download endpoint
/// (<c>/api/uploaded-msi/{id}/download</c>). Reads the signed token from
/// the <c>X-CMRemote-Msi-Token</c> header, verifies it against
/// <see cref="ISignedMsiUrlService"/>, and rejects with 401 on any
/// failure (malformed, MAC failure, expired, mismatched device or file).
///
/// <para>The route's <c>{id}</c> parameter is used as the expected
/// <c>SharedFileId</c>. The expected device id is the
/// <c>DeviceId</c> embedded in the protected payload, so the filter
/// returns success only if the caller can produce a token minted for
/// the exact (device, file) pair the route names — and the device
/// binding is encoded inside the token, not pulled from a spoofable
/// header.</para>
/// </summary>
public class SignedMsiTokenFilter : IAsyncAuthorizationFilter
{
    /// <summary>
    /// HTTP header carrying the signed token. Distinct from the
    /// legacy <c>X-Expiring-Token</c> so the two paths never collide.
    /// </summary>
    public const string HeaderName = "X-CMRemote-Msi-Token";

    private readonly ISignedMsiUrlService _signedMsiUrlService;
    private readonly ILogger<SignedMsiTokenFilter> _logger;

    public SignedMsiTokenFilter(
        ISignedMsiUrlService signedMsiUrlService,
        ILogger<SignedMsiTokenFilter> logger)
    {
        _signedMsiUrlService = signedMsiUrlService;
        _logger = logger;
    }

    public Task OnAuthorizationAsync(AuthorizationFilterContext context)
    {
        var http = context.HttpContext;

        if (!http.Request.Headers.TryGetValue(HeaderName, out var tokenHeader) ||
            tokenHeader.Count == 0 ||
            string.IsNullOrWhiteSpace(tokenHeader[0]))
        {
            _logger.LogDebug("Signed MSI download rejected: missing {header}.", HeaderName);
            context.Result = new UnauthorizedResult();
            return Task.CompletedTask;
        }

        if (!context.RouteData.Values.TryGetValue("id", out var idObj) ||
            idObj is not string sharedFileId ||
            string.IsNullOrWhiteSpace(sharedFileId))
        {
            _logger.LogDebug("Signed MSI download rejected: route is missing the 'id' parameter.");
            context.Result = new BadRequestResult();
            return Task.CompletedTask;
        }

        var token = tokenHeader[0]!;
        var payload = _signedMsiUrlService.Validate(token, sharedFileId);
        if (payload is null)
        {
            context.Result = new UnauthorizedResult();
            return Task.CompletedTask;
        }

        // Surface the validated device id to downstream handlers (audit
        // log, metrics) so they don't have to re-validate.
        http.Items["CMRemote.SignedMsi.DeviceId"] = payload.DeviceId;
        http.Items["CMRemote.SignedMsi.SharedFileId"] = payload.SharedFileId;
        return Task.CompletedTask;
    }
}
