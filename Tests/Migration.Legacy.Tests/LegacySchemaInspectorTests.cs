using Microsoft.Data.Sqlite;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class LegacySchemaInspectorTests
{
    [TestMethod]
    public void Classify_NoTables_ReturnsEmpty()
    {
        var result = LegacySchemaInspector.Classify(Array.Empty<string>());
        Assert.AreEqual(LegacySchemaVersion.Empty, result);
    }

    [TestMethod]
    public void Classify_AllCanonicalTables_ReturnsUpstreamLegacy()
    {
        var result = LegacySchemaInspector.Classify(new[]
        {
            "__EFMigrationsHistory",
            "Organizations",
            "Devices",
            "AspNetUsers",
        });
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, result);
    }

    [TestMethod]
    public void Classify_AllCanonicalTablesPlusExtras_ReturnsUpstreamLegacy()
    {
        // Real upstream databases also contain Scripts, Alerts, etc.
        // Extras must not change the classification.
        var result = LegacySchemaInspector.Classify(new[]
        {
            "__EFMigrationsHistory",
            "Organizations",
            "Devices",
            "AspNetUsers",
            "Scripts",
            "Alerts",
            "SharedFiles",
        });
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, result);
    }

    [TestMethod]
    public void Classify_PartialCanonicalSet_ReturnsUnknown()
    {
        // Devices + AspNetUsers but no __EFMigrationsHistory /
        // Organizations — looks vaguely Remotely-shaped but isn't a
        // full upstream layout. Refuse to guess.
        var result = LegacySchemaInspector.Classify(new[]
        {
            "Devices",
            "AspNetUsers",
        });
        Assert.AreEqual(LegacySchemaVersion.Unknown, result);
    }

    [TestMethod]
    public void Classify_UnrelatedTables_ReturnsUnknown()
    {
        var result = LegacySchemaInspector.Classify(new[]
        {
            "Customers",
            "Orders",
            "Invoices",
        });
        Assert.AreEqual(LegacySchemaVersion.Unknown, result);
    }

    [TestMethod]
    public void Classify_IsCaseInsensitive()
    {
        // PostgreSQL folds unquoted identifiers to lower-case, so the
        // probe results may come back in a different case than the
        // canonical EF-generated names.
        var result = LegacySchemaInspector.Classify(new[]
        {
            "__efmigrationshistory",
            "organizations",
            "devices",
            "aspnetusers",
        });
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, result);
    }

    [TestMethod]
    public async Task DetectAsync_NullOrWhitespaceConnectionString_Throws()
    {
        var inspector = new LegacySchemaInspector();
        await Assert.ThrowsExceptionAsync<ArgumentException>(
            () => inspector.DetectAsync(""));
        await Assert.ThrowsExceptionAsync<ArgumentException>(
            () => inspector.DetectAsync("   "));
    }

    [TestMethod]
    public async Task DetectAsync_AlreadyCancelled_Throws()
    {
        var inspector = new LegacySchemaInspector();
        using var cts = new CancellationTokenSource();
        cts.Cancel();
        await Assert.ThrowsExceptionAsync<OperationCanceledException>(
            () => inspector.DetectAsync("Data Source=:memory:", cts.Token));
    }

    [TestMethod]
    public async Task DetectAsync_EmptySqliteDatabase_ReturnsEmpty()
    {
        // Use a shared in-memory SQLite database so the connection
        // the inspector opens sees the same (empty) schema we set up.
        var connString = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(connString);
        await keepAlive.OpenAsync();

        var inspector = new LegacySchemaInspector();
        var version = await inspector.DetectAsync(connString);

        Assert.AreEqual(LegacySchemaVersion.Empty, version);
    }

    [TestMethod]
    public async Task DetectAsync_SqliteWithCanonicalTables_ReturnsUpstreamLegacy()
    {
        var connString = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(connString);
        await keepAlive.OpenAsync();
        await ExecuteAsync(keepAlive,
            "CREATE TABLE \"__EFMigrationsHistory\" (id TEXT);",
            "CREATE TABLE Organizations (ID TEXT);",
            "CREATE TABLE Devices (ID TEXT);",
            "CREATE TABLE AspNetUsers (Id TEXT);");

        var inspector = new LegacySchemaInspector();
        var version = await inspector.DetectAsync(connString);

        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, version);
    }

    [TestMethod]
    public async Task DetectAsync_SqliteWithUnrelatedTables_ReturnsUnknown()
    {
        var connString = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(connString);
        await keepAlive.OpenAsync();
        await ExecuteAsync(keepAlive,
            "CREATE TABLE Customers (id TEXT);",
            "CREATE TABLE Orders (id TEXT);");

        var inspector = new LegacySchemaInspector();
        var version = await inspector.DetectAsync(connString);

        Assert.AreEqual(LegacySchemaVersion.Unknown, version);
    }

    [TestMethod]
    public async Task DetectAsync_SqliteIgnoresInternalTables()
    {
        // sqlite_sequence is created automatically when an
        // AUTOINCREMENT column exists. It must not be counted as a
        // user table, otherwise an otherwise-empty database would be
        // misclassified as Unknown.
        var connString = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(connString);
        await keepAlive.OpenAsync();
        await ExecuteAsync(keepAlive,
            "CREATE TABLE seeded (id INTEGER PRIMARY KEY AUTOINCREMENT);",
            "DROP TABLE seeded;");

        // sqlite_sequence is now present but no user tables remain.
        // The inspector should treat this as Empty.
        var inspector = new LegacySchemaInspector();
        var version = await inspector.DetectAsync(connString);

        Assert.AreEqual(LegacySchemaVersion.Empty, version);
    }

    [TestMethod]
    public async Task DetectAsync_UnrecognisedConnectionStringShape_Throws()
    {
        var inspector = new LegacySchemaInspector();
        await Assert.ThrowsExceptionAsync<NotSupportedException>(
            () => inspector.DetectAsync("Provider=SQLOLEDB;DataSrc=foo"));
    }

    [TestMethod]
    public async Task RunAsync_WithRealInspectorAgainstEmptyDb_ReportsEmpty()
    {
        // End-to-end: real inspector wired into the runner, against a
        // real (in-memory) SQLite DB. Ensures the runner's known/
        // empty/unknown branches still fire correctly when the
        // inspector is no longer a fake.
        var connString = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(connString);
        await keepAlive.OpenAsync();

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            Array.Empty<object>());

        var report = await runner.RunAsync(new MigrationOptions
        {
            SourceConnectionString = connString,
            TargetConnectionString = "Host=localhost;Database=cmremote_v2",
            DryRun = true,
        });

        Assert.AreEqual(LegacySchemaVersion.Empty, report.DetectedSchemaVersion);
        Assert.AreEqual(0, report.FatalErrors.Count);
        Assert.AreEqual(0, report.Entities.Count);
    }

    [TestMethod]
    public async Task RunAsync_WithRealInspectorAgainstUpstreamSchema_EnumeratesConverters()
    {
        var connString = NewSharedSqliteConnectionString();
        await using var keepAlive = new SqliteConnection(connString);
        await keepAlive.OpenAsync();
        await ExecuteAsync(keepAlive,
            "CREATE TABLE \"__EFMigrationsHistory\" (id TEXT);",
            "CREATE TABLE Organizations (ID TEXT);",
            "CREATE TABLE Devices (ID TEXT);",
            "CREATE TABLE AspNetUsers (Id TEXT);");

        var runner = new MigrationRunner(
            new LegacySchemaInspector(),
            new object[] { new Converters.OrganizationRowConverter() });

        var report = await runner.RunAsync(new MigrationOptions
        {
            SourceConnectionString = connString,
            TargetConnectionString = "Host=localhost;Database=cmremote_v2",
            DryRun = true,
        });

        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, report.DetectedSchemaVersion);
        Assert.AreEqual(0, report.FatalErrors.Count);
        Assert.AreEqual(1, report.Entities.Count);
        Assert.AreEqual("Organization", report.Entities[0].EntityName);
    }

    /// <summary>
    /// Returns a unique shared-cache in-memory SQLite connection
    /// string. Each test gets its own database identity so they can
    /// run in parallel without trampling each other; the shared
    /// cache lets the inspector's freshly-opened connection see the
    /// schema written by the test's keep-alive connection.
    /// </summary>
    private static string NewSharedSqliteConnectionString()
    {
        var dbName = "cmremote-test-" + Guid.NewGuid().ToString("N");
        return $"Data Source={dbName};Mode=Memory;Cache=Shared";
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
}
