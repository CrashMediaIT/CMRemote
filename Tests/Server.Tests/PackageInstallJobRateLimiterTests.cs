using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services;
using System;
using System.Threading;
using System.Threading.Tasks;
using MsOptions = Microsoft.Extensions.Options.Options;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests for <see cref="PackageInstallJobRateLimiter"/> — pins the
/// per-org budget semantics so the M3 dispatcher never gets flooded.
/// </summary>
[TestClass]
public class PackageInstallJobRateLimiterTests
{
    private static PackageInstallJobRateLimiter NewLimiter(int permits, int windowSeconds = 60)
    {
        var options = MsOptions.Create(new PackageInstallJobRateLimitOptions
        {
            PermitsPerWindow = permits,
            WindowSeconds = windowSeconds,
            SegmentsPerWindow = 1,
            QueueLimit = 0,
        });
        return new PackageInstallJobRateLimiter(options, NullLogger<PackageInstallJobRateLimiter>.Instance);
    }

    [TestMethod]
    public async Task TryAcquire_UnderBudget_ReturnsTrue()
    {
        using var limiter = NewLimiter(permits: 5);
        for (var i = 0; i < 5; i++)
        {
            Assert.IsTrue(await limiter.TryAcquireAsync("org1", 1, CancellationToken.None));
        }
    }

    [TestMethod]
    public async Task TryAcquire_OverBudget_ReturnsFalse()
    {
        using var limiter = NewLimiter(permits: 3);
        Assert.IsTrue(await limiter.TryAcquireAsync("org1", 1, CancellationToken.None));
        Assert.IsTrue(await limiter.TryAcquireAsync("org1", 1, CancellationToken.None));
        Assert.IsTrue(await limiter.TryAcquireAsync("org1", 1, CancellationToken.None));
        Assert.IsFalse(await limiter.TryAcquireAsync("org1", 1, CancellationToken.None));
    }

    [TestMethod]
    public async Task TryAcquire_PerOrgBudgetsAreIndependent()
    {
        using var limiter = NewLimiter(permits: 1);
        Assert.IsTrue(await limiter.TryAcquireAsync("orgA", 1, CancellationToken.None));
        Assert.IsFalse(await limiter.TryAcquireAsync("orgA", 1, CancellationToken.None));
        // orgB still has its full budget — exhausting one org must not
        // affect another.
        Assert.IsTrue(await limiter.TryAcquireAsync("orgB", 1, CancellationToken.None));
    }

    [TestMethod]
    public async Task TryAcquire_BulkRequestExceedingBudget_ReturnsFalse()
    {
        using var limiter = NewLimiter(permits: 5);
        Assert.IsFalse(await limiter.TryAcquireAsync("org1", 6, CancellationToken.None));
    }

    [TestMethod]
    public async Task TryAcquire_EmptyOrg_ReturnsFalse()
    {
        using var limiter = NewLimiter(permits: 5);
        Assert.IsFalse(await limiter.TryAcquireAsync("", 1, CancellationToken.None));
    }

    [TestMethod]
    public async Task TryAcquire_ZeroPermits_AlwaysTrue()
    {
        using var limiter = NewLimiter(permits: 1);
        Assert.IsTrue(await limiter.TryAcquireAsync("org1", 0, CancellationToken.None));
    }
}
