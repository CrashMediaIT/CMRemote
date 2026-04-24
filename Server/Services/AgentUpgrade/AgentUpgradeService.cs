using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Services;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Default <see cref="IAgentUpgradeService"/> backed by EF Core. All
/// timestamp arithmetic flows through <see cref="ISystemTime"/> so
/// tests can drive the 60-day cut-off + retry/backoff math against a
/// virtual clock.
/// </summary>
public class AgentUpgradeService : IAgentUpgradeService
{
    private const int MaxErrorLength = 2048;

    private readonly IAppDbFactory _dbFactory;
    private readonly ISystemTime _systemTime;
    private readonly ILogger<AgentUpgradeService> _logger;

    public AgentUpgradeService(
        IAppDbFactory dbFactory,
        ISystemTime systemTime,
        ILogger<AgentUpgradeService> logger)
    {
        _dbFactory = dbFactory;
        _systemTime = systemTime;
        _logger = logger;
    }

    public async Task<AgentUpgradeStatus> EnrolDeviceAsync(
        string organizationId,
        string deviceId,
        string? fromVersion,
        DateTimeOffset deviceLastOnline,
        string? targetVersion,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            throw new ArgumentException("Organization ID is required.", nameof(organizationId));
        }
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            throw new ArgumentException("Device ID is required.", nameof(deviceId));
        }

        using var db = _dbFactory.GetContext();

        var existing = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.DeviceId == deviceId, cancellationToken);
        if (existing is not null)
        {
            return existing;
        }

        var now = _systemTime.Now;
        var inactive = (now - deviceLastOnline) > IAgentUpgradeService.InactivityCutoff;

        var row = new AgentUpgradeStatus
        {
            Id = Guid.NewGuid(),
            DeviceId = deviceId,
            OrganizationID = organizationId,
            FromVersion = Truncate(fromVersion, 64),
            ToVersion = Truncate(targetVersion, 64),
            State = inactive ? AgentUpgradeState.SkippedInactive : AgentUpgradeState.Pending,
            CreatedAt = now,
            EligibleAt = now,
            AttemptCount = 0,
        };
        db.AgentUpgradeStatuses.Add(row);
        await db.SaveChangesAsync(cancellationToken);

        _logger.LogInformation(
            "Agent-upgrade row enrolled. DeviceId={deviceId} OrgId={orgId} State={state} " +
            "FromVersion={from} ToVersion={to}",
            deviceId, organizationId, row.State, row.FromVersion, row.ToVersion);

        return row;
    }

    public async Task<AgentUpgradeStatus?> MarkDeviceCameOnlineAsync(
        string deviceId,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            return null;
        }

        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.DeviceId == deviceId, cancellationToken);
        if (row is null)
        {
            return null;
        }

        // SkippedInactive → Pending: device returned within the window, so
        // dispatch the instant we see it. Other states are left alone:
        //   - Pending / Failed: orchestrator will pick them up on its
        //     own cadence; nothing to do on connect.
        //   - SkippedOptOut: operator decision, do not override on connect.
        //   - Scheduled / InProgress: dispatch already in flight.
        //   - Succeeded: terminal until the next build is published.
        if (row.State == AgentUpgradeState.SkippedInactive)
        {
            if (!IAgentUpgradeService.IsLegalTransition(row.State, AgentUpgradeState.Pending))
            {
                return row;
            }
            row.State = AgentUpgradeState.Pending;
            row.EligibleAt = _systemTime.Now;
            row.LastAttemptError = null;
            await db.SaveChangesAsync(cancellationToken);
            _logger.LogInformation(
                "Agent-upgrade row reactivated on reconnect. DeviceId={deviceId}", deviceId);
        }
        return row;
    }

    public async Task<IReadOnlyList<AgentUpgradeStatus>> GetEligibleAsync(
        int limit,
        CancellationToken cancellationToken = default)
    {
        if (limit <= 0)
        {
            return Array.Empty<AgentUpgradeStatus>();
        }

        var now = _systemTime.Now;
        using var db = _dbFactory.GetContext();
        return await db.AgentUpgradeStatuses
            .AsNoTracking()
            .Where(x => x.State == AgentUpgradeState.Pending && x.EligibleAt <= now)
            .OrderBy(x => x.EligibleAt)
            .Take(limit)
            .ToListAsync(cancellationToken);
    }

    public async Task<bool> TryReserveAsync(Guid statusId, CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null)
        {
            return false;
        }
        if (!IAgentUpgradeService.IsLegalTransition(row.State, AgentUpgradeState.Scheduled))
        {
            return false;
        }
        // EligibleAt could have moved (e.g. operator forced a retry,
        // bumping it back); re-check here so the orchestrator's snapshot
        // can't reserve a row that's been pushed into the future.
        if (row.EligibleAt > _systemTime.Now)
        {
            return false;
        }
        row.State = AgentUpgradeState.Scheduled;
        try
        {
            await db.SaveChangesAsync(cancellationToken);
        }
        catch (DbUpdateConcurrencyException)
        {
            // Another orchestrator instance won. Treat as "didn't reserve".
            return false;
        }
        return true;
    }

    public async Task<bool> MarkInProgressAsync(Guid statusId, CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null)
        {
            return false;
        }
        if (!IAgentUpgradeService.IsLegalTransition(row.State, AgentUpgradeState.InProgress))
        {
            return false;
        }
        row.State = AgentUpgradeState.InProgress;
        row.LastAttemptAt = _systemTime.Now;
        row.AttemptCount += 1;
        await db.SaveChangesAsync(cancellationToken);
        return true;
    }

    public async Task<bool> MarkSucceededAsync(Guid statusId, string installedVersion, CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null)
        {
            return false;
        }
        if (!IAgentUpgradeService.IsLegalTransition(row.State, AgentUpgradeState.Succeeded))
        {
            return false;
        }
        var now = _systemTime.Now;
        row.State = AgentUpgradeState.Succeeded;
        row.CompletedAt = now;
        row.LastAttemptError = null;
        if (!string.IsNullOrWhiteSpace(installedVersion))
        {
            row.ToVersion = Truncate(installedVersion, 64);
        }
        await db.SaveChangesAsync(cancellationToken);
        _logger.LogInformation(
            "Agent-upgrade succeeded. DeviceId={deviceId} Version={version} Attempts={attempts}",
            row.DeviceId, row.ToVersion, row.AttemptCount);
        return true;
    }

    public async Task<bool> MarkFailedAsync(Guid statusId, string error, CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null)
        {
            return false;
        }
        if (!IAgentUpgradeService.IsLegalTransition(row.State, AgentUpgradeState.Failed))
        {
            return false;
        }
        var now = _systemTime.Now;
        row.LastAttemptError = Truncate(error, MaxErrorLength);
        row.CompletedAt = null;

        if (row.AttemptCount < IAgentUpgradeService.MaxAttempts)
        {
            // Requeue with backoff. The legal-transition table allows
            // Failed → Pending; we move InProgress → Failed → Pending in
            // a single SaveChanges so the row is never observably "stuck"
            // in Failed mid-retry.
            row.State = AgentUpgradeState.Failed;
            await db.SaveChangesAsync(cancellationToken);

            row.State = AgentUpgradeState.Pending;
            row.EligibleAt = now + IAgentUpgradeService.ComputeBackoff(row.AttemptCount);
            await db.SaveChangesAsync(cancellationToken);
            _logger.LogWarning(
                "Agent-upgrade attempt failed; requeued. DeviceId={deviceId} " +
                "Attempts={attempts} NextEligibleAt={eligibleAt}",
                row.DeviceId, row.AttemptCount, row.EligibleAt);
        }
        else
        {
            row.State = AgentUpgradeState.Failed;
            await db.SaveChangesAsync(cancellationToken);
            _logger.LogError(
                "Agent-upgrade attempts exhausted. DeviceId={deviceId} Attempts={attempts}",
                row.DeviceId, row.AttemptCount);
        }
        return true;
    }

    public async Task<bool> ForceRetryAsync(Guid statusId, CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null)
        {
            return false;
        }
        // Operator override — accept any non-Pending source state.
        row.State = AgentUpgradeState.Pending;
        row.EligibleAt = _systemTime.Now;
        row.AttemptCount = 0;
        row.LastAttemptError = null;
        row.CompletedAt = null;
        await db.SaveChangesAsync(cancellationToken);
        return true;
    }

    public async Task<bool> SetOptOutAsync(Guid statusId, CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null)
        {
            return false;
        }
        // Refuse if a dispatch is already mid-flight; let it terminate first.
        if (row.State is AgentUpgradeState.InProgress)
        {
            return false;
        }
        row.State = AgentUpgradeState.SkippedOptOut;
        await db.SaveChangesAsync(cancellationToken);
        return true;
    }

    public async Task<bool> HasInFlightJobAsync(string deviceId, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            return false;
        }
        using var db = _dbFactory.GetContext();
        return await db.PackageInstallJobs
            .AsNoTracking()
            .AnyAsync(j => j.DeviceId == deviceId &&
                (j.Status == PackageInstallJobStatus.Queued ||
                 j.Status == PackageInstallJobStatus.Running),
                cancellationToken);
    }

    public async Task<IReadOnlyDictionary<AgentUpgradeState, int>> GetStateCountsAsync(
        string organizationId,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return new Dictionary<AgentUpgradeState, int>();
        }
        using var db = _dbFactory.GetContext();
        var counts = await db.AgentUpgradeStatuses
            .AsNoTracking()
            .Where(x => x.OrganizationID == organizationId)
            .GroupBy(x => x.State)
            .Select(g => new { State = g.Key, Count = g.Count() })
            .ToListAsync(cancellationToken);

        var result = new Dictionary<AgentUpgradeState, int>();
        foreach (var state in Enum.GetValues<AgentUpgradeState>())
        {
            result[state] = 0;
        }
        foreach (var entry in counts)
        {
            result[entry.State] = entry.Count;
        }
        return result;
    }

    public async Task<IReadOnlyList<AgentUpgradeRow>> GetRowsForOrganizationAsync(
        string organizationId,
        string? search,
        int skip,
        int take,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || take <= 0)
        {
            return Array.Empty<AgentUpgradeRow>();
        }
        if (skip < 0)
        {
            skip = 0;
        }

        using var db = _dbFactory.GetContext();
        // Left-join Devices so a status row whose underlying device has
        // been deleted still surfaces in the dashboard with DeviceName /
        // LastOnline = null instead of disappearing silently.
        var query =
            from s in db.AgentUpgradeStatuses.AsNoTracking()
            where s.OrganizationID == organizationId
            join d in db.Devices.AsNoTracking() on s.DeviceId equals d.ID into deviceJoin
            from device in deviceJoin.DefaultIfEmpty()
            select new { Status = s, DeviceName = device != null ? device.DeviceName : null, LastOnline = device != null ? (DateTimeOffset?)device.LastOnline : null };

        if (!string.IsNullOrWhiteSpace(search))
        {
            // Case-insensitive substring match on DeviceId + DeviceName.
            // EF translates Contains() with StringComparison via provider
            // collation; we lower-case both sides ourselves so the same
            // expression works on SQLite (where collation is BINARY by
            // default), SQL Server, and PostgreSQL without a per-provider
            // branch.
            var needle = search.Trim().ToLowerInvariant();
            query = query.Where(x =>
                x.Status.DeviceId.ToLower().Contains(needle) ||
                (x.DeviceName != null && x.DeviceName.ToLower().Contains(needle)));
        }

        var page = await query
            .OrderByDescending(x => x.Status.CreatedAt)
            .ThenBy(x => x.Status.Id)
            .Skip(skip)
            .Take(take)
            .ToListAsync(cancellationToken);

        return page.Select(x => new AgentUpgradeRow(
            x.Status.Id,
            x.Status.DeviceId,
            x.Status.OrganizationID,
            x.DeviceName,
            x.LastOnline,
            x.Status.FromVersion,
            x.Status.ToVersion,
            x.Status.State,
            x.Status.CreatedAt,
            x.Status.EligibleAt,
            x.Status.LastAttemptAt,
            x.Status.CompletedAt,
            x.Status.LastAttemptError,
            x.Status.AttemptCount)).ToList();
    }

    public async Task<int> CountRowsForOrganizationAsync(
        string organizationId,
        string? search,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return 0;
        }
        using var db = _dbFactory.GetContext();

        if (string.IsNullOrWhiteSpace(search))
        {
            return await db.AgentUpgradeStatuses
                .AsNoTracking()
                .Where(x => x.OrganizationID == organizationId)
                .CountAsync(cancellationToken);
        }

        var needle = search.Trim().ToLowerInvariant();
        // Mirror the join + filter shape used by GetRowsForOrganizationAsync
        // so the count and the page never disagree.
        var query =
            from s in db.AgentUpgradeStatuses.AsNoTracking()
            where s.OrganizationID == organizationId
            join d in db.Devices.AsNoTracking() on s.DeviceId equals d.ID into deviceJoin
            from device in deviceJoin.DefaultIfEmpty()
            where s.DeviceId.ToLower().Contains(needle) ||
                  (device != null && device.DeviceName != null && device.DeviceName.ToLower().Contains(needle))
            select s.Id;

        return await query.CountAsync(cancellationToken);
    }

    public async Task<bool> ForceRetryAsync(
        Guid statusId,
        string organizationId,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return false;
        }
        // Org-scope check is performed against the same context the
        // mutation runs in so the operator cannot race a row out of
        // their org between the check and the write.
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null || row.OrganizationID != organizationId)
        {
            return false;
        }
        row.State = AgentUpgradeState.Pending;
        row.EligibleAt = _systemTime.Now;
        row.AttemptCount = 0;
        row.LastAttemptError = null;
        row.CompletedAt = null;
        await db.SaveChangesAsync(cancellationToken);
        _logger.LogInformation(
            "Agent-upgrade row force-retried by operator. DeviceId={deviceId} OrgId={orgId}",
            row.DeviceId, organizationId);
        return true;
    }

    public async Task<bool> SetOptOutAsync(
        Guid statusId,
        string organizationId,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return false;
        }
        using var db = _dbFactory.GetContext();
        var row = await db.AgentUpgradeStatuses
            .FirstOrDefaultAsync(x => x.Id == statusId, cancellationToken);
        if (row is null || row.OrganizationID != organizationId)
        {
            return false;
        }
        // Same refusal-while-busy rail the org-less overload uses; let
        // the in-flight dispatch terminate first.
        if (row.State is AgentUpgradeState.InProgress)
        {
            return false;
        }
        row.State = AgentUpgradeState.SkippedOptOut;
        await db.SaveChangesAsync(cancellationToken);
        _logger.LogInformation(
            "Agent-upgrade row opted-out by operator. DeviceId={deviceId} OrgId={orgId}",
            row.DeviceId, organizationId);
        return true;
    }

    private static string? Truncate(string? value, int maxLength)
    {
        if (value is null)
        {
            return null;
        }
        if (value.Length <= maxLength)
        {
            return value;
        }
        return value.Substring(0, maxLength);
    }
}
