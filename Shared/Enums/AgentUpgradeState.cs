namespace Remotely.Shared.Enums;

/// <summary>
/// State machine for the M3 background agent-upgrade pipeline (see ROADMAP.md
/// "M3 — Background agent-upgrade pipeline"). One row per device tracked by
/// <c>AgentUpgradeStatus</c>; the orchestrator drives the device through this
/// enum.
///
/// <para>Legal transitions:
/// <list type="bullet">
///   <item><c>Pending → Scheduled → InProgress → (Succeeded | Failed)</c></item>
///   <item><c>Failed → Pending</c> (retry with exponential backoff, capped at 24h)</item>
///   <item><c>Pending → SkippedInactive</c> (LastOnline older than 60 days)</item>
///   <item><c>SkippedInactive → Pending</c> (device reconnected within window)</item>
///   <item><c>Pending → SkippedOptOut</c> / <c>SkippedOptOut → Pending</c> (operator toggle)</item>
/// </list>
/// Terminal <c>Succeeded</c> rows stay until the operator triggers another
/// upgrade cycle (e.g. a new build is published).</para>
/// </summary>
public enum AgentUpgradeState
{
    Pending = 0,
    Scheduled = 1,
    InProgress = 2,
    Succeeded = 3,
    Failed = 4,
    SkippedInactive = 5,
    SkippedOptOut = 6,
}
