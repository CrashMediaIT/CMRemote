using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class OrganizationRowConverterTests
{
    private readonly OrganizationRowConverter _converter = new();

    [TestMethod]
    public void Convert_HappyPath_PreservesIdentityAndName()
    {
        var legacy = new LegacyOrganization
        {
            ID = "org-abc",
            OrganizationName = "Acme",
            IsDefaultOrganization = true,
        };

        var result = _converter.Convert(legacy);

        Assert.IsTrue(result.IsSuccess);
        Assert.IsNotNull(result.Value);
        // Identity preservation is load-bearing per ROADMAP M1.3 — every
        // device whose OrganizationID points at this row must continue
        // to find it under the same key after the migration.
        Assert.AreEqual("org-abc", result.Value!.ID);
        Assert.AreEqual("Acme", result.Value.OrganizationName);
        Assert.IsTrue(result.Value.IsDefaultOrganization);
    }

    [TestMethod]
    public void Convert_NullRow_FailsLoudly()
    {
        var result = _converter.Convert(null!);
        Assert.IsTrue(result.IsFailure);
        Assert.IsNotNull(result.ErrorMessage);
    }

    [TestMethod]
    public void Convert_MissingId_FailsLoudly()
    {
        var legacy = new LegacyOrganization { ID = "  ", OrganizationName = "Acme" };
        var result = _converter.Convert(legacy);
        Assert.IsTrue(result.IsFailure);
    }

    [TestMethod]
    public void Convert_MissingName_SkipsRow()
    {
        // Per the converter docstring: orgs missing a name are typically
        // half-deleted test rows. They are skipped (counted, not
        // written) rather than aborting the whole run.
        var legacy = new LegacyOrganization { ID = "org-1", OrganizationName = null };
        var result = _converter.Convert(legacy);
        Assert.IsTrue(result.IsSkipped);
        Assert.IsFalse(result.IsFailure);
        Assert.IsNotNull(result.SkipReason);
    }

    [TestMethod]
    public void Convert_LongName_TruncatesToV2CapAndStillSucceeds()
    {
        // v2 enforces a 25-char cap. Truncate rather than skip — losing
        // a whole org over a long name is worse than truncating a name.
        var legacy = new LegacyOrganization
        {
            ID = "org-1",
            OrganizationName = new string('a', 60),
        };

        var result = _converter.Convert(legacy);

        Assert.IsTrue(result.IsSuccess);
        Assert.AreEqual(25, result.Value!.OrganizationName.Length);
    }

    [TestMethod]
    public void Convert_TrimsSurroundingWhitespace()
    {
        var legacy = new LegacyOrganization { ID = "org-1", OrganizationName = "  Acme  " };
        var result = _converter.Convert(legacy);
        Assert.IsTrue(result.IsSuccess);
        Assert.AreEqual("Acme", result.Value!.OrganizationName);
    }

    [TestMethod]
    public void Converter_AdvertisesEntityNameAndSchemaVersion()
    {
        Assert.AreEqual("Organization", _converter.EntityName);
        Assert.AreEqual(LegacySchemaVersion.UpstreamLegacy_2026_04, _converter.HandlesSchemaVersion);
    }
}
