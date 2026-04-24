using Remotely.Shared.Entities;
using Remotely.Shared.Enums;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Owns every mutation of <see cref="AgentUpgradeStatus"/> rows for
/// the M3 background agent-upgrade pipeline (see ROADMAP.md "M3 —
/// Background agent-upgrade pipeline"). Callers MUST go through this
/// service so the legal-transition rules + retry/backoff math + 60-day
/// inactivity cut-off are enforced in one place.
///
/// <para>The service intentionally has no dependency on the actual
/// dispatch surface. Dispatch is delegated to
/// <see cref="IAgentUpgradeDispatcher"/> so the pipeline can be wired
/// against the legacy .NET installer surface today and re-pointed at
/// the Rust-agent installer (slice R8 / PR E) later without changing
/// the state machine.</para>
/// </summary>
public interface IAgentUpgradeService
{
    /// <summary>
    /// Inactivity cut-off used by <see cref="EnrolDeviceAsync"/> and the
    /// on-connect dispatch path. A device whose <c>LastOnline</c> is
    /// older than this is moved straight to
    /// <see cref="AgentUpgradeState.SkippedInactive"/> and not contacted;
    /// the row flips back to <see cref="AgentUpgradeState.Pending"/> on
    /// next reconnect (see <see cref="MarkDeviceCameOnlineAsync"/>).
    /// </summary>
    public static readonly TimeSpan InactivityCutoff = TimeSpan.FromDays(60);

    /// <summary>
    /// Maximum number of dispatch attempts before the row stays
    /// <see cref="AgentUpgradeState.Failed"/> instead of being requeued.
    /// </summary>
    public const int MaxAttempts = 5;

    /// <summary>
    /// Cap on the exponential-backoff retry delay.
    /// </summary>
    public static readonly TimeSpan MaxBackoff = TimeSpan.FromHours(24);

    /// <summary>
    /// Pure state-machine predicate. Exposed so callers + tests share one
    /// source of truth for legal transitions.
    /// </summary>
    static bool IsLegalTransition(AgentUpgradeState from, AgentUpgradeState to)
    {
        if (from == to)
        {
            return false;
        }
        return from switch
        {
            AgentUpgradeState.Pending => to is
                AgentUpgradeState.Scheduled
                or AgentUpgradeState.SkippedInactive
                or AgentUpgradeState.SkippedOptOut,
            AgentUpgradeState.Scheduled => to is
                AgentUpgradeState.InProgress
                or AgentUpgradeState.Pending
                or AgentUpgradeState.SkippedOptOut,
            AgentUpgradeState.InProgress => to is
                AgentUpgradeState.Succeeded
                or AgentUpgradeState.Failed,
            AgentUpgradeState.Failed => to is
                AgentUpgradeState.Pending
                or AgentUpgradeState.SkippedOptOut,
            AgentUpgradeState.SkippedInactive => to is
                AgentUpgradeState.Pending
                or AgentUpgradeState.SkippedOptOut,
            AgentUpgradeState.SkippedOptOut => to is
                AgentUpgradeState.Pending,
            AgentUpgradeState.Succeeded => to is
                AgentUpgradeState.Pending,
            _ => false,
        };
    }

    /// <summary>
    /// Pure backoff math. Returns the EligibleAt offset for the next
    /// attempt: <c>min(MaxBackoff, base * 2^(attemptCount-1))</c>.
    /// <paramref name="attemptCount"/> is the value AFTER the current
    /// failure has been recorded (i.e. 1 means "we just failed for the
    /// first time").
    /// </summary>
    static TimeSpan ComputeBackoff(int attemptCount)
    {
        if (attemptCount <= 0)
        {
            return TimeSpan.Zero;
        }
        // 1m, 2m, 4m, 8m, 16m … then capped at 24h. The first retry
        // therefore happens within a minute (which matches operator
        // expectation for a flaky transient failure) and the last retry
        // is spaced out by hours (so a permanently broken device
        // doesn't hammer the orchestrator).
        var seconds = 60.0 * Math.Pow(2, attemptCount - 1);
        if (double.IsInfinity(seconds) || seconds >= MaxBackoff.TotalSeconds)
        {
            return MaxBackoff;
        }
        return TimeSpan.FromSeconds(seconds);
    }

    /// <summary>
    /// Enrols a device into the upgrade pipeline. Idempotent — calling
    /// twice returns the existing row. If the device's last-online is
    /// older than <see cref="InactivityCutoff"/> at enrolment time the
    /// row is created in <see cref="AgentUpgradeState.SkippedInactive"/>.
    /// </summary>
    Task<AgentUpgradeStatus> EnrolDeviceAsync(
        string organizationId,
        string deviceId,
        string? fromVersion,
        DateTimeOffset deviceLastOnline,
        string? targetVersion,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Called from the AgentHub on-connect path. If the device's row is
    /// <see cref="AgentUpgradeState.SkippedInactive"/> and the device is
    /// now within the 60-day window again, the row is flipped back to
    /// <see cref="AgentUpgradeState.Pending"/> with EligibleAt=now so
    /// the orchestrator picks it up immediately. Returns the (possibly
    /// updated) row, or <c>null</c> if no row exists for the device.
    /// </summary>
    Task<AgentUpgradeStatus?> MarkDeviceCameOnlineAsync(
        string deviceId,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Returns up to <paramref name="limit"/> rows that are eligible for
    /// dispatch right now: state is <see cref="AgentUpgradeState.Pending"/>
    /// and <c>EligibleAt &lt;= now</c>. Ordered by <c>EligibleAt</c>
    /// ascending so the oldest-eligible work is dispatched first.
    /// </summary>
    Task<IReadOnlyList<AgentUpgradeStatus>> GetEligibleAsync(
        int limit,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Atomically reserves a row for dispatch by transitioning
    /// <see cref="AgentUpgradeState.Pending"/> →
    /// <see cref="AgentUpgradeState.Scheduled"/>. Returns <c>false</c> if
    /// the row no longer exists, the transition is illegal, or another
    /// orchestrator instance won the race.
    /// </summary>
    Task<bool> TryReserveAsync(Guid statusId, CancellationToken cancellationToken = default);

    /// <summary>
    /// Marks a reserved row as in-flight. Returns <c>false</c> if the
    /// transition is illegal.
    /// </summary>
    Task<bool> MarkInProgressAsync(Guid statusId, CancellationToken cancellationToken = default);

    /// <summary>
    /// Records a successful upgrade (terminal). Stamps
    /// <c>CompletedAt</c> and clears <c>LastAttemptError</c>.
    /// </summary>
    Task<bool> MarkSucceededAsync(Guid statusId, string installedVersion, CancellationToken cancellationToken = default);

    /// <summary>
    /// Records a failed attempt. If <c>AttemptCount + 1 &lt; MaxAttempts</c>
    /// the row is requeued (state = Pending, EligibleAt = now + backoff);
    /// otherwise the row stays in <see cref="AgentUpgradeState.Failed"/>
    /// and surfaces in the M4 dashboard.
    /// </summary>
    Task<bool> MarkFailedAsync(Guid statusId, string error, CancellationToken cancellationToken = default);

    /// <summary>
    /// Operator-facing "Retry" affordance from the M4 dashboard. Resets
    /// AttemptCount to zero and moves the row back to Pending /
    /// EligibleAt=now regardless of current state (except terminal
    /// Succeeded, which is also legal because publishing a new build
    /// re-arms the row).
    /// </summary>
    Task<bool> ForceRetryAsync(Guid statusId, CancellationToken cancellationToken = default);

    /// <summary>
    /// Operator-facing "Skip" affordance — pins the device to
    /// <see cref="AgentUpgradeState.SkippedOptOut"/>. Reversed by
    /// <see cref="ForceRetryAsync"/>.
    /// </summary>
    Task<bool> SetOptOutAsync(Guid statusId, CancellationToken cancellationToken = default);

    /// <summary>
    /// Refusal-while-busy rail: returns <c>true</c> when there is an
    /// active <see cref="PackageInstallJob"/> (Queued or Running) for
    /// the device. The orchestrator MUST refuse to dispatch the upgrade
    /// while this is true.
    /// </summary>
    Task<bool> HasInFlightJobAsync(string deviceId, CancellationToken cancellationToken = default);

    /// <summary>
    /// Aggregate counts for the M4 dashboard summary card. Returned in
    /// state-enum order so the dashboard can render a stable layout.
    /// </summary>
    Task<IReadOnlyDictionary<AgentUpgradeState, int>> GetStateCountsAsync(
        string organizationId,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Paged listing for the M4 dashboard table. Joins each
    /// <see cref="AgentUpgradeStatus"/> row with the matching
    /// <c>Device.DeviceName</c> / <c>Device.LastOnline</c> when one
    /// exists, scopes to <paramref name="organizationId"/>, and
    /// optionally filters by <paramref name="search"/> (case-insensitive
    /// substring match against <c>DeviceId</c> and <c>DeviceName</c>).
    /// Results are ordered by <c>CreatedAt</c> descending so the most
    /// recently enrolled rows surface first; pagination uses
    /// <paramref name="skip"/> / <paramref name="take"/>. A
    /// <paramref name="take"/> of zero or less returns an empty list.
    /// </summary>
    Task<IReadOnlyList<AgentUpgradeRow>> GetRowsForOrganizationAsync(
        string organizationId,
        string? search,
        int skip,
        int take,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Total row count for the same filter the dashboard's table uses,
    /// so the page can render pagination controls without reading every
    /// row. Mirrors the search semantics of
    /// <see cref="GetRowsForOrganizationAsync"/>.
    /// </summary>
    Task<int> CountRowsForOrganizationAsync(
        string organizationId,
        string? search,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Org-scoped overload of <see cref="ForceRetryAsync(Guid, CancellationToken)"/>
    /// for the M4 dashboard. Refuses (returns <c>false</c>) when the row
    /// does not exist or does not belong to <paramref name="organizationId"/>
    /// so an org-admin operator cannot reach into another organisation's
    /// rows by guessing a status id.
    /// </summary>
    Task<bool> ForceRetryAsync(
        Guid statusId,
        string organizationId,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Org-scoped overload of <see cref="SetOptOutAsync(Guid, CancellationToken)"/>
    /// for the M4 dashboard. Refuses (returns <c>false</c>) when the row
    /// does not exist or does not belong to <paramref name="organizationId"/>.
    /// </summary>
    Task<bool> SetOptOutAsync(
        Guid statusId,
        string organizationId,
        CancellationToken cancellationToken = default);
}
