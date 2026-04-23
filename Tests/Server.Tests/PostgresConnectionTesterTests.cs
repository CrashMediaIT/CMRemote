using System.Threading.Tasks;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.Setup;

namespace Remotely.Server.Tests;

[TestClass]
public class PostgresConnectionTesterTests
{
    private PostgresConnectionTester _tester = null!;

    [TestInitialize]
    public void Init()
    {
        _tester = new PostgresConnectionTester(
            NullLogger<PostgresConnectionTester>.Instance);
    }

    [TestMethod]
    public async Task TestPostgres_EmptyString_ReturnsInvalid()
    {
        var result = await _tester.TestPostgresAsync(string.Empty);
        Assert.AreEqual(ConnectionTestOutcome.InvalidConnectionString, result.Outcome);
        Assert.IsFalse(result.IsSuccess);
    }

    [TestMethod]
    public async Task TestPostgres_WhitespaceString_ReturnsInvalid()
    {
        var result = await _tester.TestPostgresAsync("   ");
        Assert.AreEqual(ConnectionTestOutcome.InvalidConnectionString, result.Outcome);
    }

    [TestMethod]
    public async Task TestPostgres_MalformedString_ReturnsInvalid()
    {
        // NpgsqlConnectionStringBuilder rejects unknown keys with
        // ArgumentException; the tester must surface that as an
        // InvalidConnectionString outcome.
        var result = await _tester.TestPostgresAsync("ThisIsNotAKey=foo;AnotherBadKey=bar");
        Assert.AreEqual(ConnectionTestOutcome.InvalidConnectionString, result.Outcome,
            "An unknown key should produce InvalidConnectionString, not NetworkOrAuthFailure.");
    }

    [TestMethod]
    public async Task TestPostgres_MissingHost_ReturnsInvalid()
    {
        var result = await _tester.TestPostgresAsync("Database=cmremote;Username=u;Password=p");
        Assert.AreEqual(ConnectionTestOutcome.InvalidConnectionString, result.Outcome);
        StringAssert.Contains(result.Message, "Host");
    }

    [TestMethod]
    public async Task TestPostgres_UnreachableHost_ReturnsNetworkFailure()
    {
        // 198.51.100.0/24 is RFC 5737 TEST-NET-2 (documentation-only);
        // attempting to reach it never succeeds and never collides
        // with a real host. With a 1s connect timeout the round trip
        // resolves quickly without spamming the network.
        const string conn =
            "Host=198.51.100.1;Port=5432;Database=cmremote;" +
            "Username=u;Password=verysecret;Timeout=1;" +
            "Command Timeout=1;CancellationTimeout=500";

        var result = await _tester.TestPostgresAsync(conn);

        Assert.AreEqual(ConnectionTestOutcome.NetworkOrAuthFailure, result.Outcome,
            $"Expected NetworkOrAuthFailure, got {result.Outcome} ({result.Message}).");
        Assert.IsFalse(result.Message.Contains("verysecret"),
            "The password must be redacted from any error message returned to the wizard.");
    }
}
