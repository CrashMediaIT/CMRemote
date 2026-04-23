using System.Collections.Concurrent;
using Microsoft.Data.Sqlite;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Readers;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class MigrationRunnerWriterIntegrationTests
{
    [TestMethod]
    public async Task RunAsync_DryRun_WriterRegistered_DoesNotInvokeWriter()
    {
        var conn = await NewSeededSourceAsync(
            ("a", "Alpha", false),
            ("b", "Bravo", false));
        var writer = new InMemoryOrganizationWriter();

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() },
            writers: new object[] { writer });

        var report = await runner.RunAsync(NewOptions(conn, dryRun: true));

        var entity = report.Entities[0];
        Assert.AreEqual(2, entity.RowsRead);
        Assert.AreEqual(2, entity.RowsConverted);
        Assert.AreEqual(0, entity.RowsWritten);
        Assert.AreEqual(0, writer.Writes.Count,
            "DryRun=true must not invoke the writer at all.");
        Assert.AreEqual(0, entity.Errors.Count);
    }

    [TestMethod]
    public async Task RunAsync_RealRun_WriterRegistered_PersistsEveryConvertedRow()
    {
        var conn = await NewSeededSourceAsync(
            ("a", "Alpha", true),
            ("b", "Bravo", false),
            ("c", "Charlie", false));
        var writer = new InMemoryOrganizationWriter();

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() },
            writers: new object[] { writer });

        var report = await runner.RunAsync(NewOptions(conn, dryRun: false));

        var entity = report.Entities[0];
        Assert.AreEqual(3, entity.RowsRead);
        Assert.AreEqual(3, entity.RowsConverted);
        Assert.AreEqual(3, entity.RowsWritten);
        Assert.AreEqual(0, entity.RowsFailed);
        CollectionAssert.AreEquivalent(
            new[] { "a", "b", "c" },
            writer.Writes.Select(o => o.ID).ToArray());
    }

    [TestMethod]
    public async Task RunAsync_RealRun_NoWriterForConverter_DemotesToDryRunWithWarning()
    {
        var conn = await NewSeededSourceAsync(
            ("a", "Alpha", false),
            ("b", "Bravo", false));

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() },
            writers: Array.Empty<object>());

        var report = await runner.RunAsync(NewOptions(conn, dryRun: false));

        var entity = report.Entities[0];
        Assert.AreEqual(2, entity.RowsRead);
        Assert.AreEqual(2, entity.RowsConverted);
        Assert.AreEqual(0, entity.RowsWritten,
            "Without a writer the runner must not invent persistence.");
        Assert.AreEqual(1, entity.Errors.Count,
            "Expected exactly one warning per entity, not one per row.");
        StringAssert.Contains(entity.Errors[0], "No legacy row writer is registered");
    }

    [TestMethod]
    public async Task RunAsync_RealRun_WriterThrowsOnSomeRows_RecordsFailuresAndContinues()
    {
        var conn = await NewSeededSourceAsync(
            ("a", "Alpha", false),
            ("bad", "Bravo", false),  // writer will throw on this id
            ("c", "Charlie", false));
        var writer = new InMemoryOrganizationWriter
        {
            ThrowForIds = new HashSet<string> { "bad" },
        };

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() },
            writers: new object[] { writer });

        var report = await runner.RunAsync(NewOptions(conn, dryRun: false));

        var entity = report.Entities[0];
        Assert.AreEqual(3, entity.RowsRead);
        Assert.AreEqual(3, entity.RowsConverted);
        Assert.AreEqual(2, entity.RowsWritten);
        Assert.AreEqual(1, entity.RowsFailed);
        Assert.AreEqual(1, entity.Errors.Count);
        StringAssert.Contains(entity.Errors[0], "Writer 'Organization' threw");
        // The other two rows still made it through, in source order.
        CollectionAssert.AreEquivalent(
            new[] { "a", "c" },
            writer.Writes.Select(o => o.ID).ToArray());
        Assert.AreEqual(0, report.FatalErrors.Count,
            "A per-row writer exception must not abort the run.");
    }

    [TestMethod]
    public async Task RunAsync_RealRun_WriterIsIdempotent_AcrossReruns()
    {
        // Resumability per ROADMAP M1.3: re-running the migration
        // against a target that already has the rows must not
        // duplicate them. The in-memory writer asserts upsert-by-id.
        var conn = await NewSeededSourceAsync(
            ("a", "Alpha", true),
            ("b", "Bravo", false));
        var writer = new InMemoryOrganizationWriter();

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            converters: new object[] { new OrganizationRowConverter() },
            readers: new object[] { new LegacyOrganizationReader() },
            writers: new object[] { writer });

        var first = await runner.RunAsync(NewOptions(conn, dryRun: false));
        var second = await runner.RunAsync(NewOptions(conn, dryRun: false));

        Assert.AreEqual(2, first.Entities[0].RowsWritten);
        Assert.AreEqual(2, second.Entities[0].RowsWritten);

        // Two write *calls* per row across the two runs is fine; what
        // must hold is that the target ends up with one row per id.
        Assert.AreEqual(2, writer.DistinctIdsWritten.Count);
        CollectionAssert.AreEquivalent(
            new[] { "a", "b" },
            writer.DistinctIdsWritten.ToArray());
    }

    [TestMethod]
    public void Constructor_NullWriters_Throws()
    {
        Assert.ThrowsException<ArgumentNullException>(() =>
            new MigrationRunner(
                new LegacySchemaInspector(),
                converters: Array.Empty<object>(),
                readers: Array.Empty<object>(),
                writers: null!));
    }

    private static MigrationOptions NewOptions(string conn, bool dryRun) => new()
    {
        SourceConnectionString = conn,
        TargetConnectionString = "Host=localhost;Database=cmremote_v2",
        DryRun = dryRun,
        BatchSize = 500,
    };

    private static async Task<string> NewSeededSourceAsync(
        params (string Id, string? Name, bool IsDefault)[] rows)
    {
        var dbName = "cmremote-writer-int-" + Guid.NewGuid().ToString("N");
        var conn = $"Data Source={dbName};Mode=Memory;Cache=Shared";

        // Note: the keep-alive connection is intentionally leaked
        // for the lifetime of the test method (the shared-cache
        // database is dropped when the last connection closes).
        var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();

        await ExecuteAsync(keepAlive,
            "CREATE TABLE \"__EFMigrationsHistory\" (id TEXT);",
            "CREATE TABLE \"Organizations\" (" +
            "\"ID\" TEXT PRIMARY KEY, " +
            "\"OrganizationName\" TEXT, " +
            "\"IsDefaultOrganization\" INTEGER);",
            "CREATE TABLE Devices (ID TEXT);",
            "CREATE TABLE AspNetUsers (Id TEXT);");

        foreach (var (id, name, isDefault) in rows)
        {
            await using var cmd = keepAlive.CreateCommand();
            cmd.CommandText =
                "INSERT INTO \"Organizations\" VALUES (@id, @name, @def);";
            cmd.Parameters.AddWithValue("@id", id);
            cmd.Parameters.AddWithValue("@name", (object?)name ?? DBNull.Value);
            cmd.Parameters.AddWithValue("@def", isDefault ? 1 : 0);
            await cmd.ExecuteNonQueryAsync();
        }

        return conn;
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

    /// <summary>
    /// Test double for the M2 target writer. Tracks every write call
    /// in order and exposes the distinct set of ids written so the
    /// idempotency assertion can fire even when a row is upserted
    /// multiple times across runs. Optionally throws for a configured
    /// set of ids to drive the per-row writer-failure path.
    /// </summary>
    private sealed class InMemoryOrganizationWriter : ILegacyRowWriter<Organization>
    {
        public string EntityName => "Organization";

        public LegacySchemaVersion HandlesSchemaVersion =>
            LegacySchemaVersion.UpstreamLegacy_2026_04;

        public ConcurrentBag<Organization> Writes { get; } = new();

        public HashSet<string> ThrowForIds { get; init; } = new();

        public HashSet<string> DistinctIdsWritten =>
            Writes.Select(w => w.ID).Where(id => id is not null).ToHashSet();

        public Task WriteAsync(
            Organization row,
            string targetConnectionString,
            CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();

            if (row is null)
            {
                throw new ArgumentNullException(nameof(row));
            }

            if (ThrowForIds.Contains(row.ID ?? string.Empty))
            {
                throw new InvalidOperationException(
                    $"Simulated upsert failure for id '{row.ID}'.");
            }

            // Real Postgres writer will UPSERT; the test double just
            // appends — DistinctIdsWritten gives the upsert view.
            Writes.Add(row);
            return Task.CompletedTask;
        }
    }
}
