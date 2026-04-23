namespace Remotely.Migration.Legacy;

/// <summary>
/// Identifies which ADO.NET driver the migration importer should use
/// to open a given source connection string.
///
/// The legacy upstream Docker image supports SQLite (the default
/// out-of-the-box configuration), SQL Server, and PostgreSQL, so the
/// importer has to handle all three on the read side. The v2 target
/// is Postgres-only per <c>ROADMAP.md</c> "M1 — First-boot setup
/// wizard" step 2.
/// </summary>
public enum LegacyDbProvider
{
    /// <summary>
    /// Connection-string shape was not recognised. The inspector
    /// throws a <see cref="System.NotSupportedException"/> so the
    /// runner records it as a fatal error rather than guessing.
    /// </summary>
    Unknown = 0,

    /// <summary>
    /// SQLite file or in-memory database. Detected by a connection
    /// string that contains a <c>Data Source=</c> token but none of
    /// the SQL-Server-specific tokens (<c>Initial Catalog</c>,
    /// <c>Server=</c>) and is not a PostgreSQL string (no
    /// <c>Host=</c>).
    /// </summary>
    Sqlite = 1,

    /// <summary>
    /// Microsoft SQL Server. Detected by the presence of
    /// <c>Server=</c> or <c>Initial Catalog=</c> in the connection
    /// string.
    /// </summary>
    SqlServer = 2,

    /// <summary>
    /// PostgreSQL. Detected by the presence of <c>Host=</c> in the
    /// connection string (Npgsql's canonical key).
    /// </summary>
    PostgreSql = 3,
}
