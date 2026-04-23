namespace Remotely.Migration.Legacy;

/// <summary>
/// Top-level orchestrator: inspects the source schema, enumerates the
/// matching converters, runs every source row through them, and emits
/// a <see cref="MigrationReport"/>. Implementations are stateless
/// across runs so the same instance can serve both the wizard's
/// import step (M1.3) and the headless CLI (`cmremote migrate`).
/// </summary>
public interface IMigrationRunner
{
    /// <summary>
    /// Executes one migration pass against <paramref name="options"/>.
    /// Always returns a report (even for fatal-error runs — the
    /// fatal message is recorded under
    /// <see cref="MigrationReport.FatalErrors"/>) so the caller can
    /// surface a written <c>migration-report.json</c> regardless of
    /// outcome.
    /// </summary>
    Task<MigrationReport> RunAsync(
        MigrationOptions options,
        CancellationToken cancellationToken = default);
}
