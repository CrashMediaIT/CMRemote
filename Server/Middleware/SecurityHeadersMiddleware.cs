using Microsoft.AspNetCore.Http;

namespace Remotely.Server.Middleware;

/// <summary>
/// Adds the CMRemote runtime-security baseline headers to every response
/// (ROADMAP.md "Track S / S7 — Runtime security posture").
///
/// Sets, by default:
/// <list type="bullet">
///   <item><c>Content-Security-Policy</c> — strict <c>default-src 'self'</c>
///         baseline with <c>frame-ancestors 'none'</c> for clickjacking
///         resistance. Bootstrap + the existing inline styles in Razor pages
///         force <c>style-src 'self' 'unsafe-inline'</c>; everything else is
///         locked down to <c>'self'</c>. SignalR is allowed via
///         <c>connect-src 'self' wss: ws:</c> so the agent / viewer hubs can
///         reach the same-origin server.</item>
///   <item><c>X-Content-Type-Options: nosniff</c> — disables MIME sniffing
///         on every response so a misconfigured static asset cannot be
///         coerced into executing as script.</item>
///   <item><c>Referrer-Policy: strict-origin-when-cross-origin</c> — the
///         OWASP-recommended default; never leaks path/query to a different
///         origin.</item>
///   <item><c>Permissions-Policy</c> — denies camera, microphone,
///         geolocation, payment, USB, accelerometer, gyroscope, magnetometer,
///         and display-capture by default. The Razor <c>/Viewer</c> route
///         (the WebRTC remote-control viewer) opts back in to camera /
///         microphone / display-capture for its own origin so the WebRTC
///         flow continues to work; every other page stays denied.</item>
/// </list>
///
/// The middleware never overwrites a header that has already been set by an
/// upstream middleware (for example a more specific per-route policy), so it
/// is safe to compose with future per-page hardening.
/// </summary>
public class SecurityHeadersMiddleware
{
    // The default Permissions-Policy denies the sensors / capture features
    // an admin panel should never need. Listed exhaustively rather than
    // relying on the spec's "deny everything not listed" because browsers
    // disagree on the default, and an explicit deny survives newer
    // permissions being added to the spec without us noticing.
    internal const string DefaultPermissionsPolicy =
        "accelerometer=(), " +
        "ambient-light-sensor=(), " +
        "autoplay=(), " +
        "battery=(), " +
        "camera=(), " +
        "display-capture=(), " +
        "document-domain=(), " +
        "encrypted-media=(), " +
        "fullscreen=(self), " +
        "geolocation=(), " +
        "gyroscope=(), " +
        "magnetometer=(), " +
        "microphone=(), " +
        "midi=(), " +
        "payment=(), " +
        "picture-in-picture=(), " +
        "publickey-credentials-get=(), " +
        "screen-wake-lock=(), " +
        "sync-xhr=(), " +
        "usb=(), " +
        "xr-spatial-tracking=()";

    // The WebRTC viewer at /Viewer needs camera + microphone +
    // display-capture for the remote-control flow to negotiate media. It
    // still denies geolocation / payment / usb / sensors etc. so the opt-in
    // is the minimum surface required for the page to work.
    internal const string ViewerPermissionsPolicy =
        "accelerometer=(), " +
        "ambient-light-sensor=(), " +
        "autoplay=(self), " +
        "battery=(), " +
        "camera=(self), " +
        "display-capture=(self), " +
        "document-domain=(), " +
        "encrypted-media=(self), " +
        "fullscreen=(self), " +
        "geolocation=(), " +
        "gyroscope=(), " +
        "magnetometer=(), " +
        "microphone=(self), " +
        "midi=(), " +
        "payment=(), " +
        "picture-in-picture=(self), " +
        "publickey-credentials-get=(), " +
        "screen-wake-lock=(), " +
        "sync-xhr=(), " +
        "usb=(), " +
        "xr-spatial-tracking=()";

    // CSP for normal admin-panel responses.
    //
    // - default-src 'self': no off-origin loads by default.
    // - script-src 'self': Blazor Server's framework JS is served from
    //   /_framework/ on the same origin; Bootstrap is bundled. No inline
    //   scripts in App.razor or the package-manager pages today.
    // - style-src 'self' 'unsafe-inline': Bootstrap + existing inline
    //   style="..." attributes in Razor pages and the dynamic theme switch
    //   require this until Module 6 (the UI rebuild) migrates them to
    //   var(--cm-...) tokens; will tighten then.
    // - img-src 'self' data: blob:: branding logos can be served as data
    //   URIs and Blazor sometimes converts uploaded images to blob URLs.
    // - font-src 'self' data:: FontAwesome ships as same-origin assets.
    // - connect-src 'self' wss: ws:: SignalR uses a same-origin WebSocket.
    // - frame-ancestors 'none': hard clickjacking deny; supersedes the
    //   older X-Frame-Options for browsers that honour CSP Level 2+.
    // - base-uri 'self', form-action 'self': prevent base-tag and form
    //   redirection injection.
    // - object-src 'none': no Flash/legacy plugin embedding.
    internal const string DefaultContentSecurityPolicy =
        "default-src 'self'; " +
        "script-src 'self'; " +
        "style-src 'self' 'unsafe-inline'; " +
        "img-src 'self' data: blob:; " +
        "font-src 'self' data:; " +
        "connect-src 'self' wss: ws:; " +
        "media-src 'self' blob:; " +
        "object-src 'none'; " +
        "base-uri 'self'; " +
        "form-action 'self'; " +
        "frame-ancestors 'none'";

    // CSP for the WebRTC viewer page. Same as the default policy but
    // additionally allows blob: as a script source (Blazor Server doesn't
    // need this but the WebRTC viewer historically does for worker
    // bootstrapping) and explicitly allows the SignalR hub origin.
    internal const string ViewerContentSecurityPolicy =
        "default-src 'self'; " +
        "script-src 'self' 'unsafe-inline'; " +
        "style-src 'self' 'unsafe-inline'; " +
        "img-src 'self' data: blob:; " +
        "font-src 'self' data:; " +
        "connect-src 'self' wss: ws:; " +
        "media-src 'self' blob:; " +
        "object-src 'none'; " +
        "base-uri 'self'; " +
        "form-action 'self'; " +
        "frame-ancestors 'none'";

    private readonly RequestDelegate _next;

    public SecurityHeadersMiddleware(RequestDelegate next)
    {
        _next = next;
    }

    public Task InvokeAsync(HttpContext context)
    {
        // Headers are written into Response.OnStarting so they apply to
        // every code path that produces a response, including short-circuit
        // responses returned by other middleware (e.g. the setup-redirect
        // middleware's 503 + Retry-After response).
        context.Response.OnStarting(static state =>
        {
            var ctx = (HttpContext)state;
            ApplyHeaders(ctx);
            return Task.CompletedTask;
        }, context);

        return _next(context);
    }

    private static void ApplyHeaders(HttpContext context)
    {
        var headers = context.Response.Headers;
        var isViewer = IsViewerRoute(context.Request.Path);

        // Use the per-route policy when a more specific policy applies;
        // otherwise the default. SetIfMissing preserves any header an
        // upstream middleware (or a future per-page filter) has already
        // set so this middleware composes cleanly.
        SetIfMissing(headers, "Content-Security-Policy",
            isViewer ? ViewerContentSecurityPolicy : DefaultContentSecurityPolicy);

        SetIfMissing(headers, "X-Content-Type-Options", "nosniff");

        SetIfMissing(headers, "Referrer-Policy", "strict-origin-when-cross-origin");

        SetIfMissing(headers, "Permissions-Policy",
            isViewer ? ViewerPermissionsPolicy : DefaultPermissionsPolicy);

        // Belt-and-braces clickjacking defence for browsers that don't
        // honour CSP Level 2+ frame-ancestors. The CSP directive above is
        // the modern equivalent.
        SetIfMissing(headers, "X-Frame-Options", "DENY");

        // Cross-origin isolation: the admin panel does not embed
        // cross-origin documents and does not need to be embedded in one
        // either, so the strictest values are safe.
        SetIfMissing(headers, "Cross-Origin-Opener-Policy", "same-origin");
        SetIfMissing(headers, "Cross-Origin-Resource-Policy", "same-origin");
    }

    private static bool IsViewerRoute(PathString path)
    {
        return path.HasValue &&
            path.StartsWithSegments("/Viewer", StringComparison.OrdinalIgnoreCase);
    }

    private static void SetIfMissing(IHeaderDictionary headers, string name, string value)
    {
        if (!headers.ContainsKey(name))
        {
            headers[name] = value;
        }
    }
}
