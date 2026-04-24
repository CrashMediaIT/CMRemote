using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.AgentUpgrade;
using System;
using System.Text.Json;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests pin the trust rules in
/// <see cref="PublisherManifestParser"/> — every change to those rules
/// breaks one of these tests on purpose.
/// </summary>
[TestClass]
public class PublisherManifestParserTests
{
    private const string SampleSha256 =
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    private static string ManifestJson(
        int schemaVersion = 1,
        string version = "1.2.3",
        string channel = "stable",
        string buildVersion = "1.2.3",
        string target = "x86_64-unknown-linux-gnu",
        string format = "deb",
        string file = "cmremote-agent_1.2.3_amd64.deb",
        long size = 12345,
        string sha256 = SampleSha256,
        string? signature = null)
    {
        var build = new
        {
            agentVersion = buildVersion,
            target,
            format,
            file,
            size,
            sha256,
            signature,
            signedBy = signature is null ? null : "ca@crashmedia.ca",
        };
        return JsonSerializer.Serialize(new
        {
            schemaVersion,
            publisher = "CrashMedia IT",
            generatedAt = "2026-04-24T00:00:00Z",
            channel,
            version,
            builds = new[] { build },
        });
    }

    [TestMethod]
    public void Parse_HappyPath_Succeeds()
    {
        var result = PublisherManifestParser.Parse(ManifestJson());
        Assert.IsTrue(result.IsSuccess, result.ErrorDetail);
        Assert.AreEqual(1, result.Manifest!.Builds.Count);
        Assert.AreEqual("stable", result.Manifest.Channel);
        Assert.AreEqual("1.2.3", result.Manifest.Version);
        Assert.AreEqual(SampleSha256, result.Manifest.Builds[0].Sha256);
    }

    [TestMethod]
    public void Parse_RejectsEmptyJson()
    {
        var result = PublisherManifestParser.Parse("");
        Assert.AreEqual(PublisherManifestParseError.InvalidJson, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsUnsupportedSchemaVersion()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(schemaVersion: 2));
        Assert.AreEqual(PublisherManifestParseError.UnsupportedSchemaVersion, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsMissingChannel()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(channel: ""));
        Assert.AreEqual(PublisherManifestParseError.MissingChannel, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsUnknownChannel()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(channel: "nightly"));
        Assert.AreEqual(PublisherManifestParseError.InvalidChannel, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsInvalidVersion()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(version: "not-semver"));
        Assert.AreEqual(PublisherManifestParseError.InvalidVersion, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsBuildVersionMismatch()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(version: "1.2.3", buildVersion: "1.2.4"));
        Assert.AreEqual(PublisherManifestParseError.InvalidBuildEntry, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsPathTraversalInFile()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(file: "../etc/passwd"));
        Assert.AreEqual(PublisherManifestParseError.InvalidBuildEntry, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsFileWithSlash()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(file: "subdir/agent.deb"));
        Assert.AreEqual(PublisherManifestParseError.InvalidBuildEntry, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsZeroSize()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(size: 0));
        Assert.AreEqual(PublisherManifestParseError.InvalidBuildEntry, result.Error);
    }

    [TestMethod]
    public void Parse_RejectsBadSha256()
    {
        var result = PublisherManifestParser.Parse(ManifestJson(sha256: "DEADBEEF"));
        Assert.AreEqual(PublisherManifestParseError.InvalidBuildEntry, result.Error);
    }

    [TestMethod]
    public void CtEqHex_EqualValues_AreEqual()
    {
        Assert.IsTrue(PublisherManifestParser.CtEqHex(SampleSha256, SampleSha256));
    }

    [TestMethod]
    public void CtEqHex_CaseInsensitive_AreEqual()
    {
        Assert.IsTrue(PublisherManifestParser.CtEqHex(SampleSha256, SampleSha256.ToUpperInvariant()));
    }

    [TestMethod]
    public void CtEqHex_DifferentValues_AreNotEqual()
    {
        var other = new string('0', 64);
        Assert.IsFalse(PublisherManifestParser.CtEqHex(SampleSha256, other));
    }

    [TestMethod]
    public void CtEqHex_DifferentLengths_AreNotEqual()
    {
        Assert.IsFalse(PublisherManifestParser.CtEqHex(SampleSha256, SampleSha256[..32]));
    }
}
