namespace Remotely.Server.Services.Setup;

/// <summary>
/// The five steps of the first-boot setup wizard, in order. Persisted
/// via <see cref="ISetupWizardProgressService"/> so that a browser
/// reload — or an operator coming back to a half-finished install
/// from a different machine — resumes at the correct step rather than
/// landing on the welcome page again.
///
/// See <c>ROADMAP.md</c> &gt; "M1 — First-boot setup wizard" for the
/// step contract.
/// </summary>
public enum SetupWizardStep
{
    /// <summary>The operator has not started the wizard yet.</summary>
    Welcome = 0,

    /// <summary>M1.1 — Welcome / preflight checks.</summary>
    Preflight = 1,

    /// <summary>M1.2 — Database connection form + live <c>SELECT 1</c>.</summary>
    Database = 2,

    /// <summary>M1.3 — Optional legacy-database import.</summary>
    Import = 3,

    /// <summary>M1.4 — Admin bootstrap (placeholder; lands in a follow-up).</summary>
    AdminBootstrap = 4,

    /// <summary>M1.5 — Done; marker is written and the operator is signed in.</summary>
    Done = 5,
}
