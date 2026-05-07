// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.Organizations;
using Remotely.Server.Tests.Infrastructure;
using Remotely.Shared.Entities;
using System.Linq;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class OrganizationServiceTests
{
    private ServiceTestFixture _fixture = null!;
    private IOrganizationService _service = null!;

    [TestInitialize]
    public async Task Init()
    {
        _fixture = await ServiceTestFixture.CreateSeededAsync();
        _service = new OrganizationService(_fixture.DbFactory);
    }

    [TestMethod]
    public async Task GetDefaultOrganization_ReturnsTheFlaggedOrg_OrFailsWhenNoneFlagged()
    {
        var initial = await _service.GetDefaultOrganization();
        Assert.IsFalse(initial.IsSuccess);
        Assert.AreEqual("Organization not found.", initial.Reason);

        await _service.SetIsDefaultOrganization(_fixture.Data!.Org1Id, true);

        var current = await _service.GetDefaultOrganization();
        Assert.IsTrue(current.IsSuccess);
        Assert.AreEqual(_fixture.Data.Org1Id, current.Value.ID);
    }

    [TestMethod]
    public async Task SetIsDefaultOrganization_IsExclusive_AndIdempotentForUnknownIds()
    {
        await _service.SetIsDefaultOrganization("missing-org", true);
        Assert.IsFalse((await _service.GetDefaultOrganization()).IsSuccess);

        await _service.SetIsDefaultOrganization(_fixture.Data!.Org1Id, true);
        await _service.SetIsDefaultOrganization(_fixture.Data.Org2Id, true);

        using var db = _fixture.DbFactory.GetContext();
        var defaults = await db.Organizations
            .AsNoTracking()
            .Where(x => x.IsDefaultOrganization)
            .Select(x => x.ID)
            .ToListAsync();

        Assert.AreEqual(1, defaults.Count);
        Assert.AreEqual(_fixture.Data.Org2Id, defaults.Single());
    }

    [TestMethod]
    public async Task GetOrganizationById_ReturnsOrgOrFailureForUnknownId()
    {
        var ok = await _service.GetOrganizationById(_fixture.Data!.Org1Id);
        Assert.IsTrue(ok.IsSuccess);
        Assert.AreEqual("Org1", ok.Value.OrganizationName);

        var fail = await _service.GetOrganizationById("missing-org");
        Assert.IsFalse(fail.IsSuccess);
        Assert.AreEqual("Organization not found.", fail.Reason);
    }

    [TestMethod]
    public async Task GetOrganizationByUserName_HandlesBlank_Unknown_AndCaseInsensitive()
    {
        var blank = await _service.GetOrganizationByUserName(" ");
        Assert.IsFalse(blank.IsSuccess);
        Assert.AreEqual("User name is required.", blank.Reason);

        var missing = await _service.GetOrganizationByUserName("missing@test.com");
        Assert.IsFalse(missing.IsSuccess);
        Assert.AreEqual("User not found.", missing.Reason);

        var ok = await _service.GetOrganizationByUserName(
            _fixture.Data!.Org1Admin1.UserName!.ToUpperInvariant());
        Assert.IsTrue(ok.IsSuccess);
        Assert.AreEqual(_fixture.Data.Org1Id, ok.Value.ID);
    }

    [TestMethod]
    public async Task GetOrganizationCountAsync_ReflectsSeededOrgs()
    {
        Assert.AreEqual(2, await _service.GetOrganizationCountAsync());
    }

    [TestMethod]
    public async Task GetOrganizationName_ByIdAndByUserName_HandleHappyAndSadPaths()
    {
        Assert.AreEqual("Org1", (await _service.GetOrganizationNameById(_fixture.Data!.Org1Id)).Value);
        Assert.IsFalse((await _service.GetOrganizationNameById("missing-org")).IsSuccess);

        Assert.AreEqual(
            "Org1",
            (await _service.GetOrganizationNameByUserName(_fixture.Data.Org1Admin1.UserName!)).Value);
        Assert.IsFalse((await _service.GetOrganizationNameByUserName(" ")).IsSuccess);
        Assert.IsFalse((await _service.GetOrganizationNameByUserName("missing@test.com")).IsSuccess);
    }

    [TestMethod]
    public async Task UpdateOrganizationName_PersistsTheValue_AndFailsForUnknownId()
    {
        var ok = await _service.UpdateOrganizationName(_fixture.Data!.Org1Id, "Renamed");
        Assert.IsTrue(ok.IsSuccess);
        Assert.AreEqual("Renamed", (await _service.GetOrganizationNameById(_fixture.Data.Org1Id)).Value);

        var fail = await _service.UpdateOrganizationName("missing-org", "anything");
        Assert.IsFalse(fail.IsSuccess);
        Assert.AreEqual("Organization not found.", fail.Reason);
    }

    [TestMethod]
    public async Task SetOrganizationPackageManagerEnabled_TogglesFlag_AndClearsSnapshotsOnDisable()
    {
        var deviceId = _fixture.Data!.Org1Device1.ID;

        using (var seed = _fixture.DbFactory.GetContext())
        {
            seed.DeviceInstalledApplicationsSnapshots.Add(new DeviceInstalledApplicationsSnapshot
            {
                DeviceId = deviceId,
                FetchedAt = System.DateTimeOffset.UtcNow,
                ApplicationsJson = "[]"
            });
            await seed.SaveChangesAsync();
        }

        await _service.SetOrganizationPackageManagerEnabled(_fixture.Data.Org1Id, true);
        Assert.IsTrue(
            (await _service.GetOrganizationById(_fixture.Data.Org1Id)).Value!.PackageManagerEnabled);

        await _service.SetOrganizationPackageManagerEnabled(_fixture.Data.Org1Id, false);
        Assert.IsFalse(
            (await _service.GetOrganizationById(_fixture.Data.Org1Id)).Value!.PackageManagerEnabled);

        using var db = _fixture.DbFactory.GetContext();
        var remaining = await db.DeviceInstalledApplicationsSnapshots
            .AsNoTracking()
            .CountAsync(x => x.DeviceId == deviceId);
        Assert.AreEqual(0, remaining);

        // Idempotent for unknown ids — must not throw.
        await _service.SetOrganizationPackageManagerEnabled("missing-org", true);
    }
}
