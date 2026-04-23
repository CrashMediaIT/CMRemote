namespace Remotely.Server.Services.Setup;

/// <summary>
/// Tracks which step of the first-boot setup wizard the operator has
/// reached. Persisted as a JSON-encoded row in <c>KeyValueRecords</c>
/// (alongside the existing <c>CMRemote.Setup.Completed</c> marker) so
/// the wizard can be resumed across browser reloads / restarts.
///
/// The redirect middleware does not consult this service — it only
/// cares about the binary "is setup complete" marker exposed by
/// <see cref="ISetupStateService"/>. This service exists so the
/// wizard's UI can route the operator to the correct step on a fresh
/// page load.
/// </summary>
public interface ISetupWizardProgressService
{
    /// <summary>
    /// Returns the highest step the operator has completed. Returns
    /// <see cref="SetupWizardStep.Welcome"/> for a fresh install.
    /// </summary>
    Task<SetupWizardStep> GetCurrentStepAsync(CancellationToken cancellationToken = default);

    /// <summary>
    /// Persists <paramref name="step"/> as the operator's current
    /// position in the wizard. Idempotent: writing the same step twice
    /// is a no-op. Refuses to move backwards: if the persisted step
    /// is already further along than <paramref name="step"/>, the call
    /// is a no-op (the wizard never "uncompletes" a step).
    /// </summary>
    Task SetCurrentStepAsync(SetupWizardStep step, CancellationToken cancellationToken = default);
}
