using Remotely.Shared.Enums;
using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// One row per device tracked by the M3 background agent-upgrade
/// pipeline (see ROADMAP.md "M3 — Background agent-upgrade pipeline").
/// Owned by <c>IAgentUpgradeService</c> and driven by
/// <c>AgentUpgradeOrchestrator</c>; callers MUST go through the service
/// rather than mutating <see cref="State"/> / timestamps directly so the
/// state-machine invariants (legal transitions, timestamp stamping,
/// attempt accounting) hold.
///
/// <para>Devices are identified by their string id (snapshotted, like
/// <c>PackageInstallJob.DeviceId</c>) so a re-provisioned device that
/// returns under a different id is tracked as a new pipeline entry.</para>
/// </summary>
public class AgentUpgradeStatus
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    /// <summary>
    /// Target device id. There is at most one row per device id (enforced
    /// with a unique index in the EF model).
    /// </summary>
    [StringLength(128)]
    public string DeviceId { get; set; } = string.Empty;

    /// <summary>
    /// Org snapshot so the M4 dashboard can filter without joining
    /// <c>Devices</c>; the FK is intentionally not declared because a row
    /// must survive device deletion as historical state.
    /// </summary>
    [StringLength(128)]
    public string OrganizationID { get; set; } = string.Empty;

    /// <summary>
    /// The agent version the device was running when enrolment captured
    /// the row. Null only for the very first enrolment of a brand-new
    /// device that has not yet reported a version.
    /// </summary>
    [StringLength(64)]
    public string? FromVersion { get; set; }

    /// <summary>
    /// The agent version the orchestrator is moving the device to. Null
    /// when the row is freshly enrolled and a target manifest has not
    /// been resolved yet.
    /// </summary>
    [StringLength(64)]
    public string? ToVersion { get; set; }

    public AgentUpgradeState State { get; set; } = AgentUpgradeState.Pending;

    public DateTimeOffset CreatedAt { get; set; } = DateTimeOffset.UtcNow;

    /// <summary>
    /// Earliest UTC time at which the orchestrator may dispatch this
    /// row. Used to implement exponential-backoff retries (see
    /// ROADMAP.md M3 "Failure handling").
    /// </summary>
    public DateTimeOffset EligibleAt { get; set; } = DateTimeOffset.UtcNow;

    public DateTimeOffset? LastAttemptAt { get; set; }

    public DateTimeOffset? CompletedAt { get; set; }

    /// <summary>
    /// Tail of the most recent failure (capped on write to avoid
    /// runaway log rows). Null on success / never-attempted.
    /// </summary>
    [StringLength(2048)]
    public string? LastAttemptError { get; set; }

    public int AttemptCount { get; set; }
}
