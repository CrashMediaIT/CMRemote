using Npgsql;

namespace Remotely.Server.Services.Setup;

/// <inheritdoc cref="IDatabaseConnectionTester" />
public class PostgresConnectionTester : IDatabaseConnectionTester
{
    private readonly ILogger<PostgresConnectionTester> _logger;

    public PostgresConnectionTester(ILogger<PostgresConnectionTester> logger)
    {
        _logger = logger;
    }

    /// <inheritdoc />
    public async Task<ConnectionTestResult> TestPostgresAsync(
        string connectionString,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(connectionString))
        {
            return new ConnectionTestResult(
                ConnectionTestOutcome.InvalidConnectionString,
                "Connection string is empty.");
        }

        // Parse separately from opening so we can return a
        // distinct InvalidConnectionString outcome — that lets the
        // wizard surface "you typo'd the form" differently from
        // "the server is unreachable".
        NpgsqlConnectionStringBuilder builder;
        try
        {
            builder = new NpgsqlConnectionStringBuilder(connectionString);
        }
        catch (ArgumentException ex)
        {
            return new ConnectionTestResult(
                ConnectionTestOutcome.InvalidConnectionString,
                $"Connection string is malformed: {ex.Message}");
        }

        // A connection string with no host literally cannot resolve;
        // surface that as an invalid string rather than a network
        // failure so the wizard's hint copy is correct.
        if (string.IsNullOrWhiteSpace(builder.Host))
        {
            return new ConnectionTestResult(
                ConnectionTestOutcome.InvalidConnectionString,
                "Connection string is missing a Host= value.");
        }

        try
        {
            await using var conn = new NpgsqlConnection(builder.ConnectionString);
            await conn.OpenAsync(cancellationToken).ConfigureAwait(false);

            await using var cmd = conn.CreateCommand();
            cmd.CommandText = "SELECT 1";
            var result = await cmd.ExecuteScalarAsync(cancellationToken).ConfigureAwait(false);

            if (result is null || Convert.ToInt32(result) != 1)
            {
                return new ConnectionTestResult(
                    ConnectionTestOutcome.NetworkOrAuthFailure,
                    "SELECT 1 did not return 1.");
            }

            var serverVersion = SafeServerVersion(conn);
            return new ConnectionTestResult(
                ConnectionTestOutcome.Success,
                $"Connected to '{builder.Database}' on '{builder.Host}'" +
                $"{(serverVersion is null ? string.Empty : $" (Postgres {serverVersion})")}.");
        }
        catch (OperationCanceledException)
        {
            // Honour explicit cancellation rather than swallowing it
            // into a generic network failure — the wizard cancels on
            // navigation away.
            throw;
        }
        catch (Exception ex)
        {
            _logger.LogInformation(ex,
                "Postgres connection test against {Host}/{Database} failed.",
                builder.Host, builder.Database);
            return new ConnectionTestResult(
                ConnectionTestOutcome.NetworkOrAuthFailure,
                Redact(ex.Message, builder.Password));
        }
    }

    private static string? SafeServerVersion(NpgsqlConnection conn)
    {
        try
        {
            return conn.PostgreSqlVersion?.ToString();
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// Defensive: drivers sometimes echo the raw connection string
    /// (and therefore the password) into exception messages. Redact
    /// the password before surfacing it to the wizard.
    /// </summary>
    private static string Redact(string message, string? password)
    {
        if (string.IsNullOrEmpty(message))
        {
            return message;
        }
        if (string.IsNullOrEmpty(password))
        {
            return message;
        }
        return message.Replace(password, "***", StringComparison.Ordinal);
    }
}
