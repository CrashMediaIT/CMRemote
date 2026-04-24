namespace Remotely.Server.Services.Setup;

/// <summary>
/// Writes the operator-supplied Postgres connection string into
/// <c>appsettings.Production.json</c> and triggers
/// <see cref="IConfigurationRoot.Reload" /> so subsequent requests
/// pick the new value up without a process restart.
///
/// On Unix the file is created with mode <c>0600</c> (owner-only
/// read/write), matching the on-disk-secret hygiene rule from the
/// wire-protocol spec's <em>Security model</em> and Track S / S6.
///
/// The writer is idempotent: writing the same string twice produces
/// the same on-disk bytes (modulo trailing newline). It preserves any
/// other keys that already exist in the file (e.g. logging config the
/// operator dropped in by hand).
/// </summary>
public interface IConnectionStringWriter
{
    /// <summary>
    /// Absolute path of the file the writer will modify. Exposed so
    /// the preflight service can verify it is in a writable
    /// directory and the wizard can show it to the operator.
    /// </summary>
    string TargetSettingsPath { get; }

    /// <summary>
    /// Persists <paramref name="postgresConnectionString" /> as
    /// <c>ConnectionStrings:PostgreSQL</c> and
    /// <c>ApplicationOptions:DbProvider=PostgreSql</c>, fsync's the
    /// file, sets <c>0600</c> on Unix, and reloads configuration.
    /// </summary>
    /// <exception cref="ArgumentException">
    /// Thrown when <paramref name="postgresConnectionString"/> is
    /// null, empty, or whitespace.
    /// </exception>
    Task WritePostgresConnectionAsync(
        string postgresConnectionString,
        CancellationToken cancellationToken = default);
}
