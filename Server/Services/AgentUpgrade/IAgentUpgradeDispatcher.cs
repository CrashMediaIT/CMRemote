using Remotely.Shared.Entities;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Pluggable dispatch surface for the M3 agent-upgrade pipeline. The
/// orchestrator is intentionally decoupled from the actual installer
/// implementation: the legacy .NET agent ships a templated PowerShell
/// installer (PR E pre-rewrite), the Rust agent ships a signed MSI /
/// `.deb` / `.rpm` / notarized `.pkg` (slice R8). The state machine in
/// <see cref="IAgentUpgradeService"/> is shared across both.
///
/// <para>The default registration is <see cref="NoopAgentUpgradeDispatcher"/>
/// (logs and returns "not available"); the real dispatcher is wired in
/// once the publisher manifest + signed-build fetch land. Wiring the
/// state machine and orchestrator first means they can be exercised
/// end-to-end against the no-op dispatcher today and pointed at the
/// real installer surface later without re-touching the schema or the
/// retry/backoff math.</para>
/// </summary>
public interface IAgentUpgradeDispatcher
{
    /// <summary>
    /// Resolves the agent build the device should be moved to. Returns
    /// <c>null</c> if no upgrade is currently published (e.g. the build
    /// channel is empty or the device is already on the target version).
    /// </summary>
    Task<AgentUpgradeTarget?> ResolveTargetAsync(
        AgentUpgradeStatus status,
        CancellationToken cancellationToken);

    /// <summary>
    /// Dispatches the upgrade to the device. Implementations MUST verify
    /// the target build's SHA-256 / signature against the publisher
    /// manifest before returning success. Implementations MUST NOT
    /// mutate the <see cref="AgentUpgradeStatus"/> row directly — the
    /// orchestrator owns state transitions through
    /// <see cref="IAgentUpgradeService"/>.
    /// </summary>
    Task<AgentUpgradeDispatchResult> DispatchAsync(
        AgentUpgradeStatus status,
        AgentUpgradeTarget target,
        CancellationToken cancellationToken);
}

/// <summary>
/// Resolved target build the orchestrator wants the device to land on.
/// </summary>
public record AgentUpgradeTarget(string Version, string Sha256, Uri DownloadUri);

/// <summary>
/// Outcome of a single dispatch attempt. <see cref="Succeeded"/> is set
/// when the device sends a heartbeat tagged with the new version (the
/// dispatcher implementation is responsible for awaiting that
/// confirmation, with a sensible per-OS timeout).
/// </summary>
public record AgentUpgradeDispatchResult(bool Succeeded, string? Error)
{
    public static AgentUpgradeDispatchResult Ok() => new(true, null);
    public static AgentUpgradeDispatchResult Fail(string error) => new(false, error);
}

/// <summary>
/// Default dispatcher used until the publisher manifest + signed-build
/// pipeline (slice R6 / R8) is wired. Returns "no target available" so
/// the orchestrator simply leaves rows in <see cref="Shared.Enums.AgentUpgradeState.Pending"/>
/// and emits a single informational log per device per sweep. This lets
/// the rest of the M3 pipeline — schema, state machine, on-connect
/// reactivation — ship and be tested end-to-end before the installer
/// rewrite catches up.
/// </summary>
public class NoopAgentUpgradeDispatcher : IAgentUpgradeDispatcher
{
    private readonly ILogger<NoopAgentUpgradeDispatcher> _logger;

    public NoopAgentUpgradeDispatcher(ILogger<NoopAgentUpgradeDispatcher> logger)
    {
        _logger = logger;
    }

    public Task<AgentUpgradeTarget?> ResolveTargetAsync(
        AgentUpgradeStatus status,
        CancellationToken cancellationToken)
    {
        _logger.LogDebug(
            "Agent-upgrade dispatcher is the no-op default; leaving DeviceId={deviceId} pending.",
            status.DeviceId);
        return Task.FromResult<AgentUpgradeTarget?>(null);
    }

    public Task<AgentUpgradeDispatchResult> DispatchAsync(
        AgentUpgradeStatus status,
        AgentUpgradeTarget target,
        CancellationToken cancellationToken)
    {
        // Should never be reached because ResolveTargetAsync returns null,
        // but kept defensive so a misconfigured composition cannot
        // silently flip a row to Failed.
        return Task.FromResult(AgentUpgradeDispatchResult.Fail(
            "Default no-op dispatcher cannot perform real upgrades."));
    }
}
