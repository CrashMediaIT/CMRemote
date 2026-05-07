// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.UserDirectory;
using Remotely.Server.Tests.Infrastructure;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;
using System.Linq;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class UserDirectoryServiceTests
{
    private ServiceTestFixture _fixture = null!;
    private IUserDirectoryService _service = null!;

    [TestInitialize]
    public async Task Init()
    {
        _fixture = await ServiceTestFixture.CreateSeededAsync();
        _service = new UserDirectoryService(
            _fixture.DbFactory,
            NullLogger<UserDirectoryService>.Instance);
    }

    [TestMethod]
    public async Task CreateUser_NormalizesEmail_AndAssignsOrgAndOptions()
    {
        var result = await _service.CreateUser("  NewUser@Example.COM  ", true, _fixture.Data!.Org1Id);

        Assert.IsTrue(result.IsSuccess, result.Reason);

        var user = (await _service.GetUserByName("newuser@example.com")).Value!;
        Assert.AreEqual("newuser@example.com", user.UserName);
        Assert.AreEqual("newuser@example.com", user.Email);
        Assert.AreEqual(_fixture.Data.Org1Id, user.OrganizationID);
        Assert.IsTrue(user.IsAdministrator);
        Assert.IsTrue(user.LockoutEnabled);
        Assert.IsNotNull(user.UserOptions);
    }

    [TestMethod]
    public async Task CreateUser_UnknownOrg_FailsWithoutCreatingUser()
    {
        var result = await _service.CreateUser("missing-org@test.com", false, "missing-org");

        Assert.IsFalse(result.IsSuccess);
        Assert.AreEqual("Organization not found.", result.Reason);
        Assert.IsFalse(_service.DoesUserExist("missing-org@test.com"));
    }

    [TestMethod]
    public async Task DeleteUser_RemovesOnlyUserInRequestedOrganization()
    {
        var crossOrgDelete = await _service.DeleteUser(_fixture.Data!.Org1Id, _fixture.Data.Org2User1.Id);

        Assert.IsFalse(crossOrgDelete.IsSuccess);
        Assert.AreEqual("User not found.", crossOrgDelete.Reason);
        Assert.IsTrue((await _service.GetUserById(_fixture.Data.Org2User1.Id)).IsSuccess);

        var delete = await _service.DeleteUser(_fixture.Data.Org1Id, _fixture.Data.Org1User1.Id);
        Assert.IsTrue(delete.IsSuccess, delete.Reason);
        Assert.IsFalse((await _service.GetUserById(_fixture.Data.Org1User1.Id)).IsSuccess);
    }

    [TestMethod]
    public async Task GetAllUsersInOrganization_ReturnsOrgScopedUsers_OrEmptyForBadInput()
    {
        var org1Users = await _service.GetAllUsersInOrganization(_fixture.Data!.Org1Id);

        Assert.AreEqual(4, org1Users.Length);
        Assert.IsTrue(org1Users.All(x => x.OrganizationID == _fixture.Data.Org1Id));
        Assert.AreEqual(0, (await _service.GetAllUsersInOrganization(string.Empty)).Length);
        Assert.AreEqual(0, (await _service.GetAllUsersInOrganization("missing-org")).Length);
    }

    [TestMethod]
    public async Task ChangeUserIsAdmin_IsOrganizationScoped()
    {
        await _service.ChangeUserIsAdmin(_fixture.Data!.Org1Id, _fixture.Data.Org2User1.Id, true);
        Assert.IsFalse((await _service.GetUserById(_fixture.Data.Org2User1.Id)).Value!.IsAdministrator);

        await _service.ChangeUserIsAdmin(_fixture.Data.Org1Id, _fixture.Data.Org1User1.Id, true);
        Assert.IsTrue((await _service.GetUserById(_fixture.Data.Org1User1.Id)).Value!.IsAdministrator);
    }

    [TestMethod]
    public async Task SetIsServerAdmin_RequiresServerAdminCaller_AndRefusesSelfChange()
    {
        await _service.SetIsServerAdmin(_fixture.Data!.Org1Admin2.Id, true, _fixture.Data.Org1User1.Id);
        Assert.IsFalse((await _service.GetUserById(_fixture.Data.Org1Admin2.Id)).Value!.IsServerAdmin);

        await _service.SetIsServerAdmin(_fixture.Data.Org1Admin1.Id, false, _fixture.Data.Org1Admin1.Id);
        Assert.IsTrue((await _service.GetUserById(_fixture.Data.Org1Admin1.Id)).Value!.IsServerAdmin);

        await _service.SetIsServerAdmin(_fixture.Data.Org1Admin2.Id, true, _fixture.Data.Org1Admin1.Id);
        Assert.IsTrue((await _service.GetUserById(_fixture.Data.Org1Admin2.Id)).Value!.IsServerAdmin);
    }

    [TestMethod]
    public async Task UserOptions_RoundTrip_AndDisplayNameUpdates()
    {
        var options = new RemotelyUserOptions
        {
            DisplayName = "Operator One",
            CommandModeShortcutBash = "/bashx"
        };

        var update = await _service.UpdateUserOptions(_fixture.Data!.Org1User1.UserName!, options);
        Assert.IsTrue(update.IsSuccess, update.Reason);

        var storedOptions = (await _service.GetUserOptions(_fixture.Data.Org1User1.UserName!)).Value!;
        Assert.AreEqual("Operator One", storedOptions.DisplayName);
        Assert.AreEqual("/bashx", storedOptions.CommandModeShortcutBash);

        await _service.SetDisplayName(_fixture.Data.Org1User1, "Help Desk");
        storedOptions = (await _service.GetUserOptions(_fixture.Data.Org1User1.UserName!)).Value!;
        Assert.AreEqual("Help Desk", storedOptions.DisplayName);
    }

    [TestMethod]
    public async Task Lookups_HandleBlankAndMissingValues()
    {
        Assert.IsFalse((await _service.GetUserById(string.Empty)).IsSuccess);
        Assert.IsFalse((await _service.GetUserByName(" ")).IsSuccess);
        Assert.IsFalse((await _service.GetUserOptions("missing@test.com")).IsSuccess);
        Assert.IsFalse(_service.DoesUserExist(" "));
        Assert.IsFalse((await _service.UpdateUserOptions("missing@test.com", new RemotelyUserOptions())).IsSuccess);
    }

    [TestMethod]
    public async Task GetAllUsersForServer_ReturnsBothOrganizations()
    {
        var users = _service.GetAllUsersForServer();

        Assert.AreEqual(8, users.Length);
        Assert.AreEqual(4, users.Count(x => x.OrganizationID == _fixture.Data!.Org1Id));
        Assert.AreEqual(4, users.Count(x => x.OrganizationID == _fixture.Data!.Org2Id));

        using var db = _fixture.DbFactory.GetContext();
        Assert.AreEqual(8, await db.Users.AsNoTracking().CountAsync());
    }
}
