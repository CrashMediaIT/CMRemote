using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;
using Remotely.Migration.Legacy.Converters;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class MigrationRunnerTests
{
    [TestMethod]
    public async Task RunAsync_UnknownSchema_RecordsFatalErrorAndDoesNotEnumerateConverters()
    {
        var runner = new MigrationRunner(
            new FakeInspector(LegacySchemaVersion.Unknown),
            new object[] { new OrganizationRowConverter() });

        var report = await runner.RunAsync(NewOptions(dryRun: true));

        Assert.AreEqual(LegacySchemaVersion.Unknown, report.DetectedSchemaVersion);
        Assert.AreEqual(0, report.Entities.Count);
        Assert.AreEqual(1, report.FatalErrors.Count);
        Assert.IsNotNull(report.CompletedAtUtc);
    }

    [TestMethod]
    public async Task RunAsync_EmptySchema_CompletesCleanlyWithNoEntities()
    {
        var runner = new MigrationRunner(
            new FakeInspector(LegacySchemaVersion.Empty),
            new object[] { new OrganizationRowConverter() });

        var report = await runner.RunAsync(NewOptions());

        Assert.AreEqual(LegacySchemaVersion.Empty, report.DetectedSchemaVersion);
        Assert.AreEqual(0, report.Entities.Count);
        Assert.AreEqual(0, report.FatalErrors.Count);
    }

    [TestMethod]
    public async Task RunAsync_KnownSchema_EnumeratesMatchingConverters()
    {
        var runner = new MigrationRunner(
            new FakeInspector(LegacySchemaVersion.UpstreamLegacy_2026_04),
            new object[] { new OrganizationRowConverter() });

        var report = await runner.RunAsync(NewOptions());

        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, report.DetectedSchemaVersion);
        Assert.AreEqual(1, report.Entities.Count);
        Assert.AreEqual("Organization", report.Entities[0].EntityName);
        Assert.AreEqual(0, report.Entities[0].RowsRead, "Scaffold runner does not read rows yet.");
        Assert.AreEqual(0, report.FatalErrors.Count);
    }

    [TestMethod]
    public async Task RunAsync_InspectorThrows_RecordsFatalErrorRatherThanCrashing()
    {
        var runner = new MigrationRunner(
            new ThrowingInspector(),
            new object[] { new OrganizationRowConverter() });

        var report = await runner.RunAsync(NewOptions());

        Assert.AreEqual(1, report.FatalErrors.Count);
        StringAssert.Contains(report.FatalErrors[0], "boom");
        Assert.IsNotNull(report.CompletedAtUtc);
    }

    [TestMethod]
    public async Task RunAsync_Cancelled_PropagatesOperationCanceled()
    {
        var runner = new MigrationRunner(
            new CancelOnDetectInspector(),
            new object[] { new OrganizationRowConverter() });

        using var cts = new CancellationTokenSource();
        cts.Cancel();

        await Assert.ThrowsExceptionAsync<OperationCanceledException>(
            () => runner.RunAsync(NewOptions(), cts.Token));
    }

    [TestMethod]
    public async Task RunAsync_ReportRoundTripsThroughJson()
    {
        var runner = new MigrationRunner(
            new FakeInspector(LegacySchemaVersion.UpstreamLegacy_2026_04),
            new object[] { new OrganizationRowConverter() });

        var report = await runner.RunAsync(NewOptions(dryRun: true));

        var json = report.ToJson();
        var roundTripped = MigrationReport.FromJson(json);

        Assert.AreEqual(report.DetectedSchemaVersion, roundTripped.DetectedSchemaVersion);
        Assert.AreEqual(report.DryRun, roundTripped.DryRun);
        Assert.AreEqual(report.Entities.Count, roundTripped.Entities.Count);
        Assert.AreEqual(report.Entities[0].EntityName, roundTripped.Entities[0].EntityName);
        Assert.AreEqual(report.ReportSchemaVersion, roundTripped.ReportSchemaVersion);
    }

    private static MigrationOptions NewOptions(bool dryRun = false) => new()
    {
        SourceConnectionString = "Data Source=:memory:",
        TargetConnectionString = "Host=localhost;Database=cmremote_v2",
        DryRun = dryRun,
    };

    private sealed class FakeInspector : ILegacySchemaInspector
    {
        private readonly LegacySchemaVersion _result;
        public FakeInspector(LegacySchemaVersion result) => _result = result;
        public Task<LegacySchemaVersion> DetectAsync(string source, CancellationToken ct = default)
            => Task.FromResult(_result);
    }

    private sealed class ThrowingInspector : ILegacySchemaInspector
    {
        public Task<LegacySchemaVersion> DetectAsync(string source, CancellationToken ct = default)
            => throw new InvalidOperationException("boom");
    }

    private sealed class CancelOnDetectInspector : ILegacySchemaInspector
    {
        public Task<LegacySchemaVersion> DetectAsync(string source, CancellationToken ct = default)
        {
            ct.ThrowIfCancellationRequested();
            return Task.FromResult(LegacySchemaVersion.Empty);
        }
    }
}
