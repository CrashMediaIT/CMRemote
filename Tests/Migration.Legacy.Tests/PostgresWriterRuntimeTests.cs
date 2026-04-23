using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy.Writers;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class PostgresWriterRuntimeTests
{
    [TestMethod]
    [DataRow(null)]
    [DataRow("")]
    [DataRow("   ")]
    public void ValidateAndCreate_BlankString_Throws(string? conn)
    {
        Assert.ThrowsException<ArgumentException>(
            () => PostgresWriterRuntime.ValidateAndCreate(conn!));
    }

    [TestMethod]
    [DataRow("Data Source=foo.db")]   // SQLite-shape
    [DataRow("Server=.;Database=Foo")] // SQL Server-shape
    public void ValidateAndCreate_NonPostgresShape_Throws(string conn)
    {
        var ex = Assert.ThrowsException<NotSupportedException>(
            () => PostgresWriterRuntime.ValidateAndCreate(conn));
        StringAssert.Contains(ex.Message, "PostgreSQL");
    }

    [TestMethod]
    public void ValidateAndCreate_PostgresShape_ReturnsConnection()
    {
        // Don't open it — just exercise the validator. The connection
        // string isn't reachable in CI but Npgsql parses it lazily.
        using var conn = PostgresWriterRuntime.ValidateAndCreate(
            "Host=localhost;Database=cmremote_v2;Username=postgres");
        Assert.IsNotNull(conn);
    }
}
