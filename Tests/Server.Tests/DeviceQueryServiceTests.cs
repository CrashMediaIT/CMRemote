// Source: CMRemote, clean-room implementation

using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.Devices;
using Remotely.Server.Tests.Infrastructure;
using System.Linq;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class DeviceQueryServiceTests
{
    private ServiceTestFixture _fixture = null!;
    private IDeviceQueryService _service = null!;

    [TestInitialize]
    public async Task Init()
    {
        _fixture = await ServiceTestFixture.CreateSeededAsync();
        _service = new DeviceQueryService(_fixture.DbFactory);
    }

    [TestMethod]
    public async Task GetDevice_ById_HappyAndSadPaths()
    {
        var ok = await _service.GetDevice(_fixture.Data!.Org1Device1.ID);
        Assert.IsTrue(ok.IsSuccess);
        Assert.AreEqual(_fixture.Data.Org1Device1.ID, ok.Value.ID);

        var fail = await _service.GetDevice("missing-device");
        Assert.IsFalse(fail.IsSuccess);
        Assert.AreEqual("Device not found.", fail.Reason);
    }

    [TestMethod]
    public async Task GetDevice_ScopedByOrg_RejectsCrossOrgLookup()
    {
        var ok = await _service.GetDevice(
            _fixture.Data!.Org1Id,
            _fixture.Data.Org1Device1.ID);
        Assert.IsTrue(ok.IsSuccess);

        var crossOrg = await _service.GetDevice(
            _fixture.Data.Org2Id,
            _fixture.Data.Org1Device1.ID);
        Assert.IsFalse(crossOrg.IsSuccess);
        Assert.AreEqual("Device not found.", crossOrg.Reason);
    }

    [TestMethod]
    public void GetAllDevices_ReturnsOnlyDevicesInOrg()
    {
        var org1 = _service.GetAllDevices(_fixture.Data!.Org1Id)
            .Select(x => x.ID)
            .OrderBy(x => x)
            .ToArray();
        CollectionAssert.AreEqual(new[] { "Org1Device1", "Org1Device2" }, org1);

        var unknown = _service.GetAllDevices("missing-org");
        Assert.AreEqual(0, unknown.Length);
    }

    [TestMethod]
    public void GetDevicesForUser_HandlesAdminsUsersAndUnknowns()
    {
        Assert.AreEqual(0, _service.GetDevicesForUser(string.Empty).Length);
        Assert.AreEqual(0, _service.GetDevicesForUser("missing@test.com").Length);

        // Admin sees every device in their org.
        Assert.AreEqual(2, _service.GetDevicesForUser(_fixture.Data!.Org1Admin1.UserName!).Length);

        // Non-admins see only devices via their device groups; users start with none.
        Assert.AreEqual(0, _service.GetDevicesForUser(_fixture.Data.Org1User1.UserName!).Length);
    }

    [TestMethod]
    public void DoesUserHaveAccessToDevice_AdminAlways_OthersByGroupMembership()
    {
        var deviceId = _fixture.Data!.Org1Device1.ID;

        Assert.IsTrue(_service.DoesUserHaveAccessToDevice(deviceId, _fixture.Data.Org1Admin1));
        Assert.IsFalse(_service.DoesUserHaveAccessToDevice(deviceId, _fixture.Data.Org1User1));

        // Cross-org admin still rejected by orgID scoping.
        Assert.IsFalse(_service.DoesUserHaveAccessToDevice(deviceId, _fixture.Data.Org2Admin1));

        // Overload by user id.
        Assert.IsTrue(_service.DoesUserHaveAccessToDevice(deviceId, _fixture.Data.Org1Admin1.Id));
        Assert.IsFalse(_service.DoesUserHaveAccessToDevice(deviceId, "missing-user-id"));
    }

    [TestMethod]
    public void FilterDeviceIdsByUserPermission_EnforcesOrgAndAdminGroupRules()
    {
        var allIds = new[] { "Org1Device1", "Org1Device2", "Org2Device1", "missing" };

        var admin = _service.FilterDeviceIdsByUserPermission(allIds, _fixture.Data!.Org1Admin1);
        CollectionAssert.AreEquivalent(new[] { "Org1Device1", "Org1Device2" }, admin);

        var user = _service.FilterDeviceIdsByUserPermission(allIds, _fixture.Data.Org1User1);
        Assert.AreEqual(0, user.Length);
    }

    [TestMethod]
    public void FilterUsersByDevicePermission_RestrictsToOrgAndPermissions()
    {
        // Org1Device1 has no group → all org1 users pass through.
        var ungroupedDevice = _fixture.Data!.Org1Device1.ID;
        var allUserIds = new[]
        {
            _fixture.Data.Org1Admin1.Id,
            _fixture.Data.Org1User1.Id,
            _fixture.Data.Org2User1.Id
        };

        var ungroupedFiltered = _service.FilterUsersByDevicePermission(allUserIds, ungroupedDevice);
        CollectionAssert.AreEquivalent(
            new[] { _fixture.Data.Org1Admin1.Id, _fixture.Data.Org1User1.Id },
            ungroupedFiltered);

        var unknownDevice = _service.FilterUsersByDevicePermission(allUserIds, "missing-device");
        Assert.AreEqual(0, unknownDevice.Length);
    }

    [TestMethod]
    public async Task GetDeviceGroup_HappyAndSadPaths()
    {
        var ok = await _service.GetDeviceGroup(
            _fixture.Data!.Org1Group1.ID,
            includeDevices: true,
            includeUsers: true);
        Assert.IsTrue(ok.IsSuccess);
        Assert.IsNotNull(ok.Value.Devices);
        Assert.IsNotNull(ok.Value.Users);

        var fail = await _service.GetDeviceGroup("missing-group");
        Assert.IsFalse(fail.IsSuccess);
        Assert.AreEqual("Device group not found.", fail.Reason);
    }
}
