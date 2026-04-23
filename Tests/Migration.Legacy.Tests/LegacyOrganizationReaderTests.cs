using Microsoft.Data.Sqlite;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;
using Remotely.Migration.Legacy.Readers;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class LegacyOrganizationReaderTests
{
    [TestMethod]
    public void EntityNameAndHandlesSchemaVersion_ArePinned()
    {
        var reader = new LegacyOrganizationReader();

        Assert.AreEqual("Organization", reader.EntityName);
        Assert.AreEqual(
            LegacySchemaVersion.UpstreamLegacy_2026_04,
            reader.HandlesSchemaVersion);
    }

    [TestMethod]
    public async Task ReadAsync_NullOrWhitespaceConnectionString_Throws()
    {
        var reader = new LegacyOrganizationReader();

        await Assert.ThrowsExceptionAsync<ArgumentException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync("", batchSize: 10)) { }
        });

        await Assert.ThrowsExceptionAsync<ArgumentException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync("   ", batchSize: 10)) { }
        });
    }

    [TestMethod]
    public async Task ReadAsync_NonPositiveBatchSize_Throws()
    {
        var reader = new LegacyOrganizationReader();

        await Assert.ThrowsExceptionAsync<ArgumentOutOfRangeException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync("Data Source=:memory:", batchSize: 0)) { }
        });

        await Assert.ThrowsExceptionAsync<ArgumentOutOfRangeException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync("Data Source=:memory:", batchSize: -1)) { }
        });
    }

    [TestMethod]
    public async Task ReadAsync_UnsupportedConnectionStringShape_Throws()
    {
        var reader = new LegacyOrganizationReader();

        await Assert.ThrowsExceptionAsync<NotSupportedException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync("Provider=SQLOLEDB;DataSrc=foo", 10)) { }
        });
    }

    [TestMethod]
    public async Task ReadAsync_EmptyTable_YieldsNothing()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await ExecuteAsync(keepAlive,
            "CREATE TABLE \"Organizations\" (" +
            "\"ID\" TEXT PRIMARY KEY, " +
            "\"OrganizationName\" TEXT, " +
            "\"IsDefaultOrganization\" INTEGER);");

        var reader = new LegacyOrganizationReader();
        var rows = await CollectAsync(reader, conn, batchSize: 10);

        Assert.AreEqual(0, rows.Count);
    }

    [TestMethod]
    public async Task ReadAsync_SinglePage_ReturnsAllRowsOrderedById()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await CreateAndSeedOrganizationsAsync(keepAlive,
            ("c", "Charlie Org", false),
            ("a", "Alpha Org", true),
            ("b", "Bravo Org", false));

        var reader = new LegacyOrganizationReader();
        var rows = await CollectAsync(reader, conn, batchSize: 100);

        CollectionAssert.AreEqual(
            new[] { "a", "b", "c" },
            rows.Select(r => r.ID).ToArray());

        Assert.AreEqual("Alpha Org", rows[0].OrganizationName);
        Assert.IsTrue(rows[0].IsDefaultOrganization);
        Assert.IsFalse(rows[1].IsDefaultOrganization);
    }

    [TestMethod]
    public async Task ReadAsync_MultiplePages_StreamsAllRowsExactlyOnce()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();

        // Seed 25 rows with sortable string ids ("org-001" .. "org-025")
        // so keyset pagination is well-defined.
        var seed = Enumerable.Range(1, 25)
            .Select(i => ($"org-{i:D3}", $"Org {i}", false))
            .ToArray();
        await CreateAndSeedOrganizationsAsync(keepAlive, seed);

        var reader = new LegacyOrganizationReader();
        var rows = await CollectAsync(reader, conn, batchSize: 7);

        Assert.AreEqual(25, rows.Count);
        CollectionAssert.AreEqual(
            seed.Select(s => s.Item1).ToArray(),
            rows.Select(r => r.ID).ToArray());
    }

    [TestMethod]
    public async Task ReadAsync_NullOrganizationName_PassesThroughAsNull()
    {
        // The reader is a dumb POCO populator; null-name handling is
        // the converter's job (it 'Skip's such rows). The reader must
        // not invent a fallback string or throw.
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await ExecuteAsync(keepAlive,
            "CREATE TABLE \"Organizations\" (" +
            "\"ID\" TEXT PRIMARY KEY, " +
            "\"OrganizationName\" TEXT, " +
            "\"IsDefaultOrganization\" INTEGER);",
            "INSERT INTO \"Organizations\" VALUES ('x', NULL, 0);");

        var reader = new LegacyOrganizationReader();
        var rows = await CollectAsync(reader, conn, batchSize: 10);

        Assert.AreEqual(1, rows.Count);
        Assert.AreEqual("x", rows[0].ID);
        Assert.IsNull(rows[0].OrganizationName);
    }

    [TestMethod]
    public async Task ReadAsync_AlreadyCancelled_Throws()
    {
        var conn = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();
        await CreateAndSeedOrganizationsAsync(keepAlive, ("a", "A", false));

        var reader = new LegacyOrganizationReader();
        using var cts = new CancellationTokenSource();
        cts.Cancel();

        await Assert.ThrowsExceptionAsync<OperationCanceledException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync(conn, batchSize: 10, cts.Token)) { }
        });
    }

    private static async Task<List<LegacyOrganization>> CollectAsync(
        ILegacyRowReader<LegacyOrganization> reader,
        string connectionString,
        int batchSize)
    {
        var collected = new List<LegacyOrganization>();
        await foreach (var row in reader.ReadAsync(connectionString, batchSize))
        {
            collected.Add(row);
        }
        return collected;
    }

    private static async Task CreateAndSeedOrganizationsAsync(
        SqliteConnection connection,
        params (string Id, string Name, bool IsDefault)[] rows)
    {
        await ExecuteAsync(connection,
            "CREATE TABLE \"Organizations\" (" +
            "\"ID\" TEXT PRIMARY KEY, " +
            "\"OrganizationName\" TEXT, " +
            "\"IsDefaultOrganization\" INTEGER);");

        foreach (var (id, name, isDefault) in rows)
        {
            await using var cmd = connection.CreateCommand();
            cmd.CommandText =
                "INSERT INTO \"Organizations\" VALUES (@id, @name, @def);";
            cmd.Parameters.AddWithValue("@id", id);
            cmd.Parameters.AddWithValue("@name", name);
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
        var dbName = "cmremote-reader-test-" + Guid.NewGuid().ToString("N");
        return $"Data Source={dbName};Mode=Memory;Cache=Shared";
    }
}
