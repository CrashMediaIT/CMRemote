namespace Remotely.Server.Services.Setup;

/// <summary>
/// Outcome of a single preflight check (M1.1). Three-valued so the
/// wizard can distinguish "blocking failure" (the operator cannot
/// continue) from "advisory" (best-effort, the operator may still
/// proceed but should be aware).
/// </summary>
public enum PreflightStatus
{
    /// <summary>The check succeeded.</summary>
    Passed = 0,

    /// <summary>The check did not pass but does not block continuing.</summary>
    Warning = 1,

    /// <summary>The check failed and the operator should resolve it before continuing.</summary>
    Failed = 2,
}

/// <summary>
/// Result of a single preflight check (writable data dir, TLS
/// configured, bind ports). The wizard renders one row per check.
/// </summary>
/// <param name="Name">Operator-visible short name of the check.</param>
/// <param name="Status">Three-valued status; see <see cref="PreflightStatus"/>.</param>
/// <param name="Detail">
/// Operator-visible detail. Always set; on a passed check this is the
/// resolved value (e.g. the absolute data-dir path), on a warning or
/// failure this is the remediation hint.
/// </param>
public sealed record PreflightCheckResult(
    string Name,
    PreflightStatus Status,
    string Detail);

/// <summary>
/// Aggregate result of running every preflight check. Exposed as a
/// shape distinct from <see cref="IReadOnlyList{T}"/> so the page can
/// expose convenience helpers (<see cref="CanContinue"/>) without
/// reopening the iteration.
/// </summary>
public sealed class PreflightReport
{
    public required IReadOnlyList<PreflightCheckResult> Checks { get; init; }

    /// <summary>
    /// True iff no check returned <see cref="PreflightStatus.Failed"/>.
    /// Warnings do not block continuing — the wizard is opinionated
    /// about preflight being advisory rather than gating.
    /// </summary>
    public bool CanContinue =>
        Checks.All(c => c.Status != PreflightStatus.Failed);
}

/// <summary>
/// Runs the M1.1 preflight checks for the first-boot setup wizard.
///
/// The current set of checks is:
///
/// <list type="bullet">
/// <item><description>Writable data directory — that the directory we plan to put
///   <c>appsettings.Production.json</c> in is writable by the server process.</description></item>
/// <item><description>TLS configuration — that the configured ASP.NET Core endpoints
///   include at least one HTTPS binding (advisory; HTTP is allowed
///   in dev / behind a reverse proxy).</description></item>
/// <item><description>Bind ports — that the bound endpoints are reachable from the
///   process (a sanity check that the server itself is up).</description></item>
/// </list>
///
/// Checks are best-effort: they never throw, instead surfacing
/// failures as <see cref="PreflightStatus.Failed"/> /
/// <see cref="PreflightStatus.Warning"/> entries so the wizard can
/// render them inline.
/// </summary>
public interface IPreflightService
{
    Task<PreflightReport> RunChecksAsync(CancellationToken cancellationToken = default);
}
