using Microsoft.Extensions.Options;
using Remotely.Shared.Utilities;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Tunable knobs for the M3 background agent-upgrade pipeline.
/// Defaults match the ROADMAP.md M3 spec ("bounded-concurrency queue
/// (default 5 in flight per server, tunable)").
/// </summary>
public class AgentUpgradeOrchestratorOptions
{
    /// <summary>Configuration section: <c>AgentUpgrade</c>.</summary>
    public const string SectionName = "AgentUpgrade";

    /// <summary>Maximum number of concurrent dispatches per server instance.</summary>
    public int MaxConcurrency { get; set; } = 5;

    /// <summary>How often the orchestrator polls for newly-eligible work.</summary>
    public TimeSpan SweepInterval { get; set; } = TimeSpan.FromMinutes(1);

    /// <summary>Hard cap on how many rows a single sweep will pull off the queue.</summary>
    public int SweepBatchSize { get; set; } = 50;

    /// <summary>
    /// Per-device dispatch timeout. The dispatcher is responsible for
    /// observing the new heartbeat; if it doesn't return within this
    /// window the attempt is recorded as a failure (and requeued with
    /// backoff per the standard rules).
    /// </summary>
    public TimeSpan DispatchTimeout { get; set; } = TimeSpan.FromMinutes(15);
}

/// <summary>
/// <see cref="IHostedService"/> that drives the M3 background
/// agent-upgrade pipeline (see ROADMAP.md "M3 — Background agent-upgrade
/// pipeline"). On a configurable cadence it sweeps for eligible
/// <see cref="Remotely.Shared.Entities.AgentUpgradeStatus"/> rows,
/// dispatches up to <see cref="AgentUpgradeOrchestratorOptions.MaxConcurrency"/>
/// upgrades concurrently through <see cref="IAgentUpgradeDispatcher"/>,
/// and feeds the results back through <see cref="IAgentUpgradeService"/>
/// so the state machine + retry/backoff math live in one place.
///
/// <para>The orchestrator deliberately holds NO mutable per-device state
/// of its own; everything observable is in the database. That makes
/// horizontal scaling trivial (the <c>TryReserveAsync</c> race in the
/// service layer is the only coordination point) and lets a restart
/// pick up exactly where the previous instance left off.</para>
/// </summary>
public class AgentUpgradeOrchestrator : IHostedService, IDisposable
{
    private readonly IServiceProvider _serviceProvider;
    private readonly AgentUpgradeOrchestratorOptions _options;
    private readonly ILogger<AgentUpgradeOrchestrator> _logger;
    private readonly TimeSpan _debugInterval = TimeSpan.FromSeconds(15);

    private CancellationTokenSource? _shutdown;
    private Task? _sweepLoop;

    public AgentUpgradeOrchestrator(
        IServiceProvider serviceProvider,
        IOptions<AgentUpgradeOrchestratorOptions> options,
        ILogger<AgentUpgradeOrchestrator> logger)
    {
        _serviceProvider = serviceProvider;
        _options = options.Value;
        _logger = logger;
    }

    public Task StartAsync(CancellationToken cancellationToken)
    {
        _shutdown = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        _sweepLoop = Task.Run(() => RunAsync(_shutdown.Token), _shutdown.Token);
        return Task.CompletedTask;
    }

    public async Task StopAsync(CancellationToken cancellationToken)
    {
        if (_shutdown is null)
        {
            return;
        }
        try
        {
            _shutdown.Cancel();
        }
        catch (ObjectDisposedException)
        {
            // already disposed
        }
        if (_sweepLoop is not null)
        {
            try
            {
                await _sweepLoop.WaitAsync(cancellationToken);
            }
            catch (OperationCanceledException) { }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Agent-upgrade orchestrator sweep loop ended with an error.");
            }
        }
    }

    public void Dispose()
    {
        _shutdown?.Dispose();
        _shutdown = null;
        GC.SuppressFinalize(this);
    }

    private async Task RunAsync(CancellationToken cancellationToken)
    {
        var interval = EnvironmentHelper.IsDebug ? _debugInterval : _options.SweepInterval;
        _logger.LogInformation(
            "Agent-upgrade orchestrator started. Sweep={interval} MaxConcurrency={concurrency} BatchSize={batch}",
            interval, _options.MaxConcurrency, _options.SweepBatchSize);

        while (!cancellationToken.IsCancellationRequested)
        {
            try
            {
                await SweepOnceAsync(cancellationToken);
            }
            catch (OperationCanceledException)
            {
                break;
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Agent-upgrade orchestrator sweep failed; will retry on next interval.");
            }

            try
            {
                await Task.Delay(interval, cancellationToken);
            }
            catch (OperationCanceledException)
            {
                break;
            }
        }
    }

    /// <summary>
    /// Pulls a batch of eligible work and dispatches it with bounded
    /// concurrency. Exposed as <c>internal</c> so deterministic tests can
    /// drive a single sweep without spinning the timer.
    /// </summary>
    internal async Task SweepOnceAsync(CancellationToken cancellationToken)
    {
        using var scope = _serviceProvider.CreateScope();
        var service = scope.ServiceProvider.GetRequiredService<IAgentUpgradeService>();
        var dispatcher = scope.ServiceProvider.GetRequiredService<IAgentUpgradeDispatcher>();

        var batch = await service.GetEligibleAsync(_options.SweepBatchSize, cancellationToken);
        if (batch.Count == 0)
        {
            return;
        }

        _logger.LogDebug("Agent-upgrade sweep: {count} eligible row(s).", batch.Count);

        using var gate = new SemaphoreSlim(Math.Max(1, _options.MaxConcurrency));
        var tasks = new List<Task>(batch.Count);
        foreach (var row in batch)
        {
            await gate.WaitAsync(cancellationToken);
            tasks.Add(Task.Run(async () =>
            {
                try
                {
                    await ProcessOneAsync(row.Id, cancellationToken);
                }
                finally
                {
                    gate.Release();
                }
            }, cancellationToken));
        }
        await Task.WhenAll(tasks);
    }

    private async Task ProcessOneAsync(Guid statusId, CancellationToken cancellationToken)
    {
        // New scope per row so the EF context lifetime tracks the unit
        // of work; matches the pattern in ScriptScheduler /
        // RemoteControlSessionCleaner.
        using var scope = _serviceProvider.CreateScope();
        var service = scope.ServiceProvider.GetRequiredService<IAgentUpgradeService>();
        var dispatcher = scope.ServiceProvider.GetRequiredService<IAgentUpgradeDispatcher>();
        var dbFactory = scope.ServiceProvider.GetRequiredService<Data.IAppDbFactory>();

        // Re-load the row inside the scope so we work on a fresh copy.
        Remotely.Shared.Entities.AgentUpgradeStatus? row;
        using (var db = dbFactory.GetContext())
        {
            row = await Microsoft.EntityFrameworkCore.EntityFrameworkQueryableExtensions
                .FirstOrDefaultAsync(db.AgentUpgradeStatuses, x => x.Id == statusId, cancellationToken);
        }
        if (row is null)
        {
            return;
        }

        // Refusal-while-busy rail (ROADMAP.md M3 "Safety rails").
        if (await service.HasInFlightJobAsync(row.DeviceId, cancellationToken))
        {
            _logger.LogDebug(
                "Skipping agent-upgrade dispatch — device has in-flight job. DeviceId={deviceId}",
                row.DeviceId);
            return;
        }

        if (!await service.TryReserveAsync(row.Id, cancellationToken))
        {
            return;
        }

        AgentUpgradeTarget? target;
        try
        {
            target = await dispatcher.ResolveTargetAsync(row, cancellationToken);
        }
        catch (Exception ex)
        {
            await service.MarkFailedAsync(row.Id, $"Resolve target failed: {ex.Message}", cancellationToken);
            return;
        }

        if (target is null)
        {
            // No build to land on (no-op dispatcher / device already on
            // target). Roll the row back to Pending so it doesn't sit in
            // Scheduled forever.
            // We use ForceRetry so AttemptCount is not incremented
            // (the device never actually attempted anything); EligibleAt
            // is reset to "now + sweep interval" by setting it manually
            // here through MarkFailedAsync would be wrong (would count
            // as a failed attempt). Instead we just walk the legal
            // transitions.
            await RollbackReservationAsync(scope.ServiceProvider, row.Id, cancellationToken);
            return;
        }

        if (!await service.MarkInProgressAsync(row.Id, cancellationToken))
        {
            return;
        }

        AgentUpgradeDispatchResult result;
        using var dispatchCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        dispatchCts.CancelAfter(_options.DispatchTimeout);
        try
        {
            result = await dispatcher.DispatchAsync(row, target, dispatchCts.Token);
        }
        catch (OperationCanceledException) when (dispatchCts.IsCancellationRequested && !cancellationToken.IsCancellationRequested)
        {
            result = AgentUpgradeDispatchResult.Fail($"Dispatch timed out after {_options.DispatchTimeout}.");
        }
        catch (OperationCanceledException)
        {
            // Host shutting down — leave the row in InProgress for the
            // next instance to surface as a stuck row (the M4 dashboard
            // will show it; an operator can force a retry).
            throw;
        }
        catch (Exception ex)
        {
            result = AgentUpgradeDispatchResult.Fail(ex.Message);
        }

        if (result.Succeeded)
        {
            await service.MarkSucceededAsync(row.Id, target.Version, cancellationToken);
        }
        else
        {
            await service.MarkFailedAsync(row.Id, result.Error ?? "Unknown error.", cancellationToken);
        }
    }

    /// <summary>
    /// Walks Scheduled → InProgress → Failed → Pending without bumping
    /// AttemptCount, so a "no target available" outcome doesn't burn a
    /// retry slot.
    /// </summary>
    private static async Task RollbackReservationAsync(IServiceProvider sp, Guid statusId, CancellationToken cancellationToken)
    {
        var dbFactory = sp.GetRequiredService<Data.IAppDbFactory>();
        using var db = dbFactory.GetContext();
        var row = await Microsoft.EntityFrameworkCore.EntityFrameworkQueryableExtensions
            .FirstOrDefaultAsync(db.AgentUpgradeStatuses, x => x.Id == statusId, cancellationToken);
        if (row is null || row.State != Shared.Enums.AgentUpgradeState.Scheduled)
        {
            return;
        }
        row.State = Shared.Enums.AgentUpgradeState.Pending;
        await db.SaveChangesAsync(cancellationToken);
    }
}
