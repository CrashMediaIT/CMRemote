using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using System;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class PackageServiceTests
{
    private TestData _testData = null!;
    private PackageService _service = null!;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();
        var dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _service = new PackageService(dbFactory, NullLogger<PackageService>.Instance);
    }

    [TestMethod]
    public async Task CreatePackage_RejectsEmptyName()
    {
        var result = await _service.CreatePackage(_testData.Org1Id, null, new Package
        {
            Name = "  ",
            PackageIdentifier = "googlechrome",
            Provider = PackageProvider.Chocolatey,
        });
        Assert.IsFalse(result.IsSuccess);
    }

    [TestMethod]
    public async Task CreatePackage_RejectsShellMetacharactersInArgs()
    {
        // ; & | $ ` ` < > and newlines must all be refused — the agent
        // splits args into argv slots, but server-side we still reject
        // anything that *looks* like a shell escape attempt.
        foreach (var bad in new[] { "--foo;rm -rf /", "a|b", "a&b", "a`whoami`", "a$(whoami)", "a\nb", "a\rb" })
        {
            var result = await _service.CreatePackage(_testData.Org1Id, null, new Package
            {
                Name = "Chrome",
                Provider = PackageProvider.Chocolatey,
                PackageIdentifier = "googlechrome",
                InstallArguments = bad,
            });
            Assert.IsFalse(result.IsSuccess, $"Expected to reject: {bad}");
        }
    }

    [TestMethod]
    public async Task CreatePackage_RejectsInvalidChocoIdentifier()
    {
        var result = await _service.CreatePackage(_testData.Org1Id, null, new Package
        {
            Name = "Chrome",
            Provider = PackageProvider.Chocolatey,
            PackageIdentifier = "google chrome",   // space is illegal
        });
        Assert.IsFalse(result.IsSuccess);
    }

    [TestMethod]
    public async Task CreatePackage_AllowsValidArgsAndRoundTrips()
    {
        var result = await _service.CreatePackage(_testData.Org1Id, _testData.Org1Admin1.Id, new Package
        {
            Name = "Chrome",
            Provider = PackageProvider.Chocolatey,
            PackageIdentifier = "googlechrome",
            InstallArguments = "--ignore-checksums --params=/NoDesktopShortcut",
            Description = "Browser",
        });
        Assert.IsTrue(result.IsSuccess);

        var listed = await _service.GetPackagesForOrg(_testData.Org1Id);
        Assert.AreEqual(1, listed.Count);
        Assert.AreEqual("Chrome", listed[0].Name);
        Assert.AreEqual(_testData.Org1Admin1.Id, listed[0].CreatedByUserId);
    }

    [TestMethod]
    public async Task GetPackage_RejectsCrossOrgRead()
    {
        var created = await _service.CreatePackage(_testData.Org1Id, null, new Package
        {
            Name = "Chrome",
            Provider = PackageProvider.Chocolatey,
            PackageIdentifier = "googlechrome",
        });
        Assert.IsTrue(created.IsSuccess);

        var foreign = await _service.GetPackage(_testData.Org2Admin1.OrganizationID, created.Value!.Id);
        Assert.IsNull(foreign);
    }

    [TestMethod]
    public async Task DeletePackage_RefusesIfReferencedByBundle()
    {
        var pkg = await _service.CreatePackage(_testData.Org1Id, null, new Package
        {
            Name = "Chrome",
            Provider = PackageProvider.Chocolatey,
            PackageIdentifier = "googlechrome",
        });
        var bundle = await _service.CreateBundle(_testData.Org1Id, null, "Onboarding", null);
        var added = await _service.AddBundleItem(_testData.Org1Id, bundle.Value!.Id, pkg.Value!.Id, 0, false);
        Assert.IsTrue(added.IsSuccess);

        var del = await _service.DeletePackage(_testData.Org1Id, pkg.Value!.Id);
        Assert.IsFalse(del.IsSuccess, "Should refuse to delete a package referenced by a bundle.");

        // Removing the bundle item then deleting the package should
        // succeed — proves the guard is local, not permanent.
        await _service.RemoveBundleItem(_testData.Org1Id, bundle.Value.Id, added.Value!.Id);
        Assert.IsTrue((await _service.DeletePackage(_testData.Org1Id, pkg.Value.Id)).IsSuccess);
    }

    [TestMethod]
    public async Task AddBundleItem_RejectsCrossOrgPackage()
    {
        // Sanity-check org isolation in the bundle/package join.
        var ownPackage = await _service.CreatePackage(_testData.Org1Id, null, new Package
        {
            Name = "Chrome",
            Provider = PackageProvider.Chocolatey,
            PackageIdentifier = "googlechrome",
        });
        var foreignBundle = await _service.CreateBundle(_testData.Org2Admin1.OrganizationID, null, "X", null);

        var add = await _service.AddBundleItem(
            _testData.Org2Admin1.OrganizationID, foreignBundle.Value!.Id, ownPackage.Value!.Id, 0, false);
        Assert.IsFalse(add.IsSuccess);
    }
}
