using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class AspNetUserRowConverterTests
{
    [TestMethod]
    public void EntityName_AndHandlesSchemaVersion_AreStable()
    {
        var c = new AspNetUserRowConverter();
        Assert.AreEqual("User", c.EntityName);
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, c.HandlesSchemaVersion);
    }

    [TestMethod]
    public void Convert_NullRow_Fails()
    {
        var result = new AspNetUserRowConverter().Convert(null!);
        Assert.IsTrue(result.IsFailure);
    }

    [TestMethod]
    public void Convert_MissingId_Fails()
    {
        var result = new AspNetUserRowConverter().Convert(
            new LegacyAspNetUser { Id = "", UserName = "alice", OrganizationID = "o" });
        Assert.IsTrue(result.IsFailure);
        StringAssert.Contains(result.ErrorMessage!, "Id");
    }

    [TestMethod]
    public void Convert_MissingUserName_Fails()
    {
        var result = new AspNetUserRowConverter().Convert(
            new LegacyAspNetUser { Id = "u1", UserName = null, OrganizationID = "o" });
        Assert.IsTrue(result.IsFailure);
        StringAssert.Contains(result.ErrorMessage!, "UserName");
    }

    [TestMethod]
    public void Convert_MissingOrg_Skips()
    {
        var result = new AspNetUserRowConverter().Convert(
            new LegacyAspNetUser { Id = "u1", UserName = "alice", OrganizationID = null });
        Assert.IsTrue(result.IsSkipped);
    }

    [TestMethod]
    public void Convert_PreservesIdentityColumnsVerbatim()
    {
        // The whole point of an importer (vs a re-invite flow) is
        // that password hashes + 2FA state survive the migration.
        // This test pins that round-trip.
        var row = new LegacyAspNetUser
        {
            Id = "u-1",
            UserName = "alice",
            NormalizedUserName = "ALICE",
            Email = "alice@example.com",
            NormalizedEmail = "ALICE@EXAMPLE.COM",
            EmailConfirmed = true,
            PasswordHash = "AQAAAAEAACcQAAAAEH..hash..",
            SecurityStamp = "STAMP-123",
            ConcurrencyStamp = "CC-456",
            PhoneNumber = "+1-555-0100",
            PhoneNumberConfirmed = true,
            TwoFactorEnabled = true,
            LockoutEnd = new DateTimeOffset(2030, 1, 1, 0, 0, 0, TimeSpan.Zero),
            LockoutEnabled = true,
            AccessFailedCount = 3,
            OrganizationID = "org-1",
            IsAdministrator = true,
            IsServerAdmin = false,
        };

        var result = new AspNetUserRowConverter().Convert(row);

        Assert.IsTrue(result.IsSuccess);
        var v2 = result.Value!;
        Assert.AreEqual("u-1", v2.Id);
        Assert.AreEqual("alice", v2.UserName);
        Assert.AreEqual("ALICE", v2.NormalizedUserName);
        Assert.AreEqual("alice@example.com", v2.Email);
        Assert.AreEqual("ALICE@EXAMPLE.COM", v2.NormalizedEmail);
        Assert.IsTrue(v2.EmailConfirmed);
        Assert.AreEqual("AQAAAAEAACcQAAAAEH..hash..", v2.PasswordHash);
        Assert.AreEqual("STAMP-123", v2.SecurityStamp);
        Assert.AreEqual("CC-456", v2.ConcurrencyStamp);
        Assert.AreEqual("+1-555-0100", v2.PhoneNumber);
        Assert.IsTrue(v2.PhoneNumberConfirmed);
        Assert.IsTrue(v2.TwoFactorEnabled);
        Assert.AreEqual(
            new DateTimeOffset(2030, 1, 1, 0, 0, 0, TimeSpan.Zero),
            v2.LockoutEnd);
        Assert.IsTrue(v2.LockoutEnabled);
        Assert.AreEqual(3, v2.AccessFailedCount);
        Assert.AreEqual("org-1", v2.OrganizationID);
        Assert.IsTrue(v2.IsAdministrator);
        Assert.IsFalse(v2.IsServerAdmin);
    }

    [TestMethod]
    public void Convert_DefaultsNormalizedFields_WhenSourceHasNone()
    {
        var result = new AspNetUserRowConverter().Convert(new LegacyAspNetUser
        {
            Id = "u-1",
            UserName = "Bob",
            NormalizedUserName = null,
            Email = "bob@example.com",
            NormalizedEmail = null,
            OrganizationID = "o",
        });

        Assert.IsTrue(result.IsSuccess);
        Assert.AreEqual("BOB", result.Value!.NormalizedUserName);
        Assert.AreEqual("BOB@EXAMPLE.COM", result.Value.NormalizedEmail);
    }

    [TestMethod]
    public void Convert_GeneratesConcurrencyStamp_WhenSourceIsNull()
    {
        var result = new AspNetUserRowConverter().Convert(new LegacyAspNetUser
        {
            Id = "u-1",
            UserName = "alice",
            ConcurrencyStamp = null,
            OrganizationID = "o",
        });

        Assert.IsTrue(result.IsSuccess);
        Assert.IsFalse(string.IsNullOrWhiteSpace(result.Value!.ConcurrencyStamp));
        Assert.IsTrue(Guid.TryParse(result.Value.ConcurrencyStamp, out _));
    }
}
