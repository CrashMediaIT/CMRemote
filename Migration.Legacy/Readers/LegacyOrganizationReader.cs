using System.Data.Common;
using System.Runtime.CompilerServices;
using Microsoft.Data.SqlClient;
using Microsoft.Data.Sqlite;
using Npgsql;
using Remotely.Migration.Legacy.Sources;

namespace Remotely.Migration.Legacy.Readers;

/// <summary>
/// <see cref="ILegacyRowReader{TLegacy}"/> for the upstream
/// <c>Organizations</c> table on schema
/// <see cref="LegacySchemaVersion.UpstreamLegacy_2026_04"/>.
///
/// <para>
/// Pages off the source connection in <see cref="MigrationOptions.BatchSize"/>
/// chunks ordered by <c>ID</c>, populating
/// <see cref="LegacyOrganization"/> POCOs that are then handed to the
/// matching <see cref="Converters.OrganizationRowConverter"/> by the
/// runner. Driver is picked from the connection-string shape via
/// <see cref="LegacyDbProviderDetector"/> so the same reader works
/// against SQLite (the upstream Docker default), SQL Server, and
/// PostgreSQL.
/// </para>
///
/// <para>
/// The pagination key is the primary-key column <c>ID</c>; this
/// makes the read order deterministic across runs (so a resumed
/// import sees the same sequence and can skip already-written rows by
/// id once the target writer lands in the next M2 slice).
/// </para>
/// </summary>
public class LegacyOrganizationReader : ILegacyRowReader<LegacyOrganization>
{
    public string EntityName => "Organization";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public async IAsyncEnumerable<LegacyOrganization> ReadAsync(
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

        // Honour an already-cancelled token before opening the
        // connection so callers see a deterministic
        // OperationCanceledException rather than a driver-specific
        // TaskCanceledException out of OpenAsync.
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

                var page = new List<LegacyOrganization>(batchSize);

                await using (var command = connection.CreateCommand())
                {
                    command.CommandText = PageQueryFor(provider, lastId is not null);

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
                        var id = reader.GetString(0);
                        var name = reader.IsDBNull(1) ? null : reader.GetString(1);
                        var isDefault = !reader.IsDBNull(2) && reader.GetBoolean(2);

                        page.Add(new LegacyOrganization
                        {
                            ID = id,
                            OrganizationName = name,
                            IsDefaultOrganization = isDefault,
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
    /// Returns a per-provider keyset-paginated SELECT against
    /// <c>Organizations</c>. Pagination is keyset rather than
    /// OFFSET/LIMIT because the latter degrades on large tables and
    /// can re-read rows when the underlying data shifts mid-import.
    ///
    /// All three providers accept the <c>@name</c> ADO.NET parameter
    /// prefix (Npgsql treats it as an alias for its native
    /// <c>:name</c> form), so the placeholder is uniform across
    /// providers and only the surrounding SQL dialect varies.
    /// </summary>
    private static string PageQueryFor(LegacyDbProvider provider, bool hasCursor)
    {
        // Identifiers are quoted because PostgreSQL folds unquoted
        // identifiers to lower-case, while EF Core lays the table
        // down with the C#-cased name 'Organizations'.
        var (table, idCol, nameCol, defaultCol) = provider switch
        {
            LegacyDbProvider.Sqlite =>
                ("\"Organizations\"", "\"ID\"", "\"OrganizationName\"", "\"IsDefaultOrganization\""),
            LegacyDbProvider.SqlServer =>
                ("[Organizations]", "[ID]", "[OrganizationName]", "[IsDefaultOrganization]"),
            LegacyDbProvider.PostgreSql =>
                ("\"Organizations\"", "\"ID\"", "\"OrganizationName\"", "\"IsDefaultOrganization\""),
            _ => throw new NotSupportedException(
                $"No page query defined for provider {provider}."),
        };

        var where = hasCursor ? $"WHERE {idCol} > @lastId " : string.Empty;

        return provider switch
        {
            // SQL Server uses TOP rather than LIMIT.
            LegacyDbProvider.SqlServer =>
                $"SELECT TOP(@batch) {idCol}, {nameCol}, {defaultCol} " +
                $"FROM {table} {where}ORDER BY {idCol};",

            // SQLite + PostgreSQL share LIMIT syntax.
            _ =>
                $"SELECT {idCol}, {nameCol}, {defaultCol} " +
                $"FROM {table} {where}ORDER BY {idCol} LIMIT @batch;",
        };
    }
}
