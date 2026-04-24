using System.Collections.Concurrent;
using System.Threading.RateLimiting;
using Microsoft.Extensions.Options;

namespace Remotely.Server.Services;

/// <summary>
/// Configuration for <see cref="PackageInstallJobRateLimiter"/>. Bound
/// from the <c>PackageInstallJobs:RateLimit</c> configuration section.
/// </summary>
public class PackageInstallJobRateLimitOptions
{
    public const string SectionName = "PackageInstallJobs:RateLimit";

    /// <summary>
    /// Maximum jobs an organization may queue inside one
    /// <see cref="WindowSeconds"/> window. Default of 240 / minute
    /// matches the M3 retry/backoff math (5 in-flight × 60s sweep ×
    /// 0.8 padding) while still letting an operator queue a
    /// fleet-wide bundle in a few seconds for hundreds of devices.
    /// </summary>
    public int PermitsPerWindow { get; set; } = 240;

    /// <summary>
    /// Sliding-window size, in seconds.
    /// </summary>
    public int WindowSeconds { get; set; } = 60;

    /// <summary>
    /// Number of segments the sliding-window is divided into. Higher
    /// numbers give a smoother rate-limit at the cost of slightly more
    /// memory per org. The .NET-recommended default of 6 is fine.
    /// </summary>
    public int SegmentsPerWindow { get; set; } = 6;

    /// <summary>
    /// Number of permits an over-rate caller may queue waiting for a
    /// permit slot. Zero means "fail fast" (the default — the operator
    /// sees the rate-limit toast, retries themselves).
    /// </summary>
    public int QueueLimit { get; set; }
}

/// <summary>
/// Per-organization rate limit on package install-job queue submissions
/// (ROADMAP.md "Track S / S7 — Runtime security posture: per-org rate
/// limits on install-job dispatch"). Wraps every
/// <c>IPackageInstallJobService.QueueJobAsync</c> /
/// <c>QueueBundleAsync</c> call so a compromised admin account can't
/// queue a million jobs to flood the M3 dispatcher.
///
/// <para>Implemented with the standard
/// <see cref="System.Threading.RateLimiting.SlidingWindowRateLimiter"/>;
/// one limiter per organization, lazily created and held for the lifetime
/// of the process. The check is acquire-and-release: if the caller can
/// take a permit they're allowed to proceed and the permit is released
/// (we are using the limiter as a counter, not as a back-pressure source
/// for blocking).</para>
/// </summary>
public interface IPackageInstallJobRateLimiter
{
    /// <summary>
    /// Attempts to acquire <paramref name="permitCount"/> permits for
    /// <paramref name="organizationId"/>. Returns <c>true</c> on
    /// success; on failure the caller should reject the request and
    /// surface a 429-style toast to the operator.
    /// </summary>
    Task<bool> TryAcquireAsync(string organizationId, int permitCount, CancellationToken cancellationToken);

    /// <summary>
    /// Convenience wrapper around <see cref="TryAcquireAsync"/> for the
    /// common single-permit case.
    /// </summary>
    Task<bool> TryAcquireAsync(string organizationId, CancellationToken cancellationToken)
        => TryAcquireAsync(organizationId, 1, cancellationToken);
}

public class PackageInstallJobRateLimiter : IPackageInstallJobRateLimiter, IDisposable
{
    private readonly PackageInstallJobRateLimitOptions _options;
    private readonly ILogger<PackageInstallJobRateLimiter> _logger;
    private readonly ConcurrentDictionary<string, RateLimiter> _limiters =
        new(StringComparer.Ordinal);

    public PackageInstallJobRateLimiter(
        IOptions<PackageInstallJobRateLimitOptions> options,
        ILogger<PackageInstallJobRateLimiter> logger)
    {
        _options = options.Value;
        _logger = logger;
    }

    public async Task<bool> TryAcquireAsync(string organizationId, int permitCount, CancellationToken cancellationToken)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            // Defensive: an unauthenticated/null-org caller never has a
            // real org row so we refuse — the caller is the bug.
            return false;
        }
        if (permitCount <= 0)
        {
            return true;
        }

        var limiter = _limiters.GetOrAdd(organizationId, CreateLimiter);

        // SlidingWindowRateLimiter throws ArgumentOutOfRangeException when
        // a single AcquireAsync call asks for more permits than the
        // window's PermitLimit. For us, that's a legitimate business
        // outcome (the bundle is bigger than the per-window budget) so
        // we translate the throw into a refusal.
        if (permitCount > Math.Max(1, _options.PermitsPerWindow))
        {
            _logger.LogWarning(
                "Per-org install-job rate limit hit for OrgId={orgId} — request size {permits} exceeds the window budget {budget}.",
                organizationId, permitCount, _options.PermitsPerWindow);
            return false;
        }

        using var lease = await limiter.AcquireAsync(permitCount, cancellationToken);
        if (lease.IsAcquired)
        {
            return true;
        }

        _logger.LogWarning(
            "Per-org install-job rate limit hit for OrgId={orgId} (requested {permits} of {budget}/{window}s).",
            organizationId, permitCount, _options.PermitsPerWindow, _options.WindowSeconds);
        return false;
    }

    private RateLimiter CreateLimiter(string _)
    {
        return new SlidingWindowRateLimiter(new SlidingWindowRateLimiterOptions
        {
            PermitLimit = Math.Max(1, _options.PermitsPerWindow),
            Window = TimeSpan.FromSeconds(Math.Max(1, _options.WindowSeconds)),
            SegmentsPerWindow = Math.Max(1, _options.SegmentsPerWindow),
            QueueLimit = Math.Max(0, _options.QueueLimit),
            QueueProcessingOrder = QueueProcessingOrder.OldestFirst,
            AutoReplenishment = true,
        });
    }

    public void Dispose()
    {
        foreach (var limiter in _limiters.Values)
        {
            limiter.Dispose();
        }
        _limiters.Clear();
        GC.SuppressFinalize(this);
    }
}
