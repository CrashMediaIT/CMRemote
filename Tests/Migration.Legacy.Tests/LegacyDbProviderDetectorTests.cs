using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Migration.Legacy;

namespace Remotely.Migration.Legacy.Tests;

[TestClass]
public class LegacyDbProviderDetectorTests
{
    [DataTestMethod]
    [DataRow("Host=localhost;Database=remotely;Username=u;Password=p", LegacyDbProvider.PostgreSql)]
    [DataRow("HOST=db.internal;Port=5432;Database=remotely", LegacyDbProvider.PostgreSql)]
    [DataRow(" Host = db ; Database = x ", LegacyDbProvider.PostgreSql)]
    [DataRow("Server=tcp:sql.example.com,1433;Database=remotely;User ID=u;Password=p", LegacyDbProvider.SqlServer)]
    [DataRow("Initial Catalog=remotely;Data Source=sql;User ID=u;Password=p", LegacyDbProvider.SqlServer)]
    [DataRow("server=.;database=remotely;trusted_connection=true", LegacyDbProvider.SqlServer)]
    [DataRow("Data Source=Remotely.db", LegacyDbProvider.Sqlite)]
    [DataRow("Data Source=:memory:", LegacyDbProvider.Sqlite)]
    [DataRow("DataSource=foo.db", LegacyDbProvider.Sqlite)]
    [DataRow("Filename=foo.db", LegacyDbProvider.Sqlite)]
    [DataRow("Data Source=cmremote;Mode=Memory;Cache=Shared", LegacyDbProvider.Sqlite)]
    public void Detect_RecognisedShape_PicksExpectedProvider(string conn, LegacyDbProvider expected)
    {
        Assert.AreEqual(expected, LegacyDbProviderDetector.Detect(conn));
    }

    [TestMethod]
    public void Detect_HostNameSubstring_DoesNotFalseMatchPostgreSql()
    {
        // 'HostName' is not the Npgsql 'Host' key — must not be
        // misclassified as PostgreSQL.
        var result = LegacyDbProviderDetector.Detect(
            "Server=.;Database=remotely;HostName=ignored");
        Assert.AreEqual(LegacyDbProvider.SqlServer, result);
    }

    [TestMethod]
    public void Detect_NullOrWhitespace_Throws()
    {
        Assert.ThrowsException<ArgumentException>(() => LegacyDbProviderDetector.Detect(""));
        Assert.ThrowsException<ArgumentException>(() => LegacyDbProviderDetector.Detect("   "));
    }

    [TestMethod]
    public void Detect_UnrecognisedShape_Throws()
    {
        Assert.ThrowsException<NotSupportedException>(
            () => LegacyDbProviderDetector.Detect("Provider=SQLOLEDB;DataSrc=foo"));
    }
}
