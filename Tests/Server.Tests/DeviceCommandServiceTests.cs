// Source: CMRemote, clean-room implementation

using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.Devices;
using Remotely.Server.Tests.Infrastructure;
using Remotely.Shared.Dtos;
using Remotely.Shared.Models;
using System.Linq;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class DeviceCommandServiceTests
{
    private ServiceTestFixture _fixture = null!;
    private IDeviceCommandService _service = null!;
    private IDeviceQueryService _query = null!;

    [TestInitialize]
    public async Task Init()
    {
        _fixture = await ServiceTestFixture.CreateSeededAsync();
        _service = _fixture.Services.GetRequiredService<IDeviceCommandService>();
        _query = _fixture.Services.GetRequiredService<IDeviceQueryService>();
    }

    [TestMethod]
    public async Task AddOrUpdateDevice_InsertsThenUpdatesAndRefreshesInventory()
    {
        var dto = new DeviceClientDto
        {
            ID = "newDevice",
            DeviceName = "newDevice-name",
            OrganizationID = _fixture.Data!.Org1Id,
            CpuUtilization = 0.25,
            UsedMemory = 1024,
            TotalMemory = 4096,
            UsedStorage = 1,
            TotalStorage = 100,
            Is64Bit = true,
            OSArchitecture = System.Runtime.InteropServices.Architecture.X64,
            OSDescription = "Linux",
            Platform = "Linux",
            ProcessorCount = 8,
            PublicIP = "10.0.0.1",
            AgentVersion = "1.0.0",
            MacAddresses = new[] { "aa:bb" }
        };

        var inserted = await _service.AddOrUpdateDevice(dto);
        Assert.IsTrue(inserted.IsSuccess);
        Assert.IsTrue(inserted.Value.IsOnline);
        Assert.AreEqual("newDevice-name", inserted.Value.DeviceName);

        // Mutate and re-upsert: the row is reused, IsOnline stays true,
        // and changed inventory fields are persisted.
        dto.DeviceName = "newDevice-renamed";
        dto.UsedMemory = 2048;
        var updated = await _service.AddOrUpdateDevice(dto);
        Assert.IsTrue(updated.IsSuccess);
        Assert.AreEqual("newDevice-renamed", updated.Value.DeviceName);
        Assert.AreEqual(2048, updated.Value.UsedMemory);

        var stored = (await _query.GetDevice("newDevice")).Value;
        Assert.IsNotNull(stored);
        Assert.AreEqual("newDevice-renamed", stored.DeviceName);
        Assert.IsTrue(stored.IsOnline);
    }

    [TestMethod]
    public async Task AddOrUpdateDevice_UnknownOrganizationFails()
    {
        var dto = new DeviceClientDto
        {
            ID = "ghost",
            DeviceName = "ghost",
            OrganizationID = "missing-org",
            MacAddresses = System.Array.Empty<string>()
        };

        var result = await _service.AddOrUpdateDevice(dto);
        Assert.IsFalse(result.IsSuccess);
        Assert.AreEqual("Organization does not exist.", result.Reason);
    }

    [TestMethod]
    public async Task CreateDevice_RejectsMissingFieldsAndDuplicates()
    {
        // Missing fields → fail.
        var bad = await _service.CreateDevice(new DeviceSetupOptions
        {
            DeviceID = string.Empty,
            OrganizationID = _fixture.Data!.Org1Id
        });
        Assert.IsFalse(bad.IsSuccess);
        Assert.AreEqual("Required parameters are missing or incorrect.", bad.Reason);

        // Duplicate id (Org1Device1 already seeded) → fail.
        var dup = await _service.CreateDevice(new DeviceSetupOptions
        {
            DeviceID = _fixture.Data.Org1Device1.ID,
            OrganizationID = _fixture.Data.Org1Id
        });
        Assert.IsFalse(dup.IsSuccess);
        Assert.AreEqual("Required parameters are missing or incorrect.", dup.Reason);

        // Happy path with a device-group name that resolves to a real group.
        var ok = await _service.CreateDevice(new DeviceSetupOptions
        {
            DeviceID = "fresh-device",
            OrganizationID = _fixture.Data.Org1Id,
            DeviceAlias = "Fresh",
            DeviceGroupName = _fixture.Data.Org1Group1.Name
        });
        Assert.IsTrue(ok.IsSuccess);
        Assert.AreEqual("Fresh", ok.Value.Alias);
        Assert.AreEqual(_fixture.Data.Org1Group1.ID, ok.Value.DeviceGroup?.ID);
    }

    [TestMethod]
    public async Task UpdateDevice_FieldOverload_AssignsAndClearsGroup()
    {
        var deviceId = _fixture.Data!.Org1Device1.ID;
        var groupId = _fixture.Data.Org1Group1.ID;

        await _service.UpdateDevice(deviceId, "tag", "alias", groupId, "notes");

        using (var db = _fixture.DbFactory.GetContext())
        {
            var device = await db.Devices.FirstAsync(x => x.ID == deviceId);
            Assert.AreEqual("tag", device.Tags);
            Assert.AreEqual("alias", device.Alias);
            Assert.AreEqual("notes", device.Notes);
            Assert.AreEqual(groupId, device.DeviceGroupID);
        }

        // Empty group id clears the assignment.
        await _service.UpdateDevice(deviceId, null, null, "", null);
        using (var db = _fixture.DbFactory.GetContext())
        {
            var device = await db.Devices.FirstAsync(x => x.ID == deviceId);
            Assert.IsNull(device.DeviceGroupID);
        }

        // Missing device → no-op (no exception).
        await _service.UpdateDevice("missing", "x", "x", "x", "x");
    }

    [TestMethod]
    public async Task UpdateDevice_OptionsOverload_RejectsCrossOrg()
    {
        var options = new DeviceSetupOptions
        {
            DeviceID = _fixture.Data!.Org1Device1.ID,
            DeviceAlias = "renamed"
        };

        // Cross-org call → rejected.
        var crossOrg = await _service.UpdateDevice(options, _fixture.Data.Org2Id);
        Assert.IsFalse(crossOrg.IsSuccess);
        Assert.AreEqual("Device not found.", crossOrg.Reason);

        // Same-org → succeeds and updates alias.
        var ok = await _service.UpdateDevice(options, _fixture.Data.Org1Id);
        Assert.IsTrue(ok.IsSuccess);
        Assert.AreEqual("renamed", ok.Value.Alias);
    }

    [TestMethod]
    public async Task UpdateTags_WritesValueAndIsNoOpForMissing()
    {
        var deviceId = _fixture.Data!.Org1Device1.ID;

        await _service.UpdateTags(deviceId, "redacted, prod");

        using var db = _fixture.DbFactory.GetContext();
        var device = await db.Devices.FirstAsync(x => x.ID == deviceId);
        Assert.AreEqual("redacted, prod", device.Tags);

        // No throw on missing device.
        await _service.UpdateTags("missing-device", "anything");
    }

    [TestMethod]
    public async Task DeviceDisconnected_FlipsOnlineFlagAndStampsLastOnline()
    {
        var deviceId = _fixture.Data!.Org1Device1.ID;

        // Seed the device as online to make the flip observable.
        await _service.AddOrUpdateDevice(new DeviceClientDto
        {
            ID = deviceId,
            DeviceName = _fixture.Data.Org1Device1.DeviceName ?? "Org1Device1",
            OrganizationID = _fixture.Data.Org1Id,
            MacAddresses = System.Array.Empty<string>()
        });

        _service.DeviceDisconnected(deviceId);

        using var db = _fixture.DbFactory.GetContext();
        var device = await db.Devices.FirstAsync(x => x.ID == deviceId);
        Assert.IsFalse(device.IsOnline);
    }

    [TestMethod]
    public async Task SetAllDevicesNotOnline_FlipsAllRows()
    {
        // Bring at least one device up first.
        await _service.AddOrUpdateDevice(new DeviceClientDto
        {
            ID = _fixture.Data!.Org1Device1.ID,
            DeviceName = _fixture.Data.Org1Device1.DeviceName ?? "Org1Device1",
            OrganizationID = _fixture.Data.Org1Id,
            MacAddresses = System.Array.Empty<string>()
        });

        await _service.SetAllDevicesNotOnline();

        using var db = _fixture.DbFactory.GetContext();
        Assert.IsTrue(db.Devices.All(d => !d.IsOnline));
    }
}
