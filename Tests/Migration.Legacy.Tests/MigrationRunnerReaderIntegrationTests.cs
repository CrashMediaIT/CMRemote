using Microsoft.Data.Sqlite;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Readers;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class MigrationRunnerReaderIntegrationTests
{
    [TestMethod]
    public async Task RunAsync_KnownSchema_WithReaderAndConverter_StreamsAndCounts()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await SeedUpstreamSchemaAsync(keepAlive);
        await SeedOrganizationsAsync(keepAlive,
            ("a", "Alpha", true),
            ("b", "Bravo", false),
            ("c", "Charlie", false));

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() });

        var report = await runner.RunAsync(NewOptions(conn));

        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, report.DetectedSchemaVersion);
        Assert.AreEqual(0, report.FatalErrors.Count);
        Assert.AreEqual(1, report.Entities.Count);

        var entity = report.Entities[0];
        Assert.AreEqual("Organization", entity.EntityName);
        Assert.AreEqual(3, entity.RowsRead);
        Assert.AreEqual(3, entity.RowsConverted);
        Assert.AreEqual(0, entity.RowsSkipped);
        Assert.AreEqual(0, entity.RowsFailed);
        Assert.AreEqual(0, entity.Errors.Count);
    }

    [TestMethod]
    public async Task RunAsync_KnownSchema_CountsSkipsAndFailuresFromConverter()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await SeedUpstreamSchemaAsync(keepAlive);
        // Mix: 1 happy, 2 with no usable name (one NULL, one
        // whitespace-only) — the existing OrganizationRowConverter
        // 'Skip's both. (We can't seed a duplicate-id row to drive
        // the converter's Fail path here because ID is the table's
        // PK; the converter's Fail-on-missing-id branch is covered
        // separately by OrganizationRowConverterTests.)
        await SeedOrganizationsAsync(keepAlive,
            ("ok-1", "Real Org", false),
            ("ok-2", null, false),    // converter Skip (no name)
            ("ok-3", "   ", false));  // converter Skip (whitespace name)

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() });

        var report = await runner.RunAsync(NewOptions(conn));

        var entity = report.Entities[0];
        Assert.AreEqual(3, entity.RowsRead);
        Assert.AreEqual(1, entity.RowsConverted);
        Assert.AreEqual(2, entity.RowsSkipped);
        Assert.AreEqual(0, entity.RowsFailed);
    }

    [TestMethod]
    public async Task RunAsync_KnownSchema_NoReaderForConverter_RecordsWarningAndContinues()
    {
        // No reader registered: the runner should still report the
        // entity (so the wizard sees it) with zero rows + a warning,
        // rather than silently dropping it or aborting the run.
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await SeedUpstreamSchemaAsync(keepAlive);

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() });
        // (single-arg readers overload omitted -> defaults to none)

        var report = await runner.RunAsync(NewOptions(conn));

        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, report.DetectedSchemaVersion);
        Assert.AreEqual(0, report.FatalErrors.Count);
        Assert.AreEqual(1, report.Entities.Count);
        var entity = report.Entities[0];
        Assert.AreEqual("Organization", entity.EntityName);
        Assert.AreEqual(0, entity.RowsRead);
        Assert.AreEqual(1, entity.Errors.Count);
        StringAssert.Contains(entity.Errors[0], "No legacy row reader is registered");
    }

    [TestMethod]
    public async Task RunAsync_KnownSchema_HonoursBatchSizeStreamingWithoutLoss()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await SeedUpstreamSchemaAsync(keepAlive);

        // 12 rows at batch size 5 forces 3 pages (5 + 5 + 2).
        var seed = Enumerable.Range(1, 12)
            .Select(i => ($"org-{i:D3}", (string?)$"Org {i}", false))
            .ToArray();
        await SeedOrganizationsAsync(keepAlive, seed);

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() });

        var report = await runner.RunAsync(NewOptions(conn, batchSize: 5));

        var entity = report.Entities[0];
        Assert.AreEqual(12, entity.RowsRead);
        Assert.AreEqual(12, entity.RowsConverted);
    }

    [TestMethod]
    public async Task RunAsync_Cancelled_DuringStream_PropagatesOperationCanceled()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await SeedUpstreamSchemaAsync(keepAlive);
        await SeedOrganizationsAsync(keepAlive, ("a", "A", false));

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() });

        using var cts = new CancellationTokenSource();
        cts.Cancel();

        await Assert.ThrowsExceptionAsync<OperationCanceledException>(
            () => runner.RunAsync(NewOptions(conn), cts.Token));
    }

    [TestMethod]
    public void Constructor_NullReaders_Throws()
    {
        Assert.ThrowsException<ArgumentNullException>(() =>
            new MigrationRunner(
                new LegacySchemaInspector(),
                Array.Empty<object>(),
                readers: null!));
    }

    private static MigrationOptions NewOptions(string conn, int batchSize = 500) => new()
    {
        SourceConnectionString = conn,
        TargetConnectionString = "Host=localhost;Database=cmremote_v2",
        DryRun = true,
        BatchSize = batchSize,
    };

    private static async Task SeedUpstreamSchemaAsync(SqliteConnection connection)
    {
        await ExecuteAsync(connection,
            "CREATE TABLE \"__EFMigrationsHistory\" (id TEXT);",
            "CREATE TABLE \"Organizations\" (" +
            "\"ID\" TEXT PRIMARY KEY, " +
            "\"OrganizationName\" TEXT, " +
            "\"IsDefaultOrganization\" INTEGER);",
            "CREATE TABLE Devices (ID TEXT);",
            "CREATE TABLE AspNetUsers (Id TEXT);");
    }

    private static async Task SeedOrganizationsAsync(
        SqliteConnection connection,
        params (string Id, string? Name, bool IsDefault)[] rows)
    {
        foreach (var (id, name, isDefault) in rows)
        {
            await using var cmd = connection.CreateCommand();
            cmd.CommandText =
                "INSERT INTO \"Organizations\" VALUES (@id, @name, @def);";
            cmd.Parameters.AddWithValue("@id", id);
            cmd.Parameters.AddWithValue("@name", (object?)name ?? DBNull.Value);
            cmd.Parameters.AddWithValue("@def", isDefault ? 1 : 0);
            await cmd.ExecuteNonQueryAsync();
        }
    }

    private static async Task ExecuteAsync(SqliteConnection connection, params string[] statements)
    {
        foreach (var sql in statements)
        {
            await using var cmd = connection.CreateCommand();
            cmd.CommandText = sql;
            await cmd.ExecuteNonQueryAsync();
        }
    }

    private static string NewSharedSqliteConnectionString()
    {
        var dbName = "cmremote-runner-int-" + Guid.NewGuid().ToString("N");
        return $"Data Source={dbName};Mode=Memory;Cache=Shared";
    }
}
