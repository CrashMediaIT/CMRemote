using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class LegacyToV2ConverterGoldenVectorTests
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true,
    };

    [TestMethod]
    public void OrganizationConverter_MatchesGoldenVector()
    {
        var vector = ReadVector<LegacyOrganization, ExpectedOrganization>("organization-basic.json");
        var result = new OrganizationRowConverter().Convert(vector.Legacy);

        Assert.IsTrue(result.IsSuccess, result.ErrorMessage);
        var actual = result.Value!;
        Assert.AreEqual(vector.Expected.ID, actual.ID);
        Assert.AreEqual(vector.Expected.OrganizationName, actual.OrganizationName);
        Assert.AreEqual(vector.Expected.IsDefaultOrganization, actual.IsDefaultOrganization);
        Assert.AreEqual(vector.Expected.PackageManagerEnabled, actual.PackageManagerEnabled);
    }

    [TestMethod]
    public void DeviceConverter_MatchesGoldenVector()
    {
        var vector = ReadVector<LegacyDevice, ExpectedDevice>("device-basic.json");
        var result = new DeviceRowConverter().Convert(vector.Legacy);

        Assert.IsTrue(result.IsSuccess, result.ErrorMessage);
        var actual = result.Value!;
        Assert.AreEqual(vector.Expected.ID, actual.ID);
        Assert.AreEqual(vector.Expected.OrganizationID, actual.OrganizationID);
        Assert.AreEqual(vector.Expected.DeviceName, actual.DeviceName);
        Assert.AreEqual(vector.Expected.Alias, actual.Alias);
        Assert.AreEqual(vector.Expected.Tags, actual.Tags);
        Assert.AreEqual(vector.Expected.Notes, actual.Notes);
        Assert.AreEqual(vector.Expected.Platform, actual.Platform);
        Assert.AreEqual(vector.Expected.OSDescription, actual.OSDescription);
        Assert.AreEqual(vector.Expected.AgentVersion, actual.AgentVersion);
        Assert.AreEqual(vector.Expected.CurrentUser, actual.CurrentUser);
        Assert.AreEqual(vector.Expected.PublicIP, actual.PublicIP);
        Assert.AreEqual(vector.Expected.DeviceGroupID, actual.DeviceGroupID);
        Assert.AreEqual(vector.Expected.ServerVerificationToken, actual.ServerVerificationToken);
        Assert.AreEqual(vector.Expected.Is64Bit, actual.Is64Bit);
        Assert.AreEqual(vector.Expected.IsOnline, actual.IsOnline);
        Assert.AreEqual(vector.Expected.LastOnline, actual.LastOnline);
        Assert.AreEqual(vector.Expected.ProcessorCount, actual.ProcessorCount);
        Assert.AreEqual(vector.Expected.CpuUtilization, actual.CpuUtilization);
        Assert.AreEqual(vector.Expected.TotalMemory, actual.TotalMemory);
        Assert.AreEqual(vector.Expected.UsedMemory, actual.UsedMemory);
        Assert.AreEqual(vector.Expected.TotalStorage, actual.TotalStorage);
        Assert.AreEqual(vector.Expected.UsedStorage, actual.UsedStorage);
        Assert.AreEqual((Architecture)vector.Expected.OSArchitecture, actual.OSArchitecture);
        CollectionAssert.AreEqual(vector.Expected.MacAddresses, actual.MacAddresses);
    }

    [TestMethod]
    public void UserConverter_MatchesGoldenVector()
    {
        var vector = ReadVector<LegacyAspNetUser, ExpectedUser>("user-basic.json");
        var result = new AspNetUserRowConverter().Convert(vector.Legacy);

        Assert.IsTrue(result.IsSuccess, result.ErrorMessage);
        var actual = result.Value!;
        Assert.AreEqual(vector.Expected.Id, actual.Id);
        Assert.AreEqual(vector.Expected.UserName, actual.UserName);
        Assert.AreEqual(vector.Expected.NormalizedUserName, actual.NormalizedUserName);
        Assert.AreEqual(vector.Expected.Email, actual.Email);
        Assert.AreEqual(vector.Expected.NormalizedEmail, actual.NormalizedEmail);
        Assert.AreEqual(vector.Expected.EmailConfirmed, actual.EmailConfirmed);
        Assert.AreEqual(vector.Expected.PasswordHash, actual.PasswordHash);
        Assert.AreEqual(vector.Expected.SecurityStamp, actual.SecurityStamp);
        Assert.AreEqual(vector.Expected.ConcurrencyStamp, actual.ConcurrencyStamp);
        Assert.AreEqual(vector.Expected.PhoneNumber, actual.PhoneNumber);
        Assert.AreEqual(vector.Expected.PhoneNumberConfirmed, actual.PhoneNumberConfirmed);
        Assert.AreEqual(vector.Expected.TwoFactorEnabled, actual.TwoFactorEnabled);
        Assert.AreEqual(vector.Expected.LockoutEnd, actual.LockoutEnd);
        Assert.AreEqual(vector.Expected.LockoutEnabled, actual.LockoutEnabled);
        Assert.AreEqual(vector.Expected.AccessFailedCount, actual.AccessFailedCount);
        Assert.AreEqual(vector.Expected.OrganizationID, actual.OrganizationID);
        Assert.AreEqual(vector.Expected.IsAdministrator, actual.IsAdministrator);
        Assert.AreEqual(vector.Expected.IsServerAdmin, actual.IsServerAdmin);
    }

    private static GoldenVector<TLegacy, TExpected> ReadVector<TLegacy, TExpected>(string fileName)
    {
        var path = Path.Combine(AppContext.BaseDirectory, "Fixtures", "legacy-to-v2", fileName);
        var json = File.ReadAllText(path);
        return JsonSerializer.Deserialize<GoldenVector<TLegacy, TExpected>>(json, JsonOptions)
            ?? throw new InvalidOperationException($"Could not deserialize golden vector '{fileName}'.");
    }

    private sealed record GoldenVector<TLegacy, TExpected>(TLegacy Legacy, TExpected Expected);

    private sealed record ExpectedOrganization(
        string ID,
        string OrganizationName,
        bool IsDefaultOrganization,
        bool PackageManagerEnabled);

    private sealed record ExpectedDevice(
        string ID,
        string OrganizationID,
        string? DeviceName,
        string? Alias,
        string? Tags,
        string? Notes,
        string? Platform,
        string? OSDescription,
        string? AgentVersion,
        string? CurrentUser,
        string? PublicIP,
        string? DeviceGroupID,
        string? ServerVerificationToken,
        bool Is64Bit,
        bool IsOnline,
        DateTimeOffset LastOnline,
        int ProcessorCount,
        double CpuUtilization,
        double TotalMemory,
        double UsedMemory,
        double TotalStorage,
        double UsedStorage,
        int OSArchitecture,
        string[] MacAddresses);

    private sealed record ExpectedUser(
        string Id,
        string? UserName,
        string? NormalizedUserName,
        string? Email,
        string? NormalizedEmail,
        bool EmailConfirmed,
        string? PasswordHash,
        string? SecurityStamp,
        string? ConcurrencyStamp,
        string? PhoneNumber,
        bool PhoneNumberConfirmed,
        bool TwoFactorEnabled,
        DateTimeOffset? LockoutEnd,
        bool LockoutEnabled,
        int AccessFailedCount,
        string OrganizationID,
        bool IsAdministrator,
        bool IsServerAdmin);
}
