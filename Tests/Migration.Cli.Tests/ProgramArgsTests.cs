using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Cli;
using Remotely.Migration.Legacy;

namespace Remotely.Migration.Cli.Tests;

[TestClass]
public class ProgramArgsTests
{
    [TestMethod]
    public void TryParseArgs_HappyPath_BindsAllFields()
    {
        var ok = Program.TryParseArgs(
            new[] { "--from", "src", "--to", "dst", "--dry-run", "--batch-size", "250" },
            out var parsed, out var error);

        Assert.IsTrue(ok, error);
        Assert.AreEqual("src", parsed.From);
        Assert.AreEqual("dst", parsed.To);
        Assert.IsTrue(parsed.DryRun);
        Assert.AreEqual(250, parsed.BatchSize);
    }

    [TestMethod]
    public void TryParseArgs_ShortFlags_AreAccepted()
    {
        var ok = Program.TryParseArgs(
            new[] { "-f", "src", "-t", "dst" },
            out var parsed, out var _);

        Assert.IsTrue(ok);
        Assert.AreEqual("src", parsed.From);
        Assert.AreEqual("dst", parsed.To);
        Assert.IsFalse(parsed.DryRun);
        Assert.AreEqual(500, parsed.BatchSize);
    }

    [TestMethod]
    public void TryParseArgs_MissingFrom_Fails()
    {
        var ok = Program.TryParseArgs(
            new[] { "--to", "dst" },
            out _, out var error);
        Assert.IsFalse(ok);
        StringAssert.Contains(error!, "--from");
    }

    [TestMethod]
    public void TryParseArgs_MissingTo_Fails()
    {
        var ok = Program.TryParseArgs(
            new[] { "--from", "src" },
            out _, out var error);
        Assert.IsFalse(ok);
        StringAssert.Contains(error!, "--to");
    }

    [TestMethod]
    public void TryParseArgs_FlagWithoutValue_Fails()
    {
        var ok = Program.TryParseArgs(
            new[] { "--from" },
            out _, out var error);
        Assert.IsFalse(ok);
        StringAssert.Contains(error!, "--from");
    }

    [TestMethod]
    public void TryParseArgs_NonPositiveBatch_Fails()
    {
        var ok = Program.TryParseArgs(
            new[] { "--from", "a", "--to", "b", "--batch-size", "0" },
            out _, out var error);
        Assert.IsFalse(ok);
        StringAssert.Contains(error!, "positive");
    }

    [TestMethod]
    public void TryParseArgs_NonIntegerBatch_Fails()
    {
        var ok = Program.TryParseArgs(
            new[] { "--from", "a", "--to", "b", "--batch-size", "huge" },
            out _, out var error);
        Assert.IsFalse(ok);
        StringAssert.Contains(error!, "positive");
    }

    [TestMethod]
    public void TryParseArgs_UnknownFlag_Fails()
    {
        var ok = Program.TryParseArgs(
            new[] { "--from", "a", "--to", "b", "--turbo" },
            out _, out var error);
        Assert.IsFalse(ok);
        StringAssert.Contains(error!, "--turbo");
    }

    [TestMethod]
    public void ComputeExitCode_NoFatalsNoFailures_IsZero()
    {
        var report = new MigrationReport
        {
            Entities =
            {
                new EntityReport { EntityName = "Organization", RowsRead = 1, RowsConverted = 1, RowsWritten = 1 },
            },
        };
        Assert.AreEqual(0, Program.ComputeExitCode(report));
    }

    [TestMethod]
    public void ComputeExitCode_RowFailures_IsOne()
    {
        var report = new MigrationReport
        {
            Entities =
            {
                new EntityReport { EntityName = "Organization", RowsRead = 2, RowsConverted = 1, RowsFailed = 1 },
            },
        };
        Assert.AreEqual(1, Program.ComputeExitCode(report));
    }

    [TestMethod]
    public void ComputeExitCode_FatalErrorsTrumpRowFailures_IsTwo()
    {
        var report = new MigrationReport
        {
            FatalErrors = { "Detection threw" },
            Entities =
            {
                // RowsFailed > 0 but a fatal already happened — fatal wins.
                new EntityReport { EntityName = "x", RowsFailed = 5 },
            },
        };
        Assert.AreEqual(2, Program.ComputeExitCode(report));
    }
}
