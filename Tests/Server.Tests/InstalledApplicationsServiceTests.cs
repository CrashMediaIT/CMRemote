using Microsoft.Extensions.Caching.Memory;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services;
using Remotely.Shared.Enums;
using Remotely.Shared.Models;
using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class InstalledApplicationsServiceTests
{
    private TestData _testData = null!;
    private IInstalledApplicationsService _service = null!;
    private IMemoryCache _memoryCache = null!;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();

        _memoryCache = new MemoryCache(new MemoryCacheOptions());
        var dbFactory = IoCActivator.ServiceProvider.GetRequiredService<Remotely.Server.Data.IAppDbFactory>();
        _service = new InstalledApplicationsService(dbFactory, _memoryCache);
    }

    [TestCleanup]
    public void Cleanup()
    {
        _memoryCache.Dispose();
    }

    [TestMethod]
    public async Task SaveAndGetSnapshot_RoundTripsApplications()
    {
        var apps = new List<InstalledApplication>
        {
            new() { ApplicationKey = "{ABCDEF01-2345-6789-ABCD-EF0123456789}",
                    Source = InstalledApplicationSource.Msi,
                    Name = "Acme Tool",
                    Version = "1.2.3",
                    Publisher = "Acme",
                    CanUninstallSilently = true },
            new() { ApplicationKey = "Microsoft.WindowsCalculator_10.2103.8.0_x64__8wekyb3d8bbwe",
                    Source = InstalledApplicationSource.Appx,
                    Name = "Calculator",
                    CanUninstallSilently = true },
        };

        var fetchedAt = DateTimeOffset.UtcNow;
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, fetchedAt);

        var loaded = await _service.GetSnapshotAsync(_testData.Org1Device1.ID);

        Assert.IsNotNull(loaded);
        Assert.AreEqual(2, loaded!.Value.Applications.Count);
        Assert.AreEqual("Acme Tool", loaded.Value.Applications[0].Name);
        Assert.AreEqual(InstalledApplicationSource.Appx, loaded.Value.Applications[1].Source);
    }

    [TestMethod]
    public async Task GetSnapshot_ReturnsNull_WhenDeviceHasNoSnapshot()
    {
        var loaded = await _service.GetSnapshotAsync(_testData.Org1Device1.ID);
        Assert.IsNull(loaded);
    }

    [TestMethod]
    public async Task IssueAndResolveToken_HappyPath()
    {
        var apps = new List<InstalledApplication>
        {
            new() { ApplicationKey = "AcmeKey", Source = InstalledApplicationSource.Win32,
                    Name = "Acme", CanUninstallSilently = true },
        };
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, DateTimeOffset.UtcNow);

        var token = _service.IssueUninstallToken(_testData.Org1Device1.ID, "AcmeKey");
        Assert.IsNotNull(token);

        var resolved = _service.ResolveUninstallToken(_testData.Org1Device1.ID, token!);
        Assert.AreEqual("AcmeKey", resolved);
    }

    [TestMethod]
    public async Task IssueToken_ReturnsNull_WhenAppNotInSnapshot()
    {
        var apps = new List<InstalledApplication>
        {
            new() { ApplicationKey = "AcmeKey", Source = InstalledApplicationSource.Win32, Name = "Acme" },
        };
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, DateTimeOffset.UtcNow);

        var token = _service.IssueUninstallToken(_testData.Org1Device1.ID, "GhostKey");
        Assert.IsNull(token);
    }

    [TestMethod]
    public async Task ResolveToken_IsSingleUse()
    {
        var apps = new List<InstalledApplication>
        {
            new() { ApplicationKey = "AcmeKey", Source = InstalledApplicationSource.Win32, Name = "Acme" },
        };
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, DateTimeOffset.UtcNow);

        var token = _service.IssueUninstallToken(_testData.Org1Device1.ID, "AcmeKey")!;

        Assert.IsNotNull(_service.ResolveUninstallToken(_testData.Org1Device1.ID, token));
        Assert.IsNull(_service.ResolveUninstallToken(_testData.Org1Device1.ID, token));
    }

    [TestMethod]
    public async Task ResolveToken_RejectsTokenForDifferentDevice()
    {
        var apps = new List<InstalledApplication>
        {
            new() { ApplicationKey = "AcmeKey", Source = InstalledApplicationSource.Win32, Name = "Acme" },
        };
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, DateTimeOffset.UtcNow);

        var token = _service.IssueUninstallToken(_testData.Org1Device1.ID, "AcmeKey")!;
        var resolved = _service.ResolveUninstallToken(_testData.Org1Device2.ID, token);
        Assert.IsNull(resolved);
    }

    [TestMethod]
    public async Task SaveSnapshot_InvalidatesPriorTokens()
    {
        var apps = new List<InstalledApplication>
        {
            new() { ApplicationKey = "AcmeKey", Source = InstalledApplicationSource.Win32, Name = "Acme" },
        };
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, DateTimeOffset.UtcNow);
        var token = _service.IssueUninstallToken(_testData.Org1Device1.ID, "AcmeKey")!;

        // Re-saving the snapshot must invalidate any previously-issued
        // tokens, even if the application is still present.
        await _service.SaveSnapshotAsync(_testData.Org1Device1.ID, apps, DateTimeOffset.UtcNow.AddMinutes(1));

        var resolved = _service.ResolveUninstallToken(_testData.Org1Device1.ID, token);
        Assert.IsNull(resolved);
    }

    [TestMethod]
    public void IssueToken_ReturnsNull_WhenSnapshotMissing()
    {
        var token = _service.IssueUninstallToken(_testData.Org1Device1.ID, "AnyKey");
        Assert.IsNull(token);
    }
}
