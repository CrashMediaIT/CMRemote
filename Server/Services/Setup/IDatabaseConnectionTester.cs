namespace Remotely.Server.Services.Setup;

/// <summary>
/// Result of a single attempt to connect to a Postgres server with
/// the operator-supplied connection string. Three outcomes:
///
/// <list type="bullet">
/// <item><description><see cref="ConnectionTestOutcome.Success"/> — the round trip
///   completed.</description></item>
/// <item><description><see cref="ConnectionTestOutcome.InvalidConnectionString"/> — the
///   string could not even be parsed; the operator typo'd a key /
///   value.</description></item>
/// <item><description><see cref="ConnectionTestOutcome.NetworkOrAuthFailure"/> — the
///   string parsed but Postgres rejected the connection (DNS, TCP,
///   TLS, password, db-not-found).</description></item>
/// </list>
/// </summary>
public enum ConnectionTestOutcome
{
    Success = 0,
    InvalidConnectionString = 1,
    NetworkOrAuthFailure = 2,
}

/// <summary>
/// Outcome of <see cref="IDatabaseConnectionTester.TestPostgresAsync"/>.
/// </summary>
/// <param name="Outcome">Three-valued status; see <see cref="ConnectionTestOutcome"/>.</param>
/// <param name="Message">
/// Operator-visible message. Always set; on success this is a
/// confirmation, on failure this is the underlying exception's
/// message (with the password redacted).
/// </param>
public sealed record ConnectionTestResult(
    ConnectionTestOutcome Outcome,
    string Message)
{
    public bool IsSuccess => Outcome == ConnectionTestOutcome.Success;
}

/// <summary>
/// Performs a single live <c>SELECT 1</c> round trip against an
/// operator-supplied Postgres connection string. Used by the M1.2
/// wizard step's "Test connection" button.
///
/// The interface accepts only Postgres because the v2 schema is
/// Postgres-only — the same constraint <c>PostgresWriterRuntime</c>
/// enforces on the migration writers.
/// </summary>
public interface IDatabaseConnectionTester
{
    Task<ConnectionTestResult> TestPostgresAsync(
        string connectionString,
        CancellationToken cancellationToken = default);
}
