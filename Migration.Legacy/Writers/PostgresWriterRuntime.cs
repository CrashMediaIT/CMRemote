using Npgsql;

namespace Remotely.Migration.Legacy.Writers;

/// <summary>
/// Common Postgres-target plumbing for the M2 row writers.
///
/// <para>
/// Every concrete writer (<see cref="LegacyOrganizationWriter"/>,
/// <see cref="LegacyDeviceWriter"/>, <see cref="LegacyUserWriter"/>)
/// follows the same shape: open an <see cref="NpgsqlConnection"/>,
/// run a single <c>INSERT … ON CONFLICT (PK) DO UPDATE SET …</c>
/// upsert keyed off the v2 primary key, and let Npgsql's built-in
/// connection pooling amortise the cost across the streamed rows.
/// </para>
///
/// <para>
/// The upsert is keyed off the <b>v2 primary key</b> (which the
/// converters preserve byte-stable from the upstream row per ROADMAP
/// M1.3) so re-running the import after a partial failure does not
/// duplicate rows — the conflict path overwrites the previously
/// written row in place. This is the contract
/// <see cref="ILegacyRowWriter{TV2}"/> requires of every implementor.
/// </para>
///
/// <para>
/// Postgres-only by design. The v2 server is Postgres (per the
/// upstream Docker compose + the project's deployment docs), so
/// there's no value in a multi-provider write path — the legacy
/// reader still reads SQLite / SQL Server / Postgres so operators
/// migrating off the upstream Docker default still have a path in.
/// </para>
/// </summary>
internal static class PostgresWriterRuntime
{
    /// <summary>
    /// Opens a fresh <see cref="NpgsqlConnection"/> against
    /// <paramref name="targetConnectionString"/>. Throws if the
    /// connection string isn't a Postgres-shaped one (the v2 schema
    /// is Postgres-only — we refuse to silently write SQL Server
    /// inserts to a Postgres-conn-string field).
    /// </summary>
    public static NpgsqlConnection ValidateAndCreate(string targetConnectionString)
    {
        if (string.IsNullOrWhiteSpace(targetConnectionString))
        {
            throw new ArgumentException(
                "Target connection string was null or whitespace.",
                nameof(targetConnectionString));
        }

        var provider = LegacyDbProviderDetector.Detect(targetConnectionString);
        if (provider != LegacyDbProvider.PostgreSql)
        {
            throw new NotSupportedException(
                $"The v2 target writer only supports PostgreSQL; got '{provider}'. " +
                "Provide a Postgres connection string (Host=…;Database=…;Username=…).");
        }

        return new NpgsqlConnection(targetConnectionString);
    }
}
