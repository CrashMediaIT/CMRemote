using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Services;

namespace Remotely.Server.Services;

/// <summary>
/// Owns the lifecycle of <see cref="PackageInstallJob"/> rows. Callers
/// queue jobs, mark them dispatched, and report final results — the
/// service enforces the legal state-machine transitions
/// (<see cref="PackageInstallJobStatus.Queued"/> →
/// <see cref="PackageInstallJobStatus.Running"/> → terminal).
///
/// <para>This is the only code path that mutates job rows; pages and
/// the hub MUST go through this service so the audit log and timestamp
/// invariants hold.</para>
/// </summary>
public interface IPackageInstallJobService
{
    Task<PackageInstallJob> QueueJobAsync(
        string organizationId,
        Guid packageId,
        string deviceId,
        PackageInstallAction action,
        Guid? bundleId,
        string? requestedByUserId);

    Task<IReadOnlyList<PackageInstallJob>> QueueBundleAsync(
        string organizationId,
        Guid bundleId,
        IReadOnlyList<string> deviceIds,
        string? requestedByUserId);

    Task<bool> MarkDispatchedAsync(Guid jobId);

    Task<bool> CompleteJobAsync(Guid jobId, PackageInstallResultDto result);

    Task<bool> CancelJobAsync(string organizationId, Guid jobId);

    Task<IReadOnlyList<PackageInstallJob>> GetRecentJobsForOrgAsync(string organizationId, int limit = 100);

    Task<PackageInstallJob?> GetJobAsync(string organizationId, Guid jobId);

    /// <summary>
    /// Pure state-machine predicate exposed for callers (and tests) so
    /// transition rules are a single source of truth.
    /// </summary>
    static bool IsLegalTransition(PackageInstallJobStatus from, PackageInstallJobStatus to)
    {
        if (from == to)
        {
            return false;
        }
        return from switch
        {
            PackageInstallJobStatus.Queued =>
                to is PackageInstallJobStatus.Running
                  or PackageInstallJobStatus.Cancelled,
            PackageInstallJobStatus.Running =>
                to is PackageInstallJobStatus.Success
                  or PackageInstallJobStatus.Failed
                  or PackageInstallJobStatus.Cancelled,
            // Terminal states cannot transition further.
            PackageInstallJobStatus.Success or
            PackageInstallJobStatus.Failed or
            PackageInstallJobStatus.Cancelled => false,
            _ => false,
        };
    }
}

public class PackageInstallJobService : IPackageInstallJobService
{
    private readonly IAppDbFactory _dbFactory;
    private readonly ISystemTime _systemTime;
    private readonly IPackageInstallJobRateLimiter _rateLimiter;
    private readonly ILogger<PackageInstallJobService> _logger;

    public PackageInstallJobService(
        IAppDbFactory dbFactory,
        ISystemTime systemTime,
        IPackageInstallJobRateLimiter rateLimiter,
        ILogger<PackageInstallJobService> logger)
    {
        _dbFactory = dbFactory;
        _systemTime = systemTime;
        _rateLimiter = rateLimiter;
        _logger = logger;
    }

    public async Task<PackageInstallJob> QueueJobAsync(
        string organizationId,
        Guid packageId,
        string deviceId,
        PackageInstallAction action,
        Guid? bundleId,
        string? requestedByUserId)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            throw new ArgumentException("Organization ID is required.", nameof(organizationId));
        }
        if (packageId == Guid.Empty)
        {
            throw new ArgumentException("Package ID is required.", nameof(packageId));
        }
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            throw new ArgumentException("Device ID is required.", nameof(deviceId));
        }

        // Track S / S7 — per-org rate limit. Refuse the queue if the
        // org has exceeded its sliding-window budget. The caller is
        // responsible for surfacing the failure to the operator
        // (CircuitConnection.QueueInstallPackage already returns a
        // Result<Guid>, which is what the toast layer reads).
        if (!await _rateLimiter.TryAcquireAsync(organizationId, CancellationToken.None))
        {
            throw new InvalidOperationException(
                "Package install-job rate limit exceeded for this organization. Try again shortly.");
        }

        using var db = _dbFactory.GetContext();

        // Cross-org reads are rejected here — a caller can only queue
        // a job for a package that exists in their own organization.
        var package = await db.Packages
            .AsNoTracking()
            .FirstOrDefaultAsync(p => p.Id == packageId && p.OrganizationID == organizationId)
            ?? throw new InvalidOperationException("Package not found in this organization.");

        var job = new PackageInstallJob
        {
            Id = Guid.NewGuid(),
            OrganizationID = organizationId,
            PackageId = packageId,
            DeploymentBundleId = bundleId,
            DeviceId = deviceId,
            Action = action,
            Status = PackageInstallJobStatus.Queued,
            CreatedAt = _systemTime.Now,
            RequestedByUserId = requestedByUserId,
        };

        db.PackageInstallJobs.Add(job);
        await db.SaveChangesAsync();

        _logger.LogInformation(
            "Package job queued. JobId={jobId} OrgId={orgId} PackageId={packageId} " +
            "DeviceId={deviceId} Action={action} BundleId={bundleId} ByUser={userId}",
            job.Id, organizationId, packageId, deviceId, action, bundleId, requestedByUserId);

        return job;
    }

    public async Task<IReadOnlyList<PackageInstallJob>> QueueBundleAsync(
        string organizationId,
        Guid bundleId,
        IReadOnlyList<string> deviceIds,
        string? requestedByUserId)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            throw new ArgumentException("Organization ID is required.", nameof(organizationId));
        }
        if (bundleId == Guid.Empty)
        {
            throw new ArgumentException("Bundle ID is required.", nameof(bundleId));
        }

        using var db = _dbFactory.GetContext();
        var bundle = await db.DeploymentBundles
            .AsNoTracking()
            .Include(b => b.Items.OrderBy(i => i.Order))
            .FirstOrDefaultAsync(b => b.Id == bundleId && b.OrganizationID == organizationId)
            ?? throw new InvalidOperationException("Bundle not found in this organization.");

        if (bundle.Items.Count == 0 || deviceIds.Count == 0)
        {
            return Array.Empty<PackageInstallJob>();
        }

        // Track S / S7 — per-org rate limit. The bundle path can fan
        // out to thousands of jobs in one call so we charge the limiter
        // up-front for every job we're about to insert; if the budget
        // doesn't cover the whole bundle we refuse the entire submission
        // rather than partially inserting.
        var totalJobs = bundle.Items.Count * deviceIds.Count(d => !string.IsNullOrWhiteSpace(d));
        if (totalJobs > 0 &&
            !await _rateLimiter.TryAcquireAsync(organizationId, totalJobs, CancellationToken.None))
        {
            throw new InvalidOperationException(
                $"Bundle would queue {totalJobs} jobs which exceeds the per-organization rate limit. " +
                "Try again shortly or split the bundle.");
        }

        var jobs = new List<PackageInstallJob>(bundle.Items.Count * deviceIds.Count);
        var now = _systemTime.Now;
        // Order matters: agents pull queued jobs in CreatedAt order, so
        // we tick through items in declared order to preserve sequencing.
        foreach (var deviceId in deviceIds)
        {
            if (string.IsNullOrWhiteSpace(deviceId))
            {
                continue;
            }
            foreach (var item in bundle.Items)
            {
                jobs.Add(new PackageInstallJob
                {
                    Id = Guid.NewGuid(),
                    OrganizationID = organizationId,
                    PackageId = item.PackageId,
                    DeploymentBundleId = bundleId,
                    DeviceId = deviceId,
                    Action = PackageInstallAction.Install,
                    Status = PackageInstallJobStatus.Queued,
                    CreatedAt = now,
                    RequestedByUserId = requestedByUserId,
                });
            }
        }

        db.PackageInstallJobs.AddRange(jobs);
        await db.SaveChangesAsync();

        _logger.LogInformation(
            "Bundle dispatched. BundleId={bundleId} OrgId={orgId} Devices={deviceCount} " +
            "Items={itemCount} JobsCreated={jobCount} ByUser={userId}",
            bundleId, organizationId, deviceIds.Count, bundle.Items.Count, jobs.Count, requestedByUserId);

        return jobs;
    }

    public async Task<bool> MarkDispatchedAsync(Guid jobId)
    {
        using var db = _dbFactory.GetContext();
        var job = await db.PackageInstallJobs.FirstOrDefaultAsync(j => j.Id == jobId);
        if (job is null)
        {
            return false;
        }
        if (!IPackageInstallJobService.IsLegalTransition(job.Status, PackageInstallJobStatus.Running))
        {
            _logger.LogWarning(
                "Refusing illegal transition. JobId={jobId} From={from} To=Running",
                jobId, job.Status);
            return false;
        }

        job.Status = PackageInstallJobStatus.Running;
        job.StartedAt = _systemTime.Now;
        await db.SaveChangesAsync();
        return true;
    }

    public async Task<bool> CompleteJobAsync(Guid jobId, PackageInstallResultDto result)
    {
        if (result is null)
        {
            return false;
        }

        using var db = _dbFactory.GetContext();
        var job = await db.PackageInstallJobs
            .Include(j => j.Result)
            .FirstOrDefaultAsync(j => j.Id == jobId);
        if (job is null)
        {
            return false;
        }

        var newStatus = result.Success
            ? PackageInstallJobStatus.Success
            : PackageInstallJobStatus.Failed;

        if (!IPackageInstallJobService.IsLegalTransition(job.Status, newStatus))
        {
            _logger.LogWarning(
                "Refusing illegal transition. JobId={jobId} From={from} To={to}",
                jobId, job.Status, newStatus);
            return false;
        }

        job.Status = newStatus;
        job.CompletedAt = _systemTime.Now;
        if (job.StartedAt is null)
        {
            // Reaching here means a result arrived for a job that was
            // never marked Running (only legal entry: agent reconnects
            // after a restart and reports a result for a previously
            // dispatched job whose dispatch ack we never observed).
            // We backfill StartedAt to CompletedAt so the row has valid
            // timestamps; the resulting "0 ms duration" is intentional
            // — the actual install time is in result.DurationMs.
            _logger.LogWarning(
                "Job result accepted without prior dispatch ack; backfilling StartedAt. " +
                "JobId={jobId}", jobId);
            job.StartedAt = job.CompletedAt;
        }
        // Result is written exactly once — write-then-no-overwrite via
        // the legal-transition guard above (terminal ⇒ no further moves).
        // Add explicitly so EF tracks the navigation as a new entity
        // rather than relying on dependent-detection through the
        // navigation property (which trips up the InMemory provider).
        if (job.Result is null)
        {
            job.Result = new PackageInstallResult
            {
                Id = Guid.NewGuid(),
                PackageInstallJobId = job.Id,
            };
            db.PackageInstallResults.Add(job.Result);
        }
        job.Result.Success = result.Success;
        job.Result.ExitCode = result.ExitCode;
        job.Result.DurationMs = result.DurationMs;
        job.Result.StdoutTail = Truncate(result.StdoutTail, 16 * 1024);
        job.Result.StderrTail = Truncate(result.StderrTail, 16 * 1024);
        job.Result.ErrorMessage = Truncate(result.ErrorMessage, 1024);

        await db.SaveChangesAsync();
        return true;
    }

    public async Task<bool> CancelJobAsync(string organizationId, Guid jobId)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return false;
        }

        using var db = _dbFactory.GetContext();
        var job = await db.PackageInstallJobs
            .FirstOrDefaultAsync(j => j.Id == jobId && j.OrganizationID == organizationId);
        if (job is null)
        {
            return false;
        }
        if (!IPackageInstallJobService.IsLegalTransition(job.Status, PackageInstallJobStatus.Cancelled))
        {
            return false;
        }

        job.Status = PackageInstallJobStatus.Cancelled;
        job.CompletedAt = _systemTime.Now;
        await db.SaveChangesAsync();
        return true;
    }

    public async Task<IReadOnlyList<PackageInstallJob>> GetRecentJobsForOrgAsync(string organizationId, int limit = 100)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Array.Empty<PackageInstallJob>();
        }

        if (limit <= 0)
        {
            limit = 100;
        }
        if (limit > 500)
        {
            limit = 500;
        }

        using var db = _dbFactory.GetContext();
        return await db.PackageInstallJobs
            .AsNoTracking()
            .Include(j => j.Package)
            .Include(j => j.Result)
            .Where(j => j.OrganizationID == organizationId)
            .OrderByDescending(j => j.CreatedAt)
            .Take(limit)
            .ToListAsync();
    }

    public async Task<PackageInstallJob?> GetJobAsync(string organizationId, Guid jobId)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || jobId == Guid.Empty)
        {
            return null;
        }

        using var db = _dbFactory.GetContext();
        return await db.PackageInstallJobs
            .AsNoTracking()
            .Include(j => j.Package)
            .Include(j => j.Result)
            .FirstOrDefaultAsync(j => j.Id == jobId && j.OrganizationID == organizationId);
    }

    private static string? Truncate(string? value, int max)
    {
        if (value is null)
        {
            return null;
        }
        return value.Length <= max ? value : value.Substring(value.Length - max);
    }
}
