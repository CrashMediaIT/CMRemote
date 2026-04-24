using Remotely.Shared.Entities;
using Remotely.Shared.Enums;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Read-only projection used by the M4 admin "Agent upgrade" dashboard
/// (see ROADMAP.md "M4 — Admin 'Agent upgrade' dashboard"). Joins
/// <see cref="AgentUpgradeStatus"/> with the device's current
/// <c>DeviceName</c> and <c>LastOnline</c> so the dashboard can show
/// the operator a name + last-online age without a second round trip
/// per row.
///
/// <para>Populated by
/// <see cref="IAgentUpgradeService.GetRowsForOrganizationAsync"/>.
/// All fields except <see cref="DeviceName"/> / <see cref="LastOnline"/>
/// come straight from the status row; the device-side fields are
/// nullable so a row whose underlying device record has been deleted
/// still surfaces in the dashboard rather than disappearing silently.
/// </para>
/// </summary>
public sealed record AgentUpgradeRow(
    Guid Id,
    string DeviceId,
    string OrganizationId,
    string? DeviceName,
    DateTimeOffset? LastOnline,
    string? FromVersion,
    string? ToVersion,
    AgentUpgradeState State,
    DateTimeOffset CreatedAt,
    DateTimeOffset EligibleAt,
    DateTimeOffset? LastAttemptAt,
    DateTimeOffset? CompletedAt,
    string? LastAttemptError,
    int AttemptCount);
