// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Tests.Infrastructure;
using System.Linq;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

/// <summary>
/// Smoke tests for <see cref="ServiceTestFixture"/>. These tests don't
/// exercise any production service — they exist purely to prove that the
/// shared fixture wires up correctly so the M3 slice tests (S1…S9) can
/// rely on it.
/// </summary>
[TestClass]
public class ServiceTestFixtureSmokeTests
{
    [TestMethod]
    public async Task CreateEmptyAsync_ResetsDb_AndExposesFactory()
    {
        var fixture = await ServiceTestFixture.CreateEmptyAsync();

        Assert.IsNotNull(fixture.DbFactory, "DbFactory should be resolved.");
        Assert.IsNotNull(fixture.Services, "Services provider should be resolved.");
        Assert.IsNull(fixture.Data, "Empty fixture should not seed TestData.");

        using var db = fixture.DbFactory.GetContext();
        Assert.AreEqual(0, await db.Organizations.CountAsync(),
            "Empty fixture should leave the Organizations table empty.");
        Assert.AreEqual(0, await db.Devices.CountAsync(),
            "Empty fixture should leave the Devices table empty.");
    }

    [TestMethod]
    public async Task CreateSeededAsync_PopulatesCanonicalTestData()
    {
        var fixture = await ServiceTestFixture.CreateSeededAsync();

        Assert.IsNotNull(fixture.Data, "Seeded fixture must expose TestData.");

        using var db = fixture.DbFactory.GetContext();
        var orgs = await db.Organizations.AsNoTracking()
            .OrderBy(o => o.OrganizationName).ToListAsync();
        Assert.AreEqual(2, orgs.Count,
            "Canonical TestData seeds exactly two organizations.");
        CollectionAssert.AreEqual(
            new[] { "Org1", "Org2" },
            orgs.Select(o => o.OrganizationName).ToArray());

        Assert.AreEqual(fixture.Data!.Org1Id, orgs[0].ID);
        Assert.AreEqual(fixture.Data!.Org2Id, orgs[1].ID);
    }

    [TestMethod]
    public async Task GetUserManager_Resolves()
    {
        var fixture = await ServiceTestFixture.CreateEmptyAsync();
        var userManager = fixture.GetUserManager();
        Assert.IsNotNull(userManager,
            "UserManager<RemotelyUser> should resolve from the shared provider.");
    }
}
