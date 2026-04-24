using Microsoft.AspNetCore.Http;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Middleware;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests for <see cref="SecurityHeadersMiddleware"/> — pins the
/// default-route headers, the Viewer opt-in, and the no-overwrite
/// composition contract.
/// </summary>
[TestClass]
public class SecurityHeadersMiddlewareTests
{
    private static async Task<HttpContext> Run(string path, System.Action<HttpContext>? preInvoke = null)
    {
        var context = new DefaultHttpContext();
        context.Request.Path = path;
        preInvoke?.Invoke(context);

        var middleware = new SecurityHeadersMiddleware(_ => Task.CompletedTask);
        await middleware.InvokeAsync(context);
        return context;
    }

    [TestMethod]
    public async Task DefaultRoute_SetsAllBaselineHeaders()
    {
        var ctx = await Run("/Account/Devices");
        var h = ctx.Response.Headers;
        Assert.AreEqual(SecurityHeadersMiddleware.DefaultContentSecurityPolicy, h["Content-Security-Policy"]);
        Assert.AreEqual("nosniff", h["X-Content-Type-Options"]);
        Assert.AreEqual("strict-origin-when-cross-origin", h["Referrer-Policy"]);
        Assert.AreEqual(SecurityHeadersMiddleware.DefaultPermissionsPolicy, h["Permissions-Policy"]);
        Assert.AreEqual("DENY", h["X-Frame-Options"]);
        Assert.AreEqual("same-origin", h["Cross-Origin-Opener-Policy"]);
        Assert.AreEqual("same-origin", h["Cross-Origin-Resource-Policy"]);
    }

    [TestMethod]
    public async Task DefaultRoute_PermissionsPolicyDeniesCameraAndMic()
    {
        var ctx = await Run("/Account/Devices");
        var pp = (string)ctx.Response.Headers["Permissions-Policy"]!;
        StringAssert.Contains(pp, "camera=()");
        StringAssert.Contains(pp, "microphone=()");
        StringAssert.Contains(pp, "geolocation=()");
    }

    [TestMethod]
    public async Task ViewerRoute_OptsInToWebRtcPermissions()
    {
        var ctx = await Run("/Viewer");
        var pp = (string)ctx.Response.Headers["Permissions-Policy"]!;
        StringAssert.Contains(pp, "camera=(self)");
        StringAssert.Contains(pp, "microphone=(self)");
        StringAssert.Contains(pp, "display-capture=(self)");
    }

    [TestMethod]
    public async Task ViewerRoute_PermissionsPolicyStillDeniesGeolocation()
    {
        var ctx = await Run("/Viewer");
        var pp = (string)ctx.Response.Headers["Permissions-Policy"]!;
        // The Viewer doesn't need geolocation; only the WebRTC capture
        // surfaces should be opted in.
        StringAssert.Contains(pp, "geolocation=()");
    }

    [TestMethod]
    public async Task ExistingHeader_IsPreserved()
    {
        var ctx = await Run("/Account/Devices", c =>
        {
            c.Response.Headers["Content-Security-Policy"] = "default-src 'none'";
        });
        Assert.AreEqual("default-src 'none'", ctx.Response.Headers["Content-Security-Policy"]);
    }

    [TestMethod]
    public async Task CspContainsFrameAncestorsNone()
    {
        var ctx = await Run("/Account/Devices");
        var csp = (string)ctx.Response.Headers["Content-Security-Policy"]!;
        StringAssert.Contains(csp, "frame-ancestors 'none'");
    }
}
