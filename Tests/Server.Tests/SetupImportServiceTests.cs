using System;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.Setup;

namespace Remotely.Server.Tests;

/// <summary>
/// End-to-end smoke tests for the wizard's M1.3 import service. Drive
/// the same SQLite-seed pattern the Migration.Cli smoke suite uses so
/// the wizard exercises the full converter / reader / (skipped)
/// writer chain without needing a live Postgres target in CI.
/// </summary>
[TestClass]
public class SetupImportServiceTests
{
    private string _tempDir = null!;
    private SqliteConnection? _keepAlive;

    [TestInitialize]
    public void Init()
    {
        _tempDir = Path.Combine(
            Path.GetTempPath(),
            "cmremote-import-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_tempDir);
    }

    [TestCleanup]
    public void Cleanup()
    {
        try { _keepAlive?.Dispose(); } catch { }
        try
        {
            if (Directory.Exists(_tempDir)) Directory.Delete(_tempDir, recursive: true);
        }
        catch { /* best-effort */ }
    }

    private SetupImportService BuildService()
    {
        var settingsPath = Path.Combine(_tempDir, "appsettings.Production.json");
        var config = new ConfigurationManager();
        var writer = new ConnectionStringWriter(
            settingsPath, config, NullLogger<ConnectionStringWriter>.Instance);
        return new SetupImportService(
            writer,
            NullLoggerFactory.Instance,
            NullLogger<SetupImportService>.Instance);
    }

    [TestMethod]
    public async Task DetectSource_EmptyConnectionString_ReportsFatal()
    {
        var service = BuildService();

        var report = await service.DetectSourceAsync(string.Empty);

        Assert.IsTrue(report.HadFatalErrors,
            "An empty source connection string must surface as a fatal error.");
        Assert.AreEqual(0, report.Entities.Count);
    }

    [TestMethod]
    public async Task DetectSource_AgainstSeededSqlite_DetectsUpstreamLegacy()
    {
        var service = BuildService();
        var sourceConn = await NewSeededUpstreamSourceAsync();

        var report = await service.DetectSourceAsync(sourceConn);

        Assert.IsFalse(report.HadFatalErrors,
            $"Detect should succeed against a seeded source. Errors: {string.Join("; ", report.FatalErrors)}");
        // The exact enum value lives inside the aliased
        // Migration.Legacy assembly; assert on the stringified
        // version that the wizard surface uses.
        StringAssert.Contains(report.DetectedSchemaVersion, "Upstream");
    }

    [TestMethod]
    public async Task RunImport_DryRun_AgainstSeededSqlite_ProducesEntityReports()
    {
        var service = BuildService();
        var sourceConn = await NewSeededUpstreamSourceAsync();

        var report = await service.RunImportAsync(
            sourceConn,
            // DryRun=true never opens the target; use a syntactically
            // valid Postgres string so the runner's option validator
            // accepts it.
            "Host=localhost;Database=cmremote_v2",
            dryRun: true);

        Assert.IsFalse(report.HadFatalErrors,
            $"Dry-run should succeed. Errors: {string.Join("; ", report.FatalErrors)}");
        Assert.IsTrue(report.DryRun);

        var entities = report.Entities.ToDictionary(e => e.EntityName);
        Assert.IsTrue(entities.ContainsKey("Organization"));
        Assert.IsTrue(entities.ContainsKey("Device"));
        Assert.IsTrue(entities.ContainsKey("User"));

        Assert.AreEqual(2, entities["Organization"].RowsRead);
        Assert.AreEqual(2, entities["Organization"].RowsConverted);
        Assert.AreEqual(0, entities["Organization"].RowsWritten,
            "DryRun must not write any rows.");

        Assert.AreEqual(1, entities["Device"].RowsRead);
        Assert.AreEqual(1, entities["User"].RowsRead);
    }

    [TestMethod]
    public async Task RunImport_DryRun_PersistsMigrationReportJson()
    {
        var service = BuildService();
        var sourceConn = await NewSeededUpstreamSourceAsync();

        await service.RunImportAsync(
            sourceConn,
            "Host=localhost;Database=cmremote_v2",
            dryRun: true);

        var reportPath = Path.Combine(_tempDir, "migration-report.json");
        Assert.IsTrue(File.Exists(reportPath),
            "RunImportAsync must persist a migration-report.json artefact next to settings.");
        var contents = await File.ReadAllTextAsync(reportPath);
        StringAssert.Contains(contents, "Organization");
        StringAssert.Contains(contents, "DryRun");
    }

    [TestMethod]
    public async Task RunImport_EmptySourceConnectionString_Throws()
    {
        var service = BuildService();
        await Assert.ThrowsExceptionAsync<ArgumentException>(() =>
            service.RunImportAsync(
                string.Empty,
                "Host=localhost;Database=cmremote_v2",
                dryRun: true));
    }

    [TestMethod]
    public async Task RunImport_EmptyTargetConnectionString_Throws()
    {
        var service = BuildService();
        await Assert.ThrowsExceptionAsync<ArgumentException>(() =>
            service.RunImportAsync(
                "Data Source=foo",
                string.Empty,
                dryRun: true));
    }

    /// <summary>
    /// Mirrors the Migration.Cli smoke-test seed exactly so the
    /// wizard and CLI paths exercise the same source shape.
    /// </summary>
    private async Task<string> NewSeededUpstreamSourceAsync()
    {
        var dbName = "cmremote-server-importtest-" + Guid.NewGuid().ToString("N");
        var conn = $"Data Source={dbName};Mode=Memory;Cache=Shared";

        // Hold the connection open for the lifetime of the test so
        // SQLite's shared-cache in-memory db is not torn down between
        // the seed and the call under test.
        _keepAlive = new SqliteConnection(conn);
        await _keepAlive.OpenAsync();

        await Exec(_keepAlive,
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

        await Exec(_keepAlive,
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
