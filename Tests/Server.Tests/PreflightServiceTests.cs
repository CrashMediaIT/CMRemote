using System;
using System.IO;
using System.Threading.Tasks;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.Setup;

namespace Remotely.Server.Tests;

[TestClass]
public class PreflightServiceTests
{
    private string _tempDir = null!;

    [TestInitialize]
    public void Init()
    {
        _tempDir = Path.Combine(
            Path.GetTempPath(),
            "cmremote-preflight-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_tempDir);
    }

    [TestCleanup]
    public void Cleanup()
    {
        try
        {
            if (Directory.Exists(_tempDir)) Directory.Delete(_tempDir, recursive: true);
        }
        catch { /* best-effort */ }
    }

    private PreflightService BuildService(IConfiguration? config = null)
    {
        config ??= new ConfigurationBuilder().AddInMemoryCollection().Build();
        var path = Path.Combine(_tempDir, "appsettings.Production.json");
        var writer = new ConnectionStringWriter(
            path, config, NullLogger<ConnectionStringWriter>.Instance);
        return new PreflightService(
            writer, config, NullLogger<PreflightService>.Instance);
    }

    [TestMethod]
    public async Task RunChecks_WritableTempDir_PassesDataDirCheck()
    {
        var service = BuildService();
        var report = await service.RunChecksAsync();

        var dirCheck = FindCheck(report, "Writable data directory");
        Assert.AreEqual(PreflightStatus.Passed, dirCheck.Status,
            $"Expected the temp dir to be writable. Detail: {dirCheck.Detail}");
    }

    [TestMethod]
    public async Task RunChecks_NoTlsConfigured_TlsCheckIsWarning()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(new System.Collections.Generic.Dictionary<string, string?>
            {
                ["ASPNETCORE_URLS"] = "http://0.0.0.0:5000",
            })
            .Build();
        var service = BuildService(config);
        var report = await service.RunChecksAsync();

        var tls = FindCheck(report, "TLS endpoint configured");
        Assert.AreEqual(PreflightStatus.Warning, tls.Status,
            "An HTTP-only ASPNETCORE_URLS should produce a warning, not a pass or fail.");
        Assert.IsTrue(report.CanContinue,
            "A TLS warning is advisory; the operator must still be able to continue.");
    }

    [TestMethod]
    public async Task RunChecks_HttpsBinding_TlsCheckPasses()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(new System.Collections.Generic.Dictionary<string, string?>
            {
                ["ASPNETCORE_URLS"] = "http://0.0.0.0:5000;https://0.0.0.0:5001",
            })
            .Build();
        var service = BuildService(config);
        var report = await service.RunChecksAsync();

        Assert.AreEqual(PreflightStatus.Passed,
            FindCheck(report, "TLS endpoint configured").Status);
    }

    [TestMethod]
    public async Task RunChecks_KestrelEndpointHttps_TlsCheckPasses()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(new System.Collections.Generic.Dictionary<string, string?>
            {
                ["Kestrel:Endpoints:Web:Url"] = "https://0.0.0.0:443",
                ["ASPNETCORE_URLS"] = "http://0.0.0.0:5000",
            })
            .Build();
        var service = BuildService(config);
        var report = await service.RunChecksAsync();

        Assert.AreEqual(PreflightStatus.Passed,
            FindCheck(report, "TLS endpoint configured").Status);
    }

    [TestMethod]
    public async Task RunChecks_BindPortsCheck_SurfacesUrls()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(new System.Collections.Generic.Dictionary<string, string?>
            {
                ["ASPNETCORE_URLS"] = "https://0.0.0.0:5001",
            })
            .Build();
        var service = BuildService(config);
        var report = await service.RunChecksAsync();

        var ports = FindCheck(report, "Bind ports reachable");
        Assert.AreEqual(PreflightStatus.Passed, ports.Status);
        StringAssert.Contains(ports.Detail, "5001");
    }

    [TestMethod]
    public async Task RunChecks_CanContinue_IsTrueWhenNoFailures()
    {
        var service = BuildService();
        var report = await service.RunChecksAsync();
        Assert.IsTrue(report.CanContinue);
    }

    private static PreflightCheckResult FindCheck(PreflightReport report, string name)
    {
        foreach (var check in report.Checks)
        {
            if (check.Name == name)
            {
                return check;
            }
        }
        Assert.Fail($"Preflight report did not contain a check named '{name}'.");
        return null!; // unreachable
    }
}
