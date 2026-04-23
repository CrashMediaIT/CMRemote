using Microsoft.Data.Sqlite;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy.Readers;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class LegacyDeviceReaderTests
{
    [TestMethod]
    public void EntityName_AndHandlesSchemaVersion_AreStable()
    {
        var r = new LegacyDeviceReader();
        Assert.AreEqual("Device", r.EntityName);
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, r.HandlesSchemaVersion);
    }

    [TestMethod]
    [DataRow(null)]
    [DataRow("")]
    [DataRow("   ")]
    public async Task ReadAsync_BlankConnString_Throws(string? conn)
    {
        var reader = new LegacyDeviceReader();
        await Assert.ThrowsExceptionAsync<ArgumentException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync(conn!, 100))
            {
            }
        });
    }

    [TestMethod]
    public async Task ReadAsync_NonPositiveBatch_Throws()
    {
        var reader = new LegacyDeviceReader();
        await Assert.ThrowsExceptionAsync<ArgumentOutOfRangeException>(async () =>
        {
            await foreach (var _ in reader.ReadAsync("Data Source=:memory:", 0))
            {
            }
        });
    }

    [TestMethod]
    public async Task ReadAsync_KeysetPaginates_AcrossMultiplePages()
    {
        var conn = await NewSeededDeviceSourceAsync(rowCount: 12);
        var reader = new LegacyDeviceReader();
        var seen = new List<string>();
        await foreach (var row in reader.ReadAsync(conn, batchSize: 5))
        {
            seen.Add(row.ID);
        }

        Assert.AreEqual(12, seen.Count);
        // Order is by ID ascending; padding the integer index to 3
        // chars guarantees lexicographic == numeric order.
        var expected = Enumerable.Range(0, 12)
            .Select(i => $"d-{i:D3}")
            .ToArray();
        CollectionAssert.AreEqual(expected, seen);
    }

    [TestMethod]
    public async Task ReadAsync_PopulatesScalarFields()
    {
        var dbName = "cmremote-dev-read-" + Guid.NewGuid().ToString("N");
        var conn = $"Data Source={dbName};Mode=Memory;Cache=Shared";
        var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();

        await Exec(keepAlive,
            // Column types are deliberately permissive — SQLite is
            // dynamically typed, the reader must tolerate whatever
            // the upstream EF migration laid down.
            """
            CREATE TABLE "Devices" (
                "ID" TEXT PRIMARY KEY,
                "OrganizationID" TEXT,
                "DeviceName" TEXT,
                "Alias" TEXT,
                "Tags" TEXT,
                "Notes" TEXT,
                "Platform" TEXT,
                "OSDescription" TEXT,
                "AgentVersion" TEXT,
                "CurrentUser" TEXT,
                "PublicIP" TEXT,
                "DeviceGroupID" TEXT,
                "ServerVerificationToken" TEXT,
                "Is64Bit" INTEGER,
                "IsOnline" INTEGER,
                "LastOnline" TEXT,
                "ProcessorCount" INTEGER,
                "CpuUtilization" REAL,
                "TotalMemory" REAL,
                "UsedMemory" REAL,
                "TotalStorage" REAL,
                "UsedStorage" REAL,
                "OSArchitecture" INTEGER);
            """);

        await using (var cmd = keepAlive.CreateCommand())
        {
            cmd.CommandText = """
                INSERT INTO "Devices" VALUES (
                    'd-0', 'o-1', 'host', 'a', 't', 'n',
                    'Linux', 'Ubuntu 22', '1.2.3', 'root', '1.2.3.4',
                    'g-1', 'tok',
                    1, 1, '2025-01-02T03:04:05+00:00',
                    8, 0.5, 16.0, 4.0, 100.0, 50.0, 9);
                """;
            await cmd.ExecuteNonQueryAsync();
        }

        var reader = new LegacyDeviceReader();
        var rows = new List<LegacyDevice>();
        await foreach (var row in reader.ReadAsync(conn, 10))
        {
            rows.Add(row);
        }

        Assert.AreEqual(1, rows.Count);
        var d = rows[0];
        Assert.AreEqual("d-0", d.ID);
        Assert.AreEqual("o-1", d.OrganizationID);
        Assert.AreEqual("host", d.DeviceName);
        Assert.AreEqual("Linux", d.Platform);
        Assert.AreEqual("1.2.3", d.AgentVersion);
        Assert.IsTrue(d.Is64Bit);
        Assert.IsTrue(d.IsOnline);
        Assert.AreEqual(8, d.ProcessorCount);
        Assert.AreEqual(16.0, d.TotalMemory);
        Assert.AreEqual(9, d.OSArchitecture);
        Assert.AreEqual(
            new DateTimeOffset(2025, 1, 2, 3, 4, 5, TimeSpan.Zero),
            d.LastOnline);
    }

    private static async Task<string> NewSeededDeviceSourceAsync(int rowCount)
    {
        var dbName = "cmremote-dev-page-" + Guid.NewGuid().ToString("N");
        var conn = $"Data Source={dbName};Mode=Memory;Cache=Shared";

        var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();

        await Exec(keepAlive,
            """
            CREATE TABLE "Devices" (
                "ID" TEXT PRIMARY KEY,
                "OrganizationID" TEXT,
                "DeviceName" TEXT,
                "Alias" TEXT,
                "Tags" TEXT,
                "Notes" TEXT,
                "Platform" TEXT,
                "OSDescription" TEXT,
                "AgentVersion" TEXT,
                "CurrentUser" TEXT,
                "PublicIP" TEXT,
                "DeviceGroupID" TEXT,
                "ServerVerificationToken" TEXT,
                "Is64Bit" INTEGER,
                "IsOnline" INTEGER,
                "LastOnline" TEXT,
                "ProcessorCount" INTEGER,
                "CpuUtilization" REAL,
                "TotalMemory" REAL,
                "UsedMemory" REAL,
                "TotalStorage" REAL,
                "UsedStorage" REAL,
                "OSArchitecture" INTEGER);
            """);

        for (var i = 0; i < rowCount; i++)
        {
            await using var cmd = keepAlive.CreateCommand();
            cmd.CommandText = """
                INSERT INTO "Devices" ("ID", "OrganizationID", "Is64Bit", "IsOnline", "LastOnline")
                VALUES (@id, 'o-1', 0, 0, '2024-01-01T00:00:00+00:00');
                """;
            cmd.Parameters.AddWithValue("@id", $"d-{i:D3}");
            await cmd.ExecuteNonQueryAsync();
        }

        return conn;
    }

    private static async Task Exec(SqliteConnection connection, string sql)
    {
        await using var cmd = connection.CreateCommand();
        cmd.CommandText = sql;
        await cmd.ExecuteNonQueryAsync();
    }
}
