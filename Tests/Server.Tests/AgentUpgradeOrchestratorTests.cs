using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.Extensions.Options;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Services;
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class AgentUpgradeOrchestratorTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private SystemTime _systemTime = null!;
    private AgentUpgradeService _service = null!;
    private ServiceProvider _scopeRoot = null!;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();
        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _systemTime = new SystemTime();
        _systemTime.Set(new DateTimeOffset(2026, 4, 24, 0, 0, 0, TimeSpan.Zero));
        _service = new AgentUpgradeService(
            _dbFactory,
            _systemTime,
            NullLogger<AgentUpgradeService>.Instance);
    }

    [TestCleanup]
    public void Cleanup()
    {
        _scopeRoot?.Dispose();
    }

    private AgentUpgradeOrchestrator BuildOrchestrator(
        IAgentUpgradeDispatcher dispatcher,
        AgentUpgradeOrchestratorOptions? options = null)
    {
        var services = new ServiceCollection();
        services.AddSingleton(_dbFactory);
        services.AddSingleton<ISystemTime>(_systemTime);
        services.AddSingleton<IAgentUpgradeService>(_service);
        services.AddSingleton(dispatcher);
        _scopeRoot = services.BuildServiceProvider();

        return new AgentUpgradeOrchestrator(
            _scopeRoot,
            Microsoft.Extensions.Options.Options.Create(options ?? new AgentUpgradeOrchestratorOptions { MaxConcurrency = 2, SweepBatchSize = 10 }),
            NullLogger<AgentUpgradeOrchestrator>.Instance);
    }

    // ---- happy path ----

    [TestMethod]
    public async Task SweepOnce_DispatchesEligibleRowAndMarksSucceeded()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "happyDevice", "1.0.0", _systemTime.Now, null);
        var dispatcher = new StubDispatcher
        {
            Target = new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/agent.msi")),
            Outcome = AgentUpgradeDispatchResult.Ok(),
        };
        var orch = BuildOrchestrator(dispatcher);

        await orch.SweepOnceAsync(CancellationToken.None);

        Assert.AreEqual(1, dispatcher.DispatchedDeviceIds.Count);
        Assert.AreEqual("happyDevice", dispatcher.DispatchedDeviceIds[0]);

        using var db = _dbFactory.GetContext();
        var reloaded = db.AgentUpgradeStatuses.Single(x => x.Id == row.Id);
        Assert.AreEqual(AgentUpgradeState.Succeeded, reloaded.State);
        Assert.AreEqual("2.0.0", reloaded.ToVersion);
        Assert.AreEqual(1, reloaded.AttemptCount);
    }

    // ---- 60-day cut-off honoured by the orchestrator ----

    [TestMethod]
    public async Task SweepOnce_SkipsInactiveDevicesEntirely()
    {
        // Enrolled with old LastOnline → starts SkippedInactive.
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deadDevice", "1.0.0",
            _systemTime.Now - TimeSpan.FromDays(180), null);
        Assert.AreEqual(AgentUpgradeState.SkippedInactive, row.State);

        var dispatcher = new StubDispatcher
        {
            Target = new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/agent.msi")),
            Outcome = AgentUpgradeDispatchResult.Ok(),
        };
        var orch = BuildOrchestrator(dispatcher);

        await orch.SweepOnceAsync(CancellationToken.None);

        Assert.AreEqual(0, dispatcher.DispatchedDeviceIds.Count,
            "Inactive device must not be contacted by the sweep.");
    }

    // ---- on-connect path returns inactive devices to the pool ----

    [TestMethod]
    public async Task SweepOnce_InactiveDeviceReconnects_BecomesEligibleNextSweep()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "wakingDevice", "1.0.0",
            _systemTime.Now - TimeSpan.FromDays(180), null);
        var dispatcher = new StubDispatcher
        {
            Target = new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/agent.msi")),
            Outcome = AgentUpgradeDispatchResult.Ok(),
        };
        var orch = BuildOrchestrator(dispatcher);

        await orch.SweepOnceAsync(CancellationToken.None);
        Assert.AreEqual(0, dispatcher.DispatchedDeviceIds.Count);

        // Device reconnects → on-connect hook flips the row.
        await _service.MarkDeviceCameOnlineAsync("wakingDevice");

        await orch.SweepOnceAsync(CancellationToken.None);
        Assert.AreEqual(1, dispatcher.DispatchedDeviceIds.Count);
        Assert.AreEqual("wakingDevice", dispatcher.DispatchedDeviceIds[0]);
    }

    // ---- refusal-while-busy rail ----

    [TestMethod]
    public async Task SweepOnce_RefusesWhileDeviceHasInFlightJob()
    {
        var device = _testData.Org1Device1;
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, device.ID, "1.0.0", _systemTime.Now, null);
        using (var db = _dbFactory.GetContext())
        {
            db.PackageInstallJobs.Add(new PackageInstallJob
            {
                Id = Guid.NewGuid(),
                OrganizationID = _testData.Org1Id,
                PackageId = Guid.NewGuid(),
                DeviceId = device.ID,
                Status = PackageInstallJobStatus.Running,
            });
            await db.SaveChangesAsync();
        }
        var dispatcher = new StubDispatcher
        {
            Target = new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/a.msi")),
            Outcome = AgentUpgradeDispatchResult.Ok(),
        };
        var orch = BuildOrchestrator(dispatcher);

        await orch.SweepOnceAsync(CancellationToken.None);

        Assert.AreEqual(0, dispatcher.DispatchedDeviceIds.Count,
            "Orchestrator must skip a device with an in-flight package job.");
        using var verifyDb = _dbFactory.GetContext();
        var reloaded = verifyDb.AgentUpgradeStatuses.Single(x => x.Id == row.Id);
        Assert.AreEqual(AgentUpgradeState.Pending, reloaded.State,
            "Skipped row must remain Pending so the next sweep retries.");
        Assert.AreEqual(0, reloaded.AttemptCount,
            "A skipped sweep must not burn a retry slot.");
    }

    // ---- failure → backoff ----

    [TestMethod]
    public async Task SweepOnce_DispatcherFailure_RequeuesWithBackoff()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "flakyDevice", "1.0.0", _systemTime.Now, null);
        var dispatcher = new StubDispatcher
        {
            Target = new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/a.msi")),
            Outcome = AgentUpgradeDispatchResult.Fail("network glitch"),
        };
        var orch = BuildOrchestrator(dispatcher);

        await orch.SweepOnceAsync(CancellationToken.None);

        using var db = _dbFactory.GetContext();
        var reloaded = db.AgentUpgradeStatuses.Single(x => x.Id == row.Id);
        Assert.AreEqual(AgentUpgradeState.Pending, reloaded.State);
        Assert.AreEqual(1, reloaded.AttemptCount);
        Assert.AreEqual("network glitch", reloaded.LastAttemptError);
        Assert.IsTrue(reloaded.EligibleAt > _systemTime.Now,
            "After a failure the row must not be eligible again until backoff has elapsed.");
    }

    // ---- no target → roll back without burning a retry slot ----

    [TestMethod]
    public async Task SweepOnce_NoTargetAvailable_RollsBackToPendingWithoutBumpingAttempts()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "noTargetDevice", "1.0.0", _systemTime.Now, null);
        var dispatcher = new StubDispatcher
        {
            Target = null,
            Outcome = AgentUpgradeDispatchResult.Ok(),
        };
        var orch = BuildOrchestrator(dispatcher);

        await orch.SweepOnceAsync(CancellationToken.None);

        using var db = _dbFactory.GetContext();
        var reloaded = db.AgentUpgradeStatuses.Single(x => x.Id == row.Id);
        Assert.AreEqual(AgentUpgradeState.Pending, reloaded.State);
        Assert.AreEqual(0, reloaded.AttemptCount,
            "Rolling back a Scheduled row because no target is published must not consume a retry slot.");
        Assert.AreEqual(0, dispatcher.DispatchedDeviceIds.Count);
    }

    // ---- concurrency rail ----

    [TestMethod]
    public async Task SweepOnce_RespectsMaxConcurrency()
    {
        const int rows = 6;
        for (int i = 0; i < rows; i++)
        {
            await _service.EnrolDeviceAsync(_testData.Org1Id, $"dev{i}", "1.0.0", _systemTime.Now, null);
        }
        var dispatcher = new ConcurrencyTrackingDispatcher();
        var orch = BuildOrchestrator(dispatcher,
            new AgentUpgradeOrchestratorOptions { MaxConcurrency = 2, SweepBatchSize = 100 });

        await orch.SweepOnceAsync(CancellationToken.None);

        Assert.AreEqual(rows, dispatcher.TotalCalls);
        Assert.IsTrue(dispatcher.MaxObservedConcurrent <= 2,
            $"Observed concurrency {dispatcher.MaxObservedConcurrent} exceeded MaxConcurrency=2.");
    }

    // ---- batch-size rail ----

    [TestMethod]
    public async Task SweepOnce_BatchSizeCapsRowsProcessedPerSweep()
    {
        for (int i = 0; i < 5; i++)
        {
            await _service.EnrolDeviceAsync(_testData.Org1Id, $"bdev{i}", "1.0.0", _systemTime.Now, null);
        }
        var dispatcher = new StubDispatcher
        {
            Target = new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/a.msi")),
            Outcome = AgentUpgradeDispatchResult.Ok(),
        };
        var orch = BuildOrchestrator(dispatcher,
            new AgentUpgradeOrchestratorOptions { MaxConcurrency = 5, SweepBatchSize = 2 });

        await orch.SweepOnceAsync(CancellationToken.None);
        Assert.AreEqual(2, dispatcher.DispatchedDeviceIds.Count);
    }

    // ---- support classes ----

    private sealed class StubDispatcher : IAgentUpgradeDispatcher
    {
        public AgentUpgradeTarget? Target { get; set; }
        public AgentUpgradeDispatchResult Outcome { get; set; } = AgentUpgradeDispatchResult.Ok();
        public List<string> DispatchedDeviceIds { get; } = new();

        public Task<AgentUpgradeTarget?> ResolveTargetAsync(AgentUpgradeStatus status, CancellationToken cancellationToken)
            => Task.FromResult(Target);

        public Task<AgentUpgradeDispatchResult> DispatchAsync(AgentUpgradeStatus status, AgentUpgradeTarget target, CancellationToken cancellationToken)
        {
            lock (DispatchedDeviceIds)
            {
                DispatchedDeviceIds.Add(status.DeviceId);
            }
            return Task.FromResult(Outcome);
        }
    }

    private sealed class ConcurrencyTrackingDispatcher : IAgentUpgradeDispatcher
    {
        private int _inflight;
        public int MaxObservedConcurrent { get; private set; }
        public int TotalCalls { get; private set; }
        private readonly object _gate = new();

        public Task<AgentUpgradeTarget?> ResolveTargetAsync(AgentUpgradeStatus status, CancellationToken cancellationToken)
            => Task.FromResult<AgentUpgradeTarget?>(new AgentUpgradeTarget("2.0.0", "abc", new Uri("https://example.com/a.msi")));

        public async Task<AgentUpgradeDispatchResult> DispatchAsync(AgentUpgradeStatus status, AgentUpgradeTarget target, CancellationToken cancellationToken)
        {
            int now = Interlocked.Increment(ref _inflight);
            lock (_gate)
            {
                if (now > MaxObservedConcurrent) MaxObservedConcurrent = now;
                TotalCalls++;
            }
            try
            {
                // Hold long enough that other tasks pile up so the gate
                // is meaningfully exercised.
                await Task.Delay(50, cancellationToken);
            }
            finally
            {
                Interlocked.Decrement(ref _inflight);
            }
            return AgentUpgradeDispatchResult.Ok();
        }
    }
}
