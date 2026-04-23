namespace Remotely.Server.Services;

/// <summary>
/// Tracks whether the first-boot setup wizard (Band 1 / PR M scaffolding —
/// see ROADMAP.md "M1 — First-boot setup wizard") has been completed.
///
/// The marker is persisted as a row in the <c>KeyValueRecords</c> table
/// keyed by a fixed Guid (<see cref="SetupStateService.SetupCompletedKey"/>)
/// so the wizard's state survives restarts and is replicated wherever the
/// AppDb is.
///
/// This is the *skeleton* slice: it provides the marker plumbing that the
/// later M1 sub-slices (preflight, DB connection, legacy import, admin
/// bootstrap) and PR M (the agent-upgrade pipeline) hang off. The actual
/// wizard steps land in subsequent PRs.
/// </summary>
public interface ISetupStateService
{
    /// <summary>
    /// Returns <c>true</c> when the first-boot setup wizard has been
    /// marked complete, either explicitly by an operator or implicitly by
    /// <see cref="EnsureMarkerForExistingDeploymentAsync"/> at startup
    /// against an already-populated database.
    /// </summary>
    Task<bool> IsSetupCompletedAsync(CancellationToken cancellationToken = default);

    /// <summary>
    /// Writes the <c>CMRemote.Setup.Completed</c> marker. Idempotent: a
    /// second call against an already-marked database is a no-op and does
    /// not overwrite the original completion timestamp.
    /// </summary>
    Task MarkSetupCompletedAsync(CancellationToken cancellationToken = default);

    /// <summary>
    /// Called once at startup. If the marker is missing **and** the
    /// database already contains operator-visible state (any
    /// <c>Organization</c>, <c>RemotelyUser</c>, or <c>Device</c>) then
    /// the marker is written so existing deployments are never hijacked
    /// into the setup wizard when they upgrade onto a build that ships
    /// the wizard skeleton.
    ///
    /// Greenfield databases (no orgs / users / devices) leave the marker
    /// unset so the redirect middleware sends the operator to
    /// <c>/setup</c>.
    /// </summary>
    Task EnsureMarkerForExistingDeploymentAsync(CancellationToken cancellationToken = default);
}
