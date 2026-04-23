using System.Data.Common;
using System.Runtime.CompilerServices;
using Microsoft.Data.SqlClient;
using Microsoft.Data.Sqlite;
using Npgsql;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Readers;

/// <summary>
/// <see cref="ILegacyRowReader{TLegacy}"/> for the upstream
/// <c>AspNetUsers</c> table on schema
/// <see cref="LegacySchemaVersion.UpstreamLegacy_2026_04"/>.
///
/// <para>
/// Pages keyset-style off the primary-key column <c>Id</c> (note the
/// lower-case <c>d</c> — that's the ASP.NET Identity convention,
/// distinct from the upstream <c>ID</c> capitalisation on
/// <c>Organizations</c> and <c>Devices</c>) so the per-provider
/// folding rule on PostgreSQL doesn't surprise the read order.
/// </para>
///
/// <para>
/// All ASP.NET Identity scalar columns we round-trip are read as-is;
/// password hashes / security stamps / lockout state survive the
/// migration verbatim so existing users keep their credentials.
/// </para>
/// </summary>
public class LegacyAspNetUserReader : ILegacyRowReader<LegacyAspNetUser>
{
    public string EntityName => "User";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    private static readonly IReadOnlyList<string> Columns = new[]
    {
        "Id", "UserName", "NormalizedUserName", "Email", "NormalizedEmail",
        "EmailConfirmed", "PasswordHash", "SecurityStamp", "ConcurrencyStamp",
        "PhoneNumber", "PhoneNumberConfirmed", "TwoFactorEnabled",
        "LockoutEnd", "LockoutEnabled", "AccessFailedCount",
        "OrganizationID", "IsAdministrator", "IsServerAdmin",
    };

    public async IAsyncEnumerable<LegacyAspNetUser> ReadAsync(
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

                var page = new List<LegacyAspNetUser>(batchSize);

                await using (var command = connection.CreateCommand())
                {
                    command.CommandText = LegacyKeysetSql.BuildPageQuery(
                        provider, "AspNetUsers", "Id", Columns, lastId is not null);

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
                        page.Add(new LegacyAspNetUser
                        {
                            Id = reader.GetString(0),
                            UserName = reader.IsDBNull(1) ? null : reader.GetString(1),
                            NormalizedUserName = reader.IsDBNull(2) ? null : reader.GetString(2),
                            Email = reader.IsDBNull(3) ? null : reader.GetString(3),
                            NormalizedEmail = reader.IsDBNull(4) ? null : reader.GetString(4),
                            EmailConfirmed = !reader.IsDBNull(5) && reader.GetBoolean(5),
                            PasswordHash = reader.IsDBNull(6) ? null : reader.GetString(6),
                            SecurityStamp = reader.IsDBNull(7) ? null : reader.GetString(7),
                            ConcurrencyStamp = reader.IsDBNull(8) ? null : reader.GetString(8),
                            PhoneNumber = reader.IsDBNull(9) ? null : reader.GetString(9),
                            PhoneNumberConfirmed = !reader.IsDBNull(10) && reader.GetBoolean(10),
                            TwoFactorEnabled = !reader.IsDBNull(11) && reader.GetBoolean(11),
                            LockoutEnd = reader.IsDBNull(12)
                                ? (DateTimeOffset?)null
                                : ReadDateTimeOffset(reader, 12),
                            LockoutEnabled = !reader.IsDBNull(13) && reader.GetBoolean(13),
                            AccessFailedCount = reader.IsDBNull(14) ? 0 : reader.GetInt32(14),
                            OrganizationID = reader.IsDBNull(15) ? null : reader.GetString(15),
                            IsAdministrator = !reader.IsDBNull(16) && reader.GetBoolean(16),
                            IsServerAdmin = !reader.IsDBNull(17) && reader.GetBoolean(17),
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

                lastId = page[^1].Id;
            }
        }
    }

    /// <summary>
    /// See <see cref="LegacyDeviceReader"/> for the rationale —
    /// SQLite stores DateTimeOffset as TEXT, the other providers
    /// bind native temporal types.
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
