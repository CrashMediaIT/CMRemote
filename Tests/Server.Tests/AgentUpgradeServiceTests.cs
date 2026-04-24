using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Services;
using System;
using System.Linq;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class AgentUpgradeServiceTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private SystemTime _systemTime = null!;
    private AgentUpgradeService _service = null!;

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

    // ---- pure transition predicate ----

    [TestMethod]
    public void IsLegalTransition_PendingFlowsToScheduledAndSkips()
    {
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Pending, AgentUpgradeState.Scheduled));
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Pending, AgentUpgradeState.SkippedInactive));
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Pending, AgentUpgradeState.SkippedOptOut));
        // Cannot jump straight to terminal
        Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Pending, AgentUpgradeState.Succeeded));
        Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Pending, AgentUpgradeState.Failed));
        Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Pending, AgentUpgradeState.InProgress));
    }

    [TestMethod]
    public void IsLegalTransition_InProgressOnlyToTerminals()
    {
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.InProgress, AgentUpgradeState.Succeeded));
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.InProgress, AgentUpgradeState.Failed));
        Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.InProgress, AgentUpgradeState.Pending));
        Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.InProgress, AgentUpgradeState.SkippedOptOut));
    }

    [TestMethod]
    public void IsLegalTransition_FailedRequeuesToPending()
    {
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Failed, AgentUpgradeState.Pending));
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.Failed, AgentUpgradeState.SkippedOptOut));
    }

    [TestMethod]
    public void IsLegalTransition_SkippedInactiveBacktoPending()
    {
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.SkippedInactive, AgentUpgradeState.Pending));
        Assert.IsTrue(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.SkippedInactive, AgentUpgradeState.SkippedOptOut));
        Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(AgentUpgradeState.SkippedInactive, AgentUpgradeState.Scheduled));
    }

    [TestMethod]
    public void IsLegalTransition_SameStateIsNoop()
    {
        foreach (var s in Enum.GetValues<AgentUpgradeState>())
        {
            Assert.IsFalse(IAgentUpgradeService.IsLegalTransition(s, s),
                $"Self-transition for {s} should be illegal.");
        }
    }

    // ---- backoff math ----

    [TestMethod]
    public void ComputeBackoff_FirstRetryIsOneMinute()
    {
        Assert.AreEqual(TimeSpan.FromSeconds(60), IAgentUpgradeService.ComputeBackoff(1));
    }

    [TestMethod]
    public void ComputeBackoff_DoublesEachAttempt()
    {
        Assert.AreEqual(TimeSpan.FromSeconds(120), IAgentUpgradeService.ComputeBackoff(2));
        Assert.AreEqual(TimeSpan.FromSeconds(240), IAgentUpgradeService.ComputeBackoff(3));
    }

    [TestMethod]
    public void ComputeBackoff_CapsAtMaxBackoff()
    {
        // The capped delay is 24h regardless of how high attempt goes.
        Assert.AreEqual(IAgentUpgradeService.MaxBackoff, IAgentUpgradeService.ComputeBackoff(50));
        Assert.AreEqual(IAgentUpgradeService.MaxBackoff, IAgentUpgradeService.ComputeBackoff(int.MaxValue));
    }

    [TestMethod]
    public void ComputeBackoff_ZeroOrNegativeIsZero()
    {
        Assert.AreEqual(TimeSpan.Zero, IAgentUpgradeService.ComputeBackoff(0));
        Assert.AreEqual(TimeSpan.Zero, IAgentUpgradeService.ComputeBackoff(-3));
    }

    // ---- enrolment ----

    [TestMethod]
    public async Task EnrolDeviceAsync_FreshDevice_StartsPending()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceA", "1.0.0", _systemTime.Now, "2.0.0");
        Assert.AreEqual(AgentUpgradeState.Pending, row.State);
        Assert.AreEqual("1.0.0", row.FromVersion);
        Assert.AreEqual("2.0.0", row.ToVersion);
        Assert.AreEqual(0, row.AttemptCount);
        Assert.AreEqual(_systemTime.Now, row.EligibleAt);
    }

    [TestMethod]
    public async Task EnrolDeviceAsync_DeviceInactiveBeyondCutoff_StartsSkippedInactive()
    {
        var oldOnline = _systemTime.Now - TimeSpan.FromDays(120);
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceB", "1.0.0", oldOnline, "2.0.0");
        Assert.AreEqual(AgentUpgradeState.SkippedInactive, row.State);
    }

    [TestMethod]
    public async Task EnrolDeviceAsync_IsIdempotent()
    {
        var row1 = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceC", "1.0.0", _systemTime.Now, "2.0.0");
        var row2 = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceC", "1.0.1", _systemTime.Now, "9.9.9");
        Assert.AreEqual(row1.Id, row2.Id);
        // Existing row is returned verbatim — no FromVersion/ToVersion churn.
        Assert.AreEqual("1.0.0", row2.FromVersion);
        Assert.AreEqual("2.0.0", row2.ToVersion);
    }

    [TestMethod]
    public async Task EnrolDeviceAsync_GuardsBlankInputs()
    {
        await Assert.ThrowsExceptionAsync<ArgumentException>(() =>
            _service.EnrolDeviceAsync("", "d", null, _systemTime.Now, null));
        await Assert.ThrowsExceptionAsync<ArgumentException>(() =>
            _service.EnrolDeviceAsync(_testData.Org1Id, "  ", null, _systemTime.Now, null));
    }

    // ---- on-connect reactivation ----

    [TestMethod]
    public async Task MarkDeviceCameOnlineAsync_FlipsSkippedInactiveBackToPending()
    {
        // Enrol with a far-past LastOnline so the row starts as SkippedInactive.
        await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceD", "1.0.0",
            _systemTime.Now - TimeSpan.FromDays(120), "2.0.0");

        // Advance the clock + simulate device reconnect.
        _systemTime.Offset(TimeSpan.FromMinutes(5));
        var updated = await _service.MarkDeviceCameOnlineAsync("deviceD");
        Assert.IsNotNull(updated);
        Assert.AreEqual(AgentUpgradeState.Pending, updated!.State);
        Assert.AreEqual(_systemTime.Now, updated.EligibleAt);
    }

    [TestMethod]
    public async Task MarkDeviceCameOnlineAsync_LeavesSkippedOptOutAlone()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceE", "1.0.0", _systemTime.Now, null);
        Assert.IsTrue(await _service.SetOptOutAsync(row.Id));
        var observed = await _service.MarkDeviceCameOnlineAsync("deviceE");
        Assert.IsNotNull(observed);
        Assert.AreEqual(AgentUpgradeState.SkippedOptOut, observed!.State);
    }

    [TestMethod]
    public async Task MarkDeviceCameOnlineAsync_UnknownDeviceReturnsNull()
    {
        Assert.IsNull(await _service.MarkDeviceCameOnlineAsync("does-not-exist"));
    }

    // ---- reservation race ----

    [TestMethod]
    public async Task TryReserveAsync_TransitionsPendingToScheduled()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceF", "1.0.0", _systemTime.Now, null);
        Assert.IsTrue(await _service.TryReserveAsync(row.Id));
        // A second concurrent attempt cannot reserve — the row is no
        // longer Pending, so the transition is illegal.
        Assert.IsFalse(await _service.TryReserveAsync(row.Id));
    }

    [TestMethod]
    public async Task TryReserveAsync_RefusesWhenEligibleAtInTheFuture()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceG", "1.0.0", _systemTime.Now, null);
        // Push EligibleAt forward by simulating a failure that requeues.
        Assert.IsTrue(await _service.TryReserveAsync(row.Id));
        Assert.IsTrue(await _service.MarkInProgressAsync(row.Id));
        Assert.IsTrue(await _service.MarkFailedAsync(row.Id, "boom"));
        // Backoff applied; now reserving should refuse because EligibleAt > now.
        Assert.IsFalse(await _service.TryReserveAsync(row.Id));

        // Advance clock past the backoff and try again.
        _systemTime.Offset(TimeSpan.FromHours(2));
        Assert.IsTrue(await _service.TryReserveAsync(row.Id));
    }

    // ---- terminal stamping ----

    [TestMethod]
    public async Task MarkSucceeded_StampsCompletedAtAndClearsError()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceH", "1.0.0", _systemTime.Now, "2.0.0");
        await _service.TryReserveAsync(row.Id);
        await _service.MarkInProgressAsync(row.Id);
        Assert.IsTrue(await _service.MarkSucceededAsync(row.Id, "2.5.0"));
        var reloaded = await GetRowAsync(row.Id);
        Assert.AreEqual(AgentUpgradeState.Succeeded, reloaded.State);
        Assert.IsNotNull(reloaded.CompletedAt);
        Assert.AreEqual("2.5.0", reloaded.ToVersion);
        Assert.IsNull(reloaded.LastAttemptError);
    }

    // ---- retry / backoff state machine ----

    [TestMethod]
    public async Task MarkFailed_RequeuesWithBackoffUntilMaxAttempts()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceI", "1.0.0", _systemTime.Now, null);

        for (int attempt = 1; attempt < IAgentUpgradeService.MaxAttempts; attempt++)
        {
            // Push the clock past any prior backoff so the row is eligible again.
            _systemTime.Offset(TimeSpan.FromHours(48));
            Assert.IsTrue(await _service.TryReserveAsync(row.Id), $"reserve #{attempt}");
            Assert.IsTrue(await _service.MarkInProgressAsync(row.Id), $"in-progress #{attempt}");
            Assert.IsTrue(await _service.MarkFailedAsync(row.Id, $"err {attempt}"), $"fail #{attempt}");

            var mid = await GetRowAsync(row.Id);
            Assert.AreEqual(AgentUpgradeState.Pending, mid.State,
                $"After failure #{attempt}, row should be requeued.");
            Assert.AreEqual(attempt, mid.AttemptCount);
            Assert.IsTrue(mid.EligibleAt > _systemTime.Now,
                $"Backoff should push EligibleAt into the future after failure #{attempt}.");
            Assert.AreEqual($"err {attempt}", mid.LastAttemptError);
        }

        // The next failure exhausts the budget — row stays Failed.
        _systemTime.Offset(TimeSpan.FromHours(48));
        Assert.IsTrue(await _service.TryReserveAsync(row.Id));
        Assert.IsTrue(await _service.MarkInProgressAsync(row.Id));
        Assert.IsTrue(await _service.MarkFailedAsync(row.Id, "final"));
        var final = await GetRowAsync(row.Id);
        Assert.AreEqual(AgentUpgradeState.Failed, final.State);
        Assert.AreEqual(IAgentUpgradeService.MaxAttempts, final.AttemptCount);
        Assert.AreEqual("final", final.LastAttemptError);
    }

    // ---- operator overrides ----

    [TestMethod]
    public async Task ForceRetry_ResetsAttemptsAndRequeuesEvenFromExhaustedFailed()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceJ", "1.0.0", _systemTime.Now, null);

        // Drive to terminal Failed.
        for (int i = 0; i < IAgentUpgradeService.MaxAttempts; i++)
        {
            _systemTime.Offset(TimeSpan.FromHours(48));
            Assert.IsTrue(await _service.TryReserveAsync(row.Id));
            Assert.IsTrue(await _service.MarkInProgressAsync(row.Id));
            Assert.IsTrue(await _service.MarkFailedAsync(row.Id, "x"));
        }
        Assert.AreEqual(AgentUpgradeState.Failed, (await GetRowAsync(row.Id)).State);

        Assert.IsTrue(await _service.ForceRetryAsync(row.Id));
        var reloaded = await GetRowAsync(row.Id);
        Assert.AreEqual(AgentUpgradeState.Pending, reloaded.State);
        Assert.AreEqual(0, reloaded.AttemptCount);
        Assert.IsNull(reloaded.LastAttemptError);
        Assert.AreEqual(_systemTime.Now, reloaded.EligibleAt);
    }

    [TestMethod]
    public async Task SetOptOut_RefusedWhileInProgress()
    {
        var row = await _service.EnrolDeviceAsync(
            _testData.Org1Id, "deviceK", "1.0.0", _systemTime.Now, null);
        Assert.IsTrue(await _service.TryReserveAsync(row.Id));
        Assert.IsTrue(await _service.MarkInProgressAsync(row.Id));
        Assert.IsFalse(await _service.SetOptOutAsync(row.Id),
            "Operator must not be able to opt out a device mid-upgrade.");
    }

    // ---- refusal-while-busy rail ----

    [TestMethod]
    public async Task HasInFlightJob_ReturnsTrueForQueuedAndRunning()
    {
        var device = _testData.Org1Device1;
        using (var db = _dbFactory.GetContext())
        {
            db.PackageInstallJobs.Add(new PackageInstallJob
            {
                Id = Guid.NewGuid(),
                OrganizationID = _testData.Org1Id,
                PackageId = Guid.NewGuid(),
                DeviceId = device.ID,
                Status = PackageInstallJobStatus.Queued,
            });
            await db.SaveChangesAsync();
        }
        Assert.IsTrue(await _service.HasInFlightJobAsync(device.ID));
    }

    [TestMethod]
    public async Task HasInFlightJob_FalseForTerminalJobsOnly()
    {
        var device = _testData.Org1Device2;
        using (var db = _dbFactory.GetContext())
        {
            db.PackageInstallJobs.Add(new PackageInstallJob
            {
                Id = Guid.NewGuid(),
                OrganizationID = _testData.Org1Id,
                PackageId = Guid.NewGuid(),
                DeviceId = device.ID,
                Status = PackageInstallJobStatus.Success,
            });
            db.PackageInstallJobs.Add(new PackageInstallJob
            {
                Id = Guid.NewGuid(),
                OrganizationID = _testData.Org1Id,
                PackageId = Guid.NewGuid(),
                DeviceId = device.ID,
                Status = PackageInstallJobStatus.Cancelled,
            });
            await db.SaveChangesAsync();
        }
        Assert.IsFalse(await _service.HasInFlightJobAsync(device.ID));
    }

    [TestMethod]
    public async Task HasInFlightJob_BlankDeviceIdIsFalse()
    {
        Assert.IsFalse(await _service.HasInFlightJobAsync(""));
        Assert.IsFalse(await _service.HasInFlightJobAsync(null!));
    }

    // ---- aggregates ----

    [TestMethod]
    public async Task GetStateCounts_ReturnsZeroForEmptyStatesAndCountsRest()
    {
        await _service.EnrolDeviceAsync(_testData.Org1Id, "devA", "1", _systemTime.Now, null);
        await _service.EnrolDeviceAsync(_testData.Org1Id, "devB", "1", _systemTime.Now, null);
        await _service.EnrolDeviceAsync(_testData.Org1Id, "devC", "1",
            _systemTime.Now - TimeSpan.FromDays(100), null);

        var counts = await _service.GetStateCountsAsync(_testData.Org1Id);
        Assert.AreEqual(2, counts[AgentUpgradeState.Pending]);
        Assert.AreEqual(1, counts[AgentUpgradeState.SkippedInactive]);
        Assert.AreEqual(0, counts[AgentUpgradeState.Succeeded]);
        // All enum values are present in the dictionary so the dashboard
        // can render a stable layout without nullability handling.
        foreach (var s in Enum.GetValues<AgentUpgradeState>())
        {
            Assert.IsTrue(counts.ContainsKey(s), $"State {s} missing.");
        }
    }

    [TestMethod]
    public async Task GetEligibleAsync_OrdersByEligibleAtAndRespectsLimit()
    {
        var rowA = await _service.EnrolDeviceAsync(_testData.Org1Id, "devEA", "1", _systemTime.Now, null);
        // Make rowA eligible later than rowB.
        using (var db = _dbFactory.GetContext())
        {
            var tracked = db.AgentUpgradeStatuses.First(x => x.Id == rowA.Id);
            tracked.EligibleAt = _systemTime.Now - TimeSpan.FromMinutes(1);
            await db.SaveChangesAsync();
        }
        var rowB = await _service.EnrolDeviceAsync(_testData.Org1Id, "devEB", "1", _systemTime.Now, null);
        using (var db = _dbFactory.GetContext())
        {
            var tracked = db.AgentUpgradeStatuses.First(x => x.Id == rowB.Id);
            tracked.EligibleAt = _systemTime.Now - TimeSpan.FromMinutes(10);
            await db.SaveChangesAsync();
        }

        var eligible = await _service.GetEligibleAsync(10);
        Assert.AreEqual(2, eligible.Count);
        Assert.AreEqual(rowB.Id, eligible[0].Id, "Older EligibleAt sorts first.");
        Assert.AreEqual(rowA.Id, eligible[1].Id);

        var capped = await _service.GetEligibleAsync(1);
        Assert.AreEqual(1, capped.Count);
        Assert.AreEqual(rowB.Id, capped[0].Id);

        var none = await _service.GetEligibleAsync(0);
        Assert.AreEqual(0, none.Count);
    }

    private async Task<AgentUpgradeStatus> GetRowAsync(Guid id)
    {
        using var db = _dbFactory.GetContext();
        var row = await Microsoft.EntityFrameworkCore.EntityFrameworkQueryableExtensions
            .FirstOrDefaultAsync(db.AgentUpgradeStatuses, x => x.Id == id);
        Assert.IsNotNull(row);
        return row!;
    }
}
