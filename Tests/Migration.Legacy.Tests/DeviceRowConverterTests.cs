using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class DeviceRowConverterTests
{
    [TestMethod]
    public void EntityName_AndHandlesSchemaVersion_AreStable()
    {
        var c = new DeviceRowConverter();
        Assert.AreEqual("Device", c.EntityName);
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, c.HandlesSchemaVersion);
    }

    [TestMethod]
    public void Convert_NullRow_Fails()
    {
        var result = new DeviceRowConverter().Convert(null!);
        Assert.IsTrue(result.IsFailure);
        StringAssert.Contains(result.ErrorMessage!, "null");
    }

    [TestMethod]
    public void Convert_MissingId_Fails()
    {
        var result = new DeviceRowConverter().Convert(
            new LegacyDevice { ID = "", OrganizationID = "o1" });
        Assert.IsTrue(result.IsFailure);
        StringAssert.Contains(result.ErrorMessage!, "ID");
    }

    [TestMethod]
    public void Convert_MissingOrg_Skips()
    {
        var result = new DeviceRowConverter().Convert(
            new LegacyDevice { ID = "d1", OrganizationID = null });
        Assert.IsTrue(result.IsSkipped);
        StringAssert.Contains(result.SkipReason!, "OrganizationID");
    }

    [TestMethod]
    public void Convert_PreservesIdentityAndScalars()
    {
        var row = new LegacyDevice
        {
            ID = "device-1",
            OrganizationID = "org-1",
            DeviceName = "host-a",
            Platform = "Linux",
            AgentVersion = "1.2.3",
            Is64Bit = true,
            ProcessorCount = 8,
            TotalMemory = 16.0,
            UsedMemory = 4.0,
            OSArchitecture = 9,  // Architecture.Arm64
        };

        var result = new DeviceRowConverter().Convert(row);

        Assert.IsTrue(result.IsSuccess);
        var v2 = result.Value!;
        Assert.AreEqual("device-1", v2.ID);
        Assert.AreEqual("org-1", v2.OrganizationID);
        Assert.AreEqual("host-a", v2.DeviceName);
        Assert.AreEqual("Linux", v2.Platform);
        Assert.AreEqual("1.2.3", v2.AgentVersion);
        Assert.IsTrue(v2.Is64Bit);
        Assert.AreEqual(8, v2.ProcessorCount);
        Assert.AreEqual(16.0, v2.TotalMemory);
        Assert.AreEqual((System.Runtime.InteropServices.Architecture)9, v2.OSArchitecture);
        Assert.IsFalse(v2.IsOnline,
            "Devices must start offline post-migration; the agent re-asserts on next check-in.");
        CollectionAssert.AreEqual(System.Array.Empty<string>(), v2.MacAddresses);
    }

    [TestMethod]
    public void Convert_LongStringFields_Truncate()
    {
        var row = new LegacyDevice
        {
            ID = "d",
            OrganizationID = "o",
            Alias = new string('a', 250),
            Tags = new string('t', 500),
            Notes = new string('n', 6000),
        };

        var result = new DeviceRowConverter().Convert(row);

        Assert.IsTrue(result.IsSuccess);
        Assert.AreEqual(100, result.Value!.Alias!.Length);
        Assert.AreEqual(200, result.Value.Tags!.Length);
        Assert.AreEqual(5000, result.Value.Notes!.Length);
    }
}
