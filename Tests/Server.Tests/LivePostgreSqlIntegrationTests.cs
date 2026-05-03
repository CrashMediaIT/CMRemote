using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Configuration;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Npgsql;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Models;

namespace Remotely.Server.Tests;

[TestClass]
public class LivePostgreSqlIntegrationTests
{
    private const string ConnectionStringEnvironmentVariable = "CMREMOTE_POSTGRES_CONNECTION_STRING";

    [TestMethod]
    [TestCategory("PostgreSqlIntegration")]
    public async Task PostgreSqlProvider_AppliesMigrations_AndPersistsCriticalRows()
    {
        var baseConnectionString = Environment.GetEnvironmentVariable(ConnectionStringEnvironmentVariable);
        if (string.IsNullOrWhiteSpace(baseConnectionString))
        {
            Assert.Inconclusive(
                $"{ConnectionStringEnvironmentVariable} is not set; skipping live PostgreSQL integration coverage.");
            return;
        }

        var databaseName = "cmremote_it_" + Guid.NewGuid().ToString("N");
        var databaseConnectionString = WithDatabase(baseConnectionString, databaseName);

        await CreateDatabaseAsync(baseConnectionString, databaseName);
        try
        {
            var configuration = new ConfigurationBuilder()
                .AddInMemoryCollection(new Dictionary<string, string?>
                {
                    ["ApplicationOptions:DbProvider"] = "PostgreSql",
                    ["ConnectionStrings:PostgreSQL"] = databaseConnectionString,
                })
                .Build();

            await using var db = new PostgreSqlDbContext(configuration, new DesignTimeWebHostEnvironment());
            await db.Database.MigrateAsync();

            var organizationId = Guid.NewGuid().ToString();
            var deviceId = "pg-device-" + Guid.NewGuid().ToString("N");
            var occurredAt = new DateTimeOffset(2026, 5, 3, 5, 45, 0, TimeSpan.Zero);
            var lastOnline = new DateTimeOffset(2026, 5, 3, 5, 40, 0, TimeSpan.Zero);

            db.Organizations.Add(new Organization
            {
                ID = organizationId,
                OrganizationName = "PostgreSQL Integration",
            });
            db.Devices.Add(new Device
            {
                ID = deviceId,
                OrganizationID = organizationId,
                DeviceName = "postgres-live-device",
                LastOnline = lastOnline,
                MacAddresses = ["00-11-22-33-44-55"],
                Drives =
                [
                    new Drive
                    {
                        Name = "System",
                        RootDirectory = "/",
                        DriveFormat = "ext4",
                        TotalSize = 1024,
                        FreeSpace = 512,
                    },
                ],
            });
            db.AgentUpgradeStatuses.Add(new AgentUpgradeStatus
            {
                DeviceId = deviceId,
                OrganizationID = organizationId,
                FromVersion = "1.0.0",
                ToVersion = "1.1.0",
                State = AgentUpgradeState.Pending,
                CreatedAt = occurredAt,
                EligibleAt = occurredAt,
            });
            db.AuditLogEntries.Add(new AuditLogEntry
            {
                OrganizationID = organizationId,
                Sequence = 1,
                OccurredAt = occurredAt,
                EventType = "postgres.integration",
                ActorId = "system",
                SubjectId = deviceId,
                Summary = "Live PostgreSQL integration smoke test",
                DetailJson = "{\"provider\":\"postgresql\"}",
                PrevHash = new string('0', 64),
                EntryHash = new string('a', 64),
            });
            await db.SaveChangesAsync();

            db.ChangeTracker.Clear();

            var device = await db.Devices.AsNoTracking().SingleAsync(x => x.ID == deviceId);
            Assert.AreEqual("postgres-live-device", device.DeviceName);
            CollectionAssert.AreEqual(
                new[] { "00-11-22-33-44-55" },
                device.MacAddresses);
            Assert.AreEqual("/", device.Drives?.Single().RootDirectory);
            Assert.AreEqual(lastOnline, device.LastOnline);

            var upgrade = await db.AgentUpgradeStatuses.AsNoTracking().SingleAsync(x => x.DeviceId == deviceId);
            Assert.AreEqual(AgentUpgradeState.Pending, upgrade.State);
            Assert.AreEqual("1.1.0", upgrade.ToVersion);

            var audit = await db.AuditLogEntries.AsNoTracking().SingleAsync(x => x.OrganizationID == organizationId);
            Assert.AreEqual("postgres.integration", audit.EventType);
            Assert.AreEqual(occurredAt, audit.OccurredAt);
        }
        finally
        {
            await DropDatabaseAsync(baseConnectionString, databaseName);
        }
    }

    private static async Task CreateDatabaseAsync(string baseConnectionString, string databaseName)
    {
        await using var connection = new NpgsqlConnection(WithDatabase(baseConnectionString, "postgres"));
        await connection.OpenAsync();
        await using var command = connection.CreateCommand();
        command.CommandText = $"CREATE DATABASE {QuoteIdentifier(databaseName)}";
        await command.ExecuteNonQueryAsync();
    }

    private static async Task DropDatabaseAsync(string baseConnectionString, string databaseName)
    {
        await using var connection = new NpgsqlConnection(WithDatabase(baseConnectionString, "postgres"));
        await connection.OpenAsync();
        await using var command = connection.CreateCommand();
        command.CommandText = $"DROP DATABASE IF EXISTS {QuoteIdentifier(databaseName)}";
        await command.ExecuteNonQueryAsync();
    }

    private static string WithDatabase(string connectionString, string databaseName)
    {
        var builder = new NpgsqlConnectionStringBuilder(connectionString)
        {
            Database = databaseName,
        };
        return builder.ConnectionString;
    }

    private static string QuoteIdentifier(string identifier)
    {
        return "\"" + identifier.Replace("\"", "\"\"", StringComparison.Ordinal) + "\"";
    }
}
