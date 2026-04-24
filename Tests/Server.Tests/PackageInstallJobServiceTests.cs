using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services;
using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Services;
using System;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class PackageInstallJobServiceTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private PackageInstallJobService _service = null!;
    private Guid _packageId;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();

        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _service = new PackageInstallJobService(
            _dbFactory,
            new SystemTime(),
            new NoopRateLimiter(),
            NullLogger<PackageInstallJobService>.Instance);

        // Seed a package the service can reference.
        using var db = _dbFactory.GetContext();
        var package = new Package
        {
            Id = Guid.NewGuid(),
            OrganizationID = _testData.Org1Id,
            Name = "Chrome",
            Provider = PackageProvider.Chocolatey,
            PackageIdentifier = "googlechrome",
            CreatedAt = DateTimeOffset.UtcNow,
        };
        db.Packages.Add(package);
        await db.SaveChangesAsync();
        _packageId = package.Id;
    }

    // ---- pure transition predicate ----

    [TestMethod]
    public void IsLegalTransition_Queued_AllowsRunningAndCancelled()
    {
        Assert.IsTrue(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Queued, PackageInstallJobStatus.Running));
        Assert.IsTrue(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Queued, PackageInstallJobStatus.Cancelled));
    }

    [TestMethod]
    public void IsLegalTransition_Queued_RejectsTerminalSuccessOrFailed()
    {
        // A job MUST go through Running before terminating Success/Failed.
        // Skipping Running would lose the StartedAt timestamp.
        Assert.IsFalse(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Queued, PackageInstallJobStatus.Success));
        Assert.IsFalse(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Queued, PackageInstallJobStatus.Failed));
    }

    [TestMethod]
    public void IsLegalTransition_Running_AllowsTerminals()
    {
        Assert.IsTrue(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Running, PackageInstallJobStatus.Success));
        Assert.IsTrue(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Running, PackageInstallJobStatus.Failed));
        Assert.IsTrue(IPackageInstallJobService.IsLegalTransition(
            PackageInstallJobStatus.Running, PackageInstallJobStatus.Cancelled));
    }

    [TestMethod]
    public void IsLegalTransition_TerminalStates_AreSticky()
    {
        foreach (var terminal in new[] {
            PackageInstallJobStatus.Success,
            PackageInstallJobStatus.Failed,
            PackageInstallJobStatus.Cancelled })
        {
            foreach (var to in new[] {
                PackageInstallJobStatus.Queued,
                PackageInstallJobStatus.Running,
                PackageInstallJobStatus.Success,
                PackageInstallJobStatus.Failed,
                PackageInstallJobStatus.Cancelled })
            {
                Assert.IsFalse(
                    IPackageInstallJobService.IsLegalTransition(terminal, to),
                    $"Expected terminal {terminal} -> {to} to be rejected.");
            }
        }
    }

    // ---- live state machine via the DB ----

    [TestMethod]
    public async Task QueueJob_PersistsAsQueued()
    {
        var job = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, bundleId: null, requestedByUserId: _testData.Org1Admin1.Id);

        Assert.AreEqual(PackageInstallJobStatus.Queued, job.Status);
        Assert.IsNull(job.StartedAt);
        Assert.IsNull(job.CompletedAt);
        Assert.AreEqual(_testData.Org1Id, job.OrganizationID);
    }

    [TestMethod]
    public async Task FullHappyPath_Queued_Running_Success_PersistsResult()
    {
        var job = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, _testData.Org1Admin1.Id);

        Assert.IsTrue(await _service.MarkDispatchedAsync(job.Id));

        var ok = await _service.CompleteJobAsync(job.Id, new PackageInstallResultDto
        {
            JobId = job.Id.ToString("D"),
            Success = true,
            ExitCode = 0,
            DurationMs = 4321,
            StdoutTail = "installed",
        });
        Assert.IsTrue(ok);

        var loaded = await _service.GetJobAsync(_testData.Org1Id, job.Id);
        Assert.IsNotNull(loaded);
        Assert.AreEqual(PackageInstallJobStatus.Success, loaded!.Status);
        Assert.IsNotNull(loaded.StartedAt);
        Assert.IsNotNull(loaded.CompletedAt);
        Assert.IsNotNull(loaded.Result);
        Assert.IsTrue(loaded.Result!.Success);
        Assert.AreEqual(0, loaded.Result.ExitCode);
        Assert.AreEqual(4321, loaded.Result.DurationMs);
    }

    [TestMethod]
    public async Task CompleteJob_FromQueued_IsRejected_NoStartedTimestamp()
    {
        var job = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, null);

        // Skipping the Running transition must be refused — otherwise
        // we'd lose the startedAt invariant.
        var ok = await _service.CompleteJobAsync(job.Id, new PackageInstallResultDto
        {
            JobId = job.Id.ToString("D"),
            Success = true,
            ExitCode = 0,
        });
        Assert.IsFalse(ok);

        var loaded = await _service.GetJobAsync(_testData.Org1Id, job.Id);
        Assert.AreEqual(PackageInstallJobStatus.Queued, loaded!.Status);
        Assert.IsNull(loaded.Result);
    }

    [TestMethod]
    public async Task CompleteJob_AfterTerminal_IsIgnored()
    {
        var job = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, null);
        await _service.MarkDispatchedAsync(job.Id);
        await _service.CompleteJobAsync(job.Id, new PackageInstallResultDto
        {
            JobId = job.Id.ToString("D"), Success = false, ExitCode = 1,
        });

        // A second result for the same job must not flip Failed → Success.
        var ok = await _service.CompleteJobAsync(job.Id, new PackageInstallResultDto
        {
            JobId = job.Id.ToString("D"), Success = true, ExitCode = 0,
        });
        Assert.IsFalse(ok);

        var loaded = await _service.GetJobAsync(_testData.Org1Id, job.Id);
        Assert.AreEqual(PackageInstallJobStatus.Failed, loaded!.Status);
        Assert.AreEqual(1, loaded.Result!.ExitCode);
    }

    [TestMethod]
    public async Task CancelJob_FromQueued_Succeeds_FromRunning_Succeeds_FromTerminal_Fails()
    {
        var queued = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, null);
        Assert.IsTrue(await _service.CancelJobAsync(_testData.Org1Id, queued.Id));

        var running = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, null);
        await _service.MarkDispatchedAsync(running.Id);
        Assert.IsTrue(await _service.CancelJobAsync(_testData.Org1Id, running.Id));

        var done = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, null);
        await _service.MarkDispatchedAsync(done.Id);
        await _service.CompleteJobAsync(done.Id, new PackageInstallResultDto
        {
            JobId = done.Id.ToString("D"), Success = true, ExitCode = 0,
        });
        Assert.IsFalse(await _service.CancelJobAsync(_testData.Org1Id, done.Id));
    }

    [TestMethod]
    public async Task GetJob_RejectsCrossOrgRead()
    {
        var job = await _service.QueueJobAsync(
            _testData.Org1Id, _packageId, _testData.Org1Device1.ID,
            PackageInstallAction.Install, null, null);

        var foreign = await _service.GetJobAsync(_testData.Org2Admin1.OrganizationID, job.Id);
        Assert.IsNull(foreign);
    }

    [TestMethod]
    public async Task QueueJob_RejectsForeignPackage()
    {
        // A caller in Org2 cannot queue against Org1's package — even
        // if they guess the GUID, the org-scoped lookup must reject it.
        await Assert.ThrowsExceptionAsync<InvalidOperationException>(async () =>
            await _service.QueueJobAsync(
                _testData.Org2Admin1.OrganizationID,
                _packageId,
                _testData.Org2Device1.ID,
                PackageInstallAction.Install,
                null,
                null));
    }

    /// <summary>
    /// Test-only no-op rate limiter so the existing service tests keep
    /// asserting the state-machine semantics without coupling to the
    /// per-org budget. The dedicated
    /// <c>PackageInstallJobRateLimiterTests</c> exercises the limiter
    /// itself.
    /// </summary>
    private sealed class NoopRateLimiter : IPackageInstallJobRateLimiter
    {
        public Task<bool> TryAcquireAsync(string organizationId, int permitCount, CancellationToken cancellationToken)
            => Task.FromResult(true);
    }
}
