using Microsoft.AspNetCore.Http;
using Remotely.Server.Services;

namespace Remotely.Server.Middleware;

/// <summary>
/// Redirects every browser request to <c>/setup</c> while the
/// <c>CMRemote.Setup.Completed</c> marker (see
/// <see cref="ISetupStateService"/>) is unset, so a freshly-installed
/// CMRemote v2 deployment lands on the first-boot wizard skeleton instead
/// of an empty Identity login form.
///
/// Allowlists framework / static / health endpoints so the wizard page
/// itself can render and so liveness probes never bounce. Once the marker
/// is written the middleware short-circuits to a no-op on every request.
/// </summary>
public class SetupRedirectMiddleware
{
    private readonly RequestDelegate _next;

    public SetupRedirectMiddleware(RequestDelegate next)
    {
        _next = next;
    }

    public async Task InvokeAsync(HttpContext context, ISetupStateService setupState)
    {
        if (await setupState.IsSetupCompletedAsync(context.RequestAborted))
        {
            await _next(context);
            return;
        }

        var path = context.Request.Path;

        if (IsAllowedWhileUnconfigured(path))
        {
            await _next(context);
            return;
        }

        // Preserve method semantics: only safe-method browser navigations
        // are redirected. POST/PUT/DELETE etc. get a 503 so partially-
        // upgraded clients don't silently drop state on an unconfigured
        // server.
        if (!HttpMethods.IsGet(context.Request.Method) &&
            !HttpMethods.IsHead(context.Request.Method))
        {
            context.Response.StatusCode = StatusCodes.Status503ServiceUnavailable;
            context.Response.Headers.RetryAfter = "30";
            await context.Response.WriteAsync(
                "CMRemote setup is not yet complete. Visit /setup in a browser to finish first-boot configuration.",
                context.RequestAborted);
            return;
        }

        context.Response.Redirect("/setup");
    }

    private static bool IsAllowedWhileUnconfigured(PathString path)
    {
        if (!path.HasValue)
        {
            return false;
        }

        // The wizard itself.
        if (path.StartsWithSegments("/setup", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        // Blazor / framework / static-asset paths needed by the wizard's
        // own server-rendered page.
        if (path.StartsWithSegments("/_blazor", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/_framework", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/_content", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/css", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/js", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/lib", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/images", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        // Health / liveness probes from container orchestrators must not
        // be redirected to a 302.
        if (path.StartsWithSegments("/health", StringComparison.OrdinalIgnoreCase) ||
            path.StartsWithSegments("/.well-known", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        // Common static assets served from wwwroot.
        var value = path.Value!;
        if (value.EndsWith(".ico", StringComparison.OrdinalIgnoreCase) ||
            value.EndsWith(".png", StringComparison.OrdinalIgnoreCase) ||
            value.EndsWith(".svg", StringComparison.OrdinalIgnoreCase) ||
            value.EndsWith(".webmanifest", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        return false;
    }
}
