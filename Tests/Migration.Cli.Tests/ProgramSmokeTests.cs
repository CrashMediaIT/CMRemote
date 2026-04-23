using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Cli;
using Remotely.Migration.Legacy;

namespace Remotely.Migration.Cli.Tests;

/// <summary>
/// End-to-end smoke test for the CLI's runner composition. Drives a
/// dry-run against a SQLite source seeded with the canonical
/// upstream tables so the runner walks the full converter / reader /
/// (skipped) writer chain without actually needing a Postgres
/// target available in CI.
/// </summary>
[TestClass]
public class ProgramSmokeTests
{
    [TestMethod]
    public async Task BuildRunner_DryRun_AgainstCanonicalSqliteSource_ProducesReport()
    {
        var sourceConn = await NewSeededUpstreamSourceAsync();

        var runner = Program.BuildRunner(NullLoggerFactory.Instance);

        var report = await runner.RunAsync(new MigrationOptions
        {
            SourceConnectionString = sourceConn,
            // Postgres connection string shape so the writer's
            // validator accepts it — the runner won't actually call
            // the writer because DryRun=true.
            TargetConnectionString = "Host=localhost;Database=cmremote_v2",
            DryRun = true,
            BatchSize = 50,
        });

        Assert.AreEqual(0, report.FatalErrors.Count, string.Join("; ", report.FatalErrors));
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, report.DetectedSchemaVersion);

        // All three M2 entities show up in the report (Org / Device / User),
        // each with the seeded row count and zero RowsWritten (DryRun).
        var byName = report.Entities.ToDictionary(e => e.EntityName);
        Assert.IsTrue(byName.ContainsKey("Organization"));
        Assert.IsTrue(byName.ContainsKey("Device"));
        Assert.IsTrue(byName.ContainsKey("User"));

        Assert.AreEqual(2, byName["Organization"].RowsRead);
        Assert.AreEqual(2, byName["Organization"].RowsConverted);
        Assert.AreEqual(0, byName["Organization"].RowsWritten);

        Assert.AreEqual(1, byName["Device"].RowsRead);
        Assert.AreEqual(1, byName["Device"].RowsConverted);
        Assert.AreEqual(0, byName["Device"].RowsWritten);

        Assert.AreEqual(1, byName["User"].RowsRead);
        Assert.AreEqual(1, byName["User"].RowsConverted);
        Assert.AreEqual(0, byName["User"].RowsWritten);

        Assert.AreEqual(0, Program.ComputeExitCode(report));
    }

    [TestMethod]
    public void PrintReport_FormatsKnownColumns()
    {
        // Pin the report's plain-text format because operators are
        // expected to grep / paste it from a shell — silently
        // changing column headers would break their muscle memory.
        var report = new MigrationReport
        {
            DetectedSchemaVersion = LegacySchemaVersion.UpstreamLegacy_2026_04,
            Entities =
            {
                new EntityReport
                {
                    EntityName = "Organization",
                    RowsRead = 3,
                    RowsConverted = 3,
                    RowsWritten = 3,
                },
            },
        };

        using var sw = new StringWriter();
        Program.PrintReport(report, sw);
        var output = sw.ToString();

        StringAssert.Contains(output, "=== Migration Report ===");
        StringAssert.Contains(output, "Read");
        StringAssert.Contains(output, "Conv");
        StringAssert.Contains(output, "Skip");
        StringAssert.Contains(output, "Fail");
        StringAssert.Contains(output, "Wrote");
        StringAssert.Contains(output, "Organization");
    }

    private static async Task<string> NewSeededUpstreamSourceAsync()
    {
        var dbName = "cmremote-cli-smoke-" + Guid.NewGuid().ToString("N");
        var conn = $"Data Source={dbName};Mode=Memory;Cache=Shared";

        var keepAlive = new SqliteConnection(conn);
        await keepAlive.OpenAsync();

        await Exec(keepAlive,
            // The four canonical tables the inspector probes for.
            // Devices + AspNetUsers schemas only need the columns
            // the readers actually project.
            "CREATE TABLE \"__EFMigrationsHistory\" (id TEXT);",
            """
            CREATE TABLE "Organizations" (
                "ID" TEXT PRIMARY KEY,
                "OrganizationName" TEXT,
                "IsDefaultOrganization" INTEGER);
            """,
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
            """,
            """
            CREATE TABLE "AspNetUsers" (
                "Id" TEXT PRIMARY KEY,
                "UserName" TEXT,
                "NormalizedUserName" TEXT,
                "Email" TEXT,
                "NormalizedEmail" TEXT,
                "EmailConfirmed" INTEGER,
                "PasswordHash" TEXT,
                "SecurityStamp" TEXT,
                "ConcurrencyStamp" TEXT,
                "PhoneNumber" TEXT,
                "PhoneNumberConfirmed" INTEGER,
                "TwoFactorEnabled" INTEGER,
                "LockoutEnd" TEXT,
                "LockoutEnabled" INTEGER,
                "AccessFailedCount" INTEGER,
                "OrganizationID" TEXT,
                "IsAdministrator" INTEGER,
                "IsServerAdmin" INTEGER);
            """);

        await Exec(keepAlive,
            "INSERT INTO \"Organizations\" VALUES ('o-1', 'Acme', 1);",
            "INSERT INTO \"Organizations\" VALUES ('o-2', 'Widget', 0);",
            """
            INSERT INTO "Devices" VALUES (
                'd-1', 'o-1', 'host', NULL, NULL, NULL, NULL, NULL, NULL,
                NULL, NULL, NULL, NULL, 1, 1, '2025-01-01T00:00:00+00:00',
                4, 0.1, 8.0, 1.0, 100.0, 50.0, 0);
            """,
            """
            INSERT INTO "AspNetUsers" VALUES (
                'u-1', 'alice', 'ALICE', 'alice@example.com', 'ALICE@EXAMPLE.COM',
                1, 'pwhash', 'stamp', 'cc', NULL, 0, 0,
                NULL, 1, 0, 'o-1', 1, 0);
            """);

        return conn;
    }

    private static async Task Exec(SqliteConnection connection, params string[] sqls)
    {
        foreach (var sql in sqls)
        {
            await using var cmd = connection.CreateCommand();
            cmd.CommandText = sql;
            await cmd.ExecuteNonQueryAsync();
        }
    }
}
