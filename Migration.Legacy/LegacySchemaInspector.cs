using System.Data.Common;
using Microsoft.Data.SqlClient;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using Npgsql;

namespace Remotely.Migration.Legacy;

/// <summary>
/// Concrete <see cref="ILegacySchemaInspector"/>. Opens the source
/// connection (SQLite / SQL Server / PostgreSQL — picked from the
/// connection-string shape per
/// <see cref="LegacyDbProviderDetector.Detect"/>) and probes for the
/// canonical upstream-Remotely table set
/// (<c>__EFMigrationsHistory</c> + <c>Organizations</c> +
/// <c>Devices</c> + <c>AspNetUsers</c>).
///
/// <para>
/// Resolution rules:
/// <list type="bullet">
///   <item><description>All four canonical tables present →
///     <see cref="LegacySchemaVersion.UpstreamLegacy_2026_04"/>.</description></item>
///   <item><description>Database connectable but contains zero
///     user-visible tables →
///     <see cref="LegacySchemaVersion.Empty"/>.</description></item>
///   <item><description>Database contains some tables but the
///     canonical set is incomplete →
///     <see cref="LegacySchemaVersion.Unknown"/>. The runner refuses
///     to import in this state because partial-set imports risk
///     silent data loss.</description></item>
/// </list>
/// </para>
///
/// <para>
/// The inspector deliberately uses raw ADO.NET (`Microsoft.Data.Sqlite`,
/// `Microsoft.Data.SqlClient`, `Npgsql`) rather than EF Core because
/// the source schema is not modelled by the v2 <c>DbContext</c> and
/// never will be — the upstream tables only exist long enough to be
/// read.
/// </para>
///
/// <para>
/// Connection-time / query-time exceptions are not swallowed here:
/// they propagate to <see cref="MigrationRunner"/> which already
/// records them as fatal errors on the report so the wizard /
/// CLI can surface a clean operator-visible message.
/// </para>
/// </summary>
public class LegacySchemaInspector : ILegacySchemaInspector
{
    /// <summary>
    /// The four tables whose simultaneous presence pins the
    /// upstream-Remotely 2026.04 schema. Matched case-insensitively
    /// because PostgreSQL folds unquoted identifiers to lower-case
    /// while SQLite / SQL Server preserve the EF-Core-generated
    /// casing.
    /// </summary>
    internal static readonly string[] CanonicalUpstreamTables =
    {
        "__EFMigrationsHistory",
        "Organizations",
        "Devices",
        "AspNetUsers",
    };

    private readonly ILogger<LegacySchemaInspector> _logger;

    public LegacySchemaInspector(ILogger<LegacySchemaInspector>? logger = null)
    {
        _logger = logger ?? NullLogger<LegacySchemaInspector>.Instance;
    }

    /// <inheritdoc />
    public async Task<LegacySchemaVersion> DetectAsync(
        string sourceConnectionString,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(sourceConnectionString))
        {
            throw new ArgumentException(
                "Source connection string was null or whitespace.",
                nameof(sourceConnectionString));
        }

        cancellationToken.ThrowIfCancellationRequested();

        var provider = LegacyDbProviderDetector.Detect(sourceConnectionString);
        _logger.LogInformation(
            "Inspecting source database via {Provider} driver.", provider);

        var tables = await ListTablesAsync(provider, sourceConnectionString, cancellationToken)
            .ConfigureAwait(false);

        return Classify(tables);
    }

    /// <summary>
    /// Pure mapping from the observed user-table set to a
    /// <see cref="LegacySchemaVersion"/>. Exposed (internal) so the
    /// classification rule is unit-testable without standing up a
    /// real database connection.
    /// </summary>
    internal static LegacySchemaVersion Classify(IReadOnlyCollection<string> tables)
    {
        if (tables.Count == 0)
        {
            return LegacySchemaVersion.Empty;
        }

        var present = new HashSet<string>(tables, StringComparer.OrdinalIgnoreCase);
        var matched = 0;
        foreach (var canonical in CanonicalUpstreamTables)
        {
            if (present.Contains(canonical))
            {
                matched++;
            }
        }

        if (matched == CanonicalUpstreamTables.Length)
        {
            return LegacySchemaVersion.UpstreamLegacy_2026_04;
        }

        // Some tables are present but the canonical set is
        // incomplete. Refuse to guess — the runner will record a
        // fatal error and the operator can either point us at a
        // different connection or proceed without an import.
        return LegacySchemaVersion.Unknown;
    }

    private async Task<IReadOnlyCollection<string>> ListTablesAsync(
        LegacyDbProvider provider,
        string connectionString,
        CancellationToken cancellationToken)
    {
        DbConnection connection = provider switch
        {
            LegacyDbProvider.Sqlite => new SqliteConnection(connectionString),
            LegacyDbProvider.SqlServer => new SqlConnection(connectionString),
            LegacyDbProvider.PostgreSql => new NpgsqlConnection(connectionString),
            _ => throw new NotSupportedException(
                $"Unsupported source connection string shape (provider={provider})."),
        };

        await using (connection.ConfigureAwait(false))
        {
            await connection.OpenAsync(cancellationToken).ConfigureAwait(false);

            await using var command = connection.CreateCommand();
            command.CommandText = TableListQueryFor(provider);

            var names = new List<string>();
            await using var reader = await command
                .ExecuteReaderAsync(cancellationToken)
                .ConfigureAwait(false);
            while (await reader.ReadAsync(cancellationToken).ConfigureAwait(false))
            {
                if (!reader.IsDBNull(0))
                {
                    names.Add(reader.GetString(0));
                }
            }
            return names;
        }
    }

    /// <summary>
    /// Returns the per-provider query that lists user-visible tables
    /// (excluding system / internal catalogues). The output column is
    /// always a single <c>name</c>-shaped column at ordinal 0.
    /// </summary>
    private static string TableListQueryFor(LegacyDbProvider provider) => provider switch
    {
        // SQLite: sqlite_master holds every table; filter out the
        // sqlite_* internal tables that exist on every database.
        LegacyDbProvider.Sqlite =>
            "SELECT name FROM sqlite_master " +
            "WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\';",

        // SQL Server: INFORMATION_SCHEMA.TABLES is portable but
        // includes system schemas; restrict to user schema 'dbo' and
        // the EF-Core history table that lives under it.
        LegacyDbProvider.SqlServer =>
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES " +
            "WHERE TABLE_TYPE = 'BASE TABLE';",

        // PostgreSQL: information_schema.tables, restricted to the
        // 'public' schema where EF Core lays down the upstream
        // Remotely tables by default.
        LegacyDbProvider.PostgreSql =>
            "SELECT table_name FROM information_schema.tables " +
            "WHERE table_schema = 'public' AND table_type = 'BASE TABLE';",

        _ => throw new NotSupportedException(
            $"No table-list query defined for provider {provider}."),
    };
}
