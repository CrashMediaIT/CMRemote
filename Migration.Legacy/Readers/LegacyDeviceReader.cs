using System.Data.Common;
using System.Runtime.CompilerServices;
using Microsoft.Data.SqlClient;
using Microsoft.Data.Sqlite;
using Npgsql;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Readers;

/// <summary>
/// <see cref="ILegacyRowReader{TLegacy}"/> for the upstream
/// <c>Devices</c> table on schema
/// <see cref="LegacySchemaVersion.UpstreamLegacy_2026_04"/>.
///
/// <para>
/// Mirrors <see cref="LegacyOrganizationReader"/>'s shape: pages
/// keyset-style off <c>ID</c> across SQLite / SQL Server / PostgreSQL,
/// projects the minimal scalar subset the converter consumes
/// (telemetry counters that the live agent re-fills on next check-in
/// are intentionally read so existing online/offline state is not
/// lost; the rich JSON columns <c>Drives</c> + <c>MacAddresses</c>
/// are intentionally skipped because their per-provider marshalling
/// would balloon the reader without buying anything that survives
/// the next agent check-in anyway).
/// </para>
/// </summary>
public class LegacyDeviceReader : ILegacyRowReader<LegacyDevice>
{
    public string EntityName => "Device";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    private static readonly IReadOnlyList<string> Columns = new[]
    {
        "ID", "OrganizationID", "DeviceName", "Alias", "Tags", "Notes",
        "Platform", "OSDescription", "AgentVersion", "CurrentUser",
        "PublicIP", "DeviceGroupID", "ServerVerificationToken",
        "Is64Bit", "IsOnline", "LastOnline", "ProcessorCount",
        "CpuUtilization", "TotalMemory", "UsedMemory", "TotalStorage",
        "UsedStorage", "OSArchitecture",
    };

    public async IAsyncEnumerable<LegacyDevice> ReadAsync(
        string sourceConnectionString,
        int batchSize,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(sourceConnectionString))
        {
            throw new ArgumentException(
                "Source connection string was null or whitespace.",
                nameof(sourceConnectionString));
        }

        if (batchSize <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(batchSize),
                "Batch size must be a positive integer.");
        }

        var provider = LegacyDbProviderDetector.Detect(sourceConnectionString);
        cancellationToken.ThrowIfCancellationRequested();

        DbConnection connection = provider switch
        {
            LegacyDbProvider.Sqlite => new SqliteConnection(sourceConnectionString),
            LegacyDbProvider.SqlServer => new SqlConnection(sourceConnectionString),
            LegacyDbProvider.PostgreSql => new NpgsqlConnection(sourceConnectionString),
            _ => throw new NotSupportedException(
                $"Unsupported source connection string shape (provider={provider})."),
        };

        await using (connection.ConfigureAwait(false))
        {
            await connection.OpenAsync(cancellationToken).ConfigureAwait(false);

            string? lastId = null;
            while (true)
            {
                cancellationToken.ThrowIfCancellationRequested();

                var page = new List<LegacyDevice>(batchSize);

                await using (var command = connection.CreateCommand())
                {
                    command.CommandText = LegacyKeysetSql.BuildPageQuery(
                        provider, "Devices", "ID", Columns, lastId is not null);

                    var batchParam = command.CreateParameter();
                    batchParam.ParameterName = "@batch";
                    batchParam.Value = batchSize;
                    command.Parameters.Add(batchParam);

                    if (lastId is not null)
                    {
                        var lastParam = command.CreateParameter();
                        lastParam.ParameterName = "@lastId";
                        lastParam.Value = lastId;
                        command.Parameters.Add(lastParam);
                    }

                    await using var reader = await command
                        .ExecuteReaderAsync(cancellationToken)
                        .ConfigureAwait(false);

                    while (await reader.ReadAsync(cancellationToken).ConfigureAwait(false))
                    {
                        page.Add(new LegacyDevice
                        {
                            ID = reader.GetString(0),
                            OrganizationID = reader.IsDBNull(1) ? null : reader.GetString(1),
                            DeviceName = reader.IsDBNull(2) ? null : reader.GetString(2),
                            Alias = reader.IsDBNull(3) ? null : reader.GetString(3),
                            Tags = reader.IsDBNull(4) ? null : reader.GetString(4),
                            Notes = reader.IsDBNull(5) ? null : reader.GetString(5),
                            Platform = reader.IsDBNull(6) ? null : reader.GetString(6),
                            OSDescription = reader.IsDBNull(7) ? null : reader.GetString(7),
                            AgentVersion = reader.IsDBNull(8) ? null : reader.GetString(8),
                            CurrentUser = reader.IsDBNull(9) ? null : reader.GetString(9),
                            PublicIP = reader.IsDBNull(10) ? null : reader.GetString(10),
                            DeviceGroupID = reader.IsDBNull(11) ? null : reader.GetString(11),
                            ServerVerificationToken = reader.IsDBNull(12) ? null : reader.GetString(12),
                            Is64Bit = !reader.IsDBNull(13) && reader.GetBoolean(13),
                            IsOnline = !reader.IsDBNull(14) && reader.GetBoolean(14),
                            LastOnline = reader.IsDBNull(15)
                                ? default
                                : ReadDateTimeOffset(reader, 15),
                            ProcessorCount = reader.IsDBNull(16) ? 0 : reader.GetInt32(16),
                            CpuUtilization = reader.IsDBNull(17) ? 0.0 : reader.GetDouble(17),
                            TotalMemory = reader.IsDBNull(18) ? 0.0 : reader.GetDouble(18),
                            UsedMemory = reader.IsDBNull(19) ? 0.0 : reader.GetDouble(19),
                            TotalStorage = reader.IsDBNull(20) ? 0.0 : reader.GetDouble(20),
                            UsedStorage = reader.IsDBNull(21) ? 0.0 : reader.GetDouble(21),
                            OSArchitecture = reader.IsDBNull(22) ? 0 : reader.GetInt32(22),
                        });
                    }
                }

                if (page.Count == 0)
                {
                    yield break;
                }

                foreach (var row in page)
                {
                    yield return row;
                }

                if (page.Count < batchSize)
                {
                    yield break;
                }

                lastId = page[^1].ID;
            }
        }
    }

    /// <summary>
    /// SQLite stores DateTimeOffset as TEXT, while SQL Server and
    /// PostgreSQL bind to native temporal types — so a naive
    /// <c>reader.GetDateTime</c> call can fail on SQLite when the
    /// column was written as ISO-8601 text. Handle both cases here so
    /// the reader stays provider-agnostic.
    /// </summary>
    private static DateTimeOffset ReadDateTimeOffset(DbDataReader reader, int ordinal)
    {
        var raw = reader.GetValue(ordinal);
        return raw switch
        {
            DateTimeOffset dto => dto,
            DateTime dt => new DateTimeOffset(
                DateTime.SpecifyKind(dt, DateTimeKind.Utc), TimeSpan.Zero),
            string s => DateTimeOffset.Parse(s,
                System.Globalization.CultureInfo.InvariantCulture,
                System.Globalization.DateTimeStyles.AssumeUniversal
                | System.Globalization.DateTimeStyles.AdjustToUniversal),
            _ => default,
        };
    }
}
