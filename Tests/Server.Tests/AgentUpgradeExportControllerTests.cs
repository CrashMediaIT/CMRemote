using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.API;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Enums;
using System;
using System.Collections.Generic;
using System.Text;

namespace Remotely.Server.Tests;

/// <summary>
/// Format-only tests for the M4 dashboard's CSV export
/// (<see cref="AgentUpgradeExportController.BuildCsv"/>). The HTTP
/// pipeline is exercised by integration tests; these guard the
/// operator-visible columns + RFC 4180 escaping rules so a code change
/// that drifts the format breaks the build rather than silently
/// shipping a malformed download.
/// </summary>
[TestClass]
public class AgentUpgradeExportControllerTests
{
    [TestMethod]
    public void BuildCsv_EmptyRows_ReturnsHeaderOnly_WithUtf8Bom()
    {
        var bytes = AgentUpgradeExportController.BuildCsv(Array.Empty<AgentUpgradeRow>());
        Assert.IsTrue(bytes.Length >= 3, "Expected UTF-8 BOM preamble.");
        Assert.AreEqual(0xEF, bytes[0]);
        Assert.AreEqual(0xBB, bytes[1]);
        Assert.AreEqual(0xBF, bytes[2]);
        var text = Encoding.UTF8.GetString(bytes, 3, bytes.Length - 3);
        Assert.IsTrue(text.StartsWith(
            "DeviceId,DeviceName,State,FromVersion,ToVersion,LastOnlineUtc,AttemptCount,EligibleAtUtc,LastAttemptAtUtc,CompletedAtUtc,LastAttemptError"),
            $"Unexpected header: {text}");
    }

    [TestMethod]
    public void BuildCsv_EscapesCommasQuotesAndNewlinesPerRfc4180()
    {
        var row = new AgentUpgradeRow(
            Id: Guid.NewGuid(),
            DeviceId: "d-1",
            OrganizationId: "o-1",
            DeviceName: "name, with \"quotes\"\nand newline",
            LastOnline: new DateTimeOffset(2026, 4, 24, 1, 2, 3, TimeSpan.Zero),
            FromVersion: "1.0",
            ToVersion: "2.0",
            State: AgentUpgradeState.Failed,
            CreatedAt: new DateTimeOffset(2026, 4, 23, 0, 0, 0, TimeSpan.Zero),
            EligibleAt: new DateTimeOffset(2026, 4, 24, 0, 0, 0, TimeSpan.Zero),
            LastAttemptAt: null,
            CompletedAt: null,
            LastAttemptError: "boom",
            AttemptCount: 3);
        var bytes = AgentUpgradeExportController.BuildCsv(new List<AgentUpgradeRow> { row });
        var text = Encoding.UTF8.GetString(bytes, 3, bytes.Length - 3);
        // The DeviceName field must be quoted with internal quotes doubled
        // and the embedded newline preserved inside the quoted field.
        StringAssert.Contains(text, "\"name, with \"\"quotes\"\"\nand newline\"");
        // State + integer column round-trip unquoted.
        StringAssert.Contains(text, ",Failed,");
        StringAssert.Contains(text, ",3,");
        // UTC timestamp is rendered with the ISO 8601 'u' format ("2026-04-24 01:02:03Z").
        StringAssert.Contains(text, "2026-04-24 01:02:03Z");
    }

    [TestMethod]
    public void BuildCsv_NullableFieldsRenderAsEmpty()
    {
        var row = new AgentUpgradeRow(
            Id: Guid.NewGuid(),
            DeviceId: "d-1",
            OrganizationId: "o-1",
            DeviceName: null,
            LastOnline: null,
            FromVersion: null,
            ToVersion: null,
            State: AgentUpgradeState.Pending,
            CreatedAt: DateTimeOffset.UnixEpoch,
            EligibleAt: DateTimeOffset.UnixEpoch,
            LastAttemptAt: null,
            CompletedAt: null,
            LastAttemptError: null,
            AttemptCount: 0);
        var bytes = AgentUpgradeExportController.BuildCsv(new List<AgentUpgradeRow> { row });
        var text = Encoding.UTF8.GetString(bytes, 3, bytes.Length - 3);
        // Header line + one data line + trailing newline.
        var lines = text.Split('\n');
        Assert.IsTrue(lines.Length >= 2);
        var data = lines[1];
        // DeviceId,DeviceName(empty),State,FromVersion(empty),ToVersion(empty),LastOnlineUtc(empty),0,EligibleAtUtc,LastAttemptAt(empty),CompletedAt(empty),LastAttemptError(empty)
        StringAssert.StartsWith(data, "d-1,,Pending,,,,0,");
        StringAssert.EndsWith(data, ",,,");
    }
}
