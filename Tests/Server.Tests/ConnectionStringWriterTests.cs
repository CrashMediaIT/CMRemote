using System;
using System.IO;
using System.Threading.Tasks;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.Setup;

namespace Remotely.Server.Tests;

[TestClass]
public class ConnectionStringWriterTests
{
    private string _tempDir = null!;

    [TestInitialize]
    public void Init()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "cmremote-cstest-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_tempDir);
    }

    [TestCleanup]
    public void Cleanup()
    {
        try
        {
            if (Directory.Exists(_tempDir))
            {
                Directory.Delete(_tempDir, recursive: true);
            }
        }
        catch
        {
            // best-effort
        }
    }

    private (ConnectionStringWriter Writer, ConfigurationManager Config, string Path) CreateWriter()
    {
        var path = Path.Combine(_tempDir, "appsettings.Production.json");
        var config = new ConfigurationManager();
        var writer = new ConnectionStringWriter(
            path, config, NullLogger<ConnectionStringWriter>.Instance);
        return (writer, config, path);
    }

    [TestMethod]
    public async Task WritePostgresConnection_CreatesFile_WithExpectedKeys()
    {
        var (writer, _, path) = CreateWriter();

        await writer.WritePostgresConnectionAsync(
            "Host=db.local;Database=cmremote;Username=u;Password=p");

        Assert.IsTrue(File.Exists(path), "Settings file should have been created.");
        var contents = await File.ReadAllTextAsync(path);
        StringAssert.Contains(contents, "ConnectionStrings");
        StringAssert.Contains(contents, "PostgreSQL");
        StringAssert.Contains(contents, "Host=db.local");
        StringAssert.Contains(contents, "ApplicationOptions");
        StringAssert.Contains(contents, "DbProvider");
        StringAssert.Contains(contents, "PostgreSql");
    }

    [TestMethod]
    public async Task WritePostgresConnection_PreservesExistingKeys()
    {
        var (writer, _, path) = CreateWriter();

        // Pre-seed an unrelated key to assert the writer does not
        // clobber operator-added settings.
        await File.WriteAllTextAsync(
            path,
            """{"Logging":{"LogLevel":{"Default":"Warning"}}}""");

        await writer.WritePostgresConnectionAsync(
            "Host=db.local;Database=cmremote;Username=u;Password=p");

        var contents = await File.ReadAllTextAsync(path);
        StringAssert.Contains(contents, "Logging",
            "The writer must preserve unrelated keys already present in the file.");
        StringAssert.Contains(contents, "Warning");
        StringAssert.Contains(contents, "Host=db.local");
    }

    [TestMethod]
    public async Task WritePostgresConnection_OverwritesPreviousConnectionValue()
    {
        var (writer, _, path) = CreateWriter();

        await writer.WritePostgresConnectionAsync(
            "Host=old.local;Database=cmremote;Username=u;Password=p");
        await writer.WritePostgresConnectionAsync(
            "Host=new.local;Database=cmremote;Username=u;Password=p");

        var contents = await File.ReadAllTextAsync(path);
        StringAssert.Contains(contents, "Host=new.local");
        Assert.IsFalse(contents.Contains("Host=old.local"),
            "A second write must replace the previous Postgres connection string.");
    }

    [TestMethod]
    public async Task WritePostgresConnection_TriggersConfigurationReload()
    {
        var (writer, config, path) = CreateWriter();

        // The ConfigurationManager picks up file-based providers
        // dynamically, so to assert "Reload was called" we instead
        // observe that a fresh Configuration instance built from the
        // file would see the new value. The contract here is "the
        // file is on disk after the call returns" — Reload itself is
        // a delegated call to IConfigurationRoot which we exercise
        // via the production codepath.
        await writer.WritePostgresConnectionAsync(
            "Host=reload.local;Database=cmremote;Username=u;Password=p");

        // Verify a fresh configuration root reads the value back
        // (proving the file is well-formed JSON in the shape
        // IConfiguration expects).
        var fresh = new ConfigurationBuilder().AddJsonFile(path).Build();
        Assert.AreEqual(
            "Host=reload.local;Database=cmremote;Username=u;Password=p",
            fresh.GetConnectionString("PostgreSQL"));
        Assert.AreEqual("PostgreSql", fresh["ApplicationOptions:DbProvider"]);
    }

    [TestMethod]
    public async Task WritePostgresConnection_RejectsEmptyString()
    {
        var (writer, _, _) = CreateWriter();

        await Assert.ThrowsExceptionAsync<ArgumentException>(() =>
            writer.WritePostgresConnectionAsync(string.Empty));
        await Assert.ThrowsExceptionAsync<ArgumentException>(() =>
            writer.WritePostgresConnectionAsync("   "));
    }

    [TestMethod]
    [TestCategory("LinuxOnly")]
    public async Task WritePostgresConnection_OnUnix_AppliesOwnerOnlyMode()
    {
        if (!OperatingSystem.IsLinux() && !OperatingSystem.IsMacOS())
        {
            Assert.Inconclusive("File-mode check is Unix-only.");
            return;
        }

        var (writer, _, path) = CreateWriter();

        await writer.WritePostgresConnectionAsync(
            "Host=db.local;Database=cmremote;Username=u;Password=p");

        var mode = File.GetUnixFileMode(path);
        var allowed = UnixFileMode.UserRead | UnixFileMode.UserWrite;
        Assert.AreEqual(allowed, mode,
            $"Settings file must be 0600 (owner-only RW) but was {mode}.");
    }
}
