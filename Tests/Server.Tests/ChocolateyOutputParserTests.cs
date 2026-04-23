using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Shared.PackageManager;
using System.Linq;

namespace Remotely.Server.Tests;

[TestClass]
public class ChocolateyOutputParserTests
{
    [TestMethod]
    public void IsSuccessExitCode_AcceptsZero()
    {
        Assert.IsTrue(ChocolateyOutputParser.IsSuccessExitCode(0));
    }

    [TestMethod]
    public void IsSuccessExitCode_AcceptsRebootCodes()
    {
        // 1641 = "reboot initiated", 3010 = "reboot required" — both
        // mean the operation succeeded; pending reboot is the OS's,
        // not the package manager's, problem.
        Assert.IsTrue(ChocolateyOutputParser.IsSuccessExitCode(1641));
        Assert.IsTrue(ChocolateyOutputParser.IsSuccessExitCode(3010));
    }

    [TestMethod]
    public void IsSuccessExitCode_RejectsArbitraryNonZero()
    {
        Assert.IsFalse(ChocolateyOutputParser.IsSuccessExitCode(1));
        Assert.IsFalse(ChocolateyOutputParser.IsSuccessExitCode(-1));
        Assert.IsFalse(ChocolateyOutputParser.IsSuccessExitCode(2));
    }

    [TestMethod]
    public void ParseListOutput_EmptyOrWhitespace_ReturnsEmpty()
    {
        Assert.AreEqual(0, ChocolateyOutputParser.ParseListOutput(null).Count);
        Assert.AreEqual(0, ChocolateyOutputParser.ParseListOutput(string.Empty).Count);
        Assert.AreEqual(0, ChocolateyOutputParser.ParseListOutput("   \r\n\t").Count);
    }

    [TestMethod]
    public void ParseListOutput_LimitOutputFormat_ParsesIdAndVersion()
    {
        var output = """
            googlechrome|120.0.6099.130
            7zip|22.1.0.20220715
            git|2.43.0
            """;

        var packages = ChocolateyOutputParser.ParseListOutput(output);

        Assert.AreEqual(3, packages.Count);
        Assert.AreEqual("googlechrome", packages[0].Id);
        Assert.AreEqual("120.0.6099.130", packages[0].Version);
        Assert.AreEqual("7zip", packages[1].Id);
        Assert.AreEqual("git", packages[2].Id);
    }

    [TestMethod]
    public void ParseListOutput_LegacyV1Format_StripsBannerAndSummary()
    {
        // Reproduces the v1 shape that some hosts still emit even with
        // --limit-output: a "Chocolatey vX.Y.Z" banner up top and a
        // "N packages installed." footer.
        var output = """
            Chocolatey v1.4.0
            googlechrome 120.0.6099.130
            7zip 22.1.0.20220715
            2 packages installed.
            """;

        var packages = ChocolateyOutputParser.ParseListOutput(output);

        Assert.AreEqual(2, packages.Count);
        CollectionAssert.AreEquivalent(
            new[] { "googlechrome", "7zip" },
            packages.Select(p => p.Id).ToArray());
    }

    [TestMethod]
    public void ParseListOutput_IgnoresMalformedLines()
    {
        // Malformed lines (no pipe + no version-looking second token,
        // or trailing pipe with empty version) must not throw and must
        // not produce stub entries.
        var output = """
            valid|1.2.3

            random text without version
            badpipe|
            """;

        var packages = ChocolateyOutputParser.ParseListOutput(output);

        Assert.AreEqual(1, packages.Count);
        Assert.AreEqual("valid", packages[0].Id);
        Assert.AreEqual("1.2.3", packages[0].Version);
    }
}
