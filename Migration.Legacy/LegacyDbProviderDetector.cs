namespace Remotely.Migration.Legacy;

/// <summary>
/// Pure helper that picks the right ADO.NET driver for a given
/// source connection string, based on the connection-string shape
/// alone. Kept separate from <see cref="LegacySchemaInspector"/> so
/// the detection rule is unit-testable without standing up a real
/// database connection, and so per-entity readers in subsequent M2
/// slices can reuse it without reaching back into the inspector.
///
/// <para>
/// Detection order is deliberate: <see cref="LegacyDbProvider.PostgreSql"/>
/// first because <c>Host=</c> is unambiguous,
/// <see cref="LegacyDbProvider.SqlServer"/> second because
/// <c>Server=</c> and <c>Initial Catalog=</c> are SQL-Server-specific,
/// and <see cref="LegacyDbProvider.Sqlite"/> last because
/// <c>Data Source=</c> is the most generic token (SQL Server's
/// <c>SqlConnectionStringBuilder</c> also accepts it as an alias for
/// <c>Server=</c>).
/// </para>
/// </summary>
public static class LegacyDbProviderDetector
{
    /// <summary>
    /// Returns the provider implied by <paramref name="connectionString"/>'s
    /// shape, or throws <see cref="NotSupportedException"/> if no
    /// known shape matches. The runner converts the throw into a
    /// fatal-errors entry on the report, so the wizard / CLI surface
    /// it as an operator-visible message rather than a stack trace.
    /// </summary>
    public static LegacyDbProvider Detect(string connectionString)
    {
        if (string.IsNullOrWhiteSpace(connectionString))
        {
            throw new ArgumentException(
                "Connection string was null or whitespace.",
                nameof(connectionString));
        }

        if (ContainsKey(connectionString, "Host"))
        {
            return LegacyDbProvider.PostgreSql;
        }

        if (ContainsKey(connectionString, "Server")
            || ContainsKey(connectionString, "Initial Catalog"))
        {
            return LegacyDbProvider.SqlServer;
        }

        if (ContainsKey(connectionString, "Data Source")
            || ContainsKey(connectionString, "DataSource")
            || ContainsKey(connectionString, "Filename"))
        {
            return LegacyDbProvider.Sqlite;
        }

        throw new NotSupportedException(
            "Source connection string shape was not recognised. Expected " +
            "one of: PostgreSQL ('Host=...'), SQL Server ('Server=...' or " +
            "'Initial Catalog=...'), or SQLite ('Data Source=...').");
    }

    /// <summary>
    /// Case-insensitive token-presence check. Looks for
    /// <c>{key}=</c> as a whole token between either start-of-string /
    /// <c>;</c> on the left and any character on the right, so a
    /// substring like <c>HostName</c> doesn't false-match
    /// <c>Host</c>.
    /// </summary>
    private static bool ContainsKey(string connectionString, string key)
    {
        var span = connectionString.AsSpan();
        var keySpan = key.AsSpan();
        var index = 0;
        while (index < span.Length)
        {
            // Skip leading whitespace and ';' separators.
            while (index < span.Length
                && (span[index] == ';' || char.IsWhiteSpace(span[index])))
            {
                index++;
            }

            // Find the end of this segment.
            var segmentStart = index;
            while (index < span.Length && span[index] != ';')
            {
                index++;
            }

            var segment = span[segmentStart..index];
            var eq = segment.IndexOf('=');
            if (eq > 0)
            {
                var name = segment[..eq].Trim();
                if (name.Equals(keySpan, StringComparison.OrdinalIgnoreCase))
                {
                    return true;
                }
            }
        }
        return false;
    }
}
