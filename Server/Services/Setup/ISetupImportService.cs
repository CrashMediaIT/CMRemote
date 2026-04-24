extern alias MigrationLegacy;

using MigrationLegacy::Remotely.Migration.Legacy;

namespace Remotely.Server.Services.Setup;

/// <summary>
/// Wizard-facing snapshot of <see cref="MigrationReport"/>. Defined
/// inside the Server's Setup namespace so the M1.3 Razor page can
/// render it without needing <c>extern alias</c> support (which
/// Razor does not provide). One-to-one field mapping with
/// <see cref="MigrationReport"/> — every value on this type is
/// copied verbatim from the underlying report and the original
/// report is also persisted to <c>migration-report.json</c> so
/// nothing is lost.
/// </summary>
public sealed class WizardImportReport
{
    public required string DetectedSchemaVersion { get; init; }
    public required bool DryRun { get; init; }
    public required DateTimeOffset StartedAtUtc { get; init; }
    public required DateTimeOffset? CompletedAtUtc { get; init; }
    public required IReadOnlyList<WizardImportEntityReport> Entities { get; init; }
    public required IReadOnlyList<string> FatalErrors { get; init; }

    public bool HadFatalErrors => FatalErrors.Count > 0;

    public int TotalRowsRead => Entities.Sum(e => e.RowsRead);
    public int TotalRowsConverted => Entities.Sum(e => e.RowsConverted);
    public int TotalRowsWritten => Entities.Sum(e => e.RowsWritten);
    public int TotalRowsSkipped => Entities.Sum(e => e.RowsSkipped);
    public int TotalRowsFailed => Entities.Sum(e => e.RowsFailed);

    internal static WizardImportReport From(MigrationReport report) => new()
    {
        DetectedSchemaVersion = report.DetectedSchemaVersion.ToString(),
        DryRun = report.DryRun,
        StartedAtUtc = report.StartedAtUtc,
        CompletedAtUtc = report.CompletedAtUtc,
        Entities = report.Entities
            .Select(WizardImportEntityReport.From)
            .ToList(),
        FatalErrors = report.FatalErrors.ToList(),
    };
}

/// <summary>Per-entity slice of <see cref="WizardImportReport"/>.</summary>
public sealed class WizardImportEntityReport
{
    public required string EntityName { get; init; }
    public required int RowsRead { get; init; }
    public required int RowsConverted { get; init; }
    public required int RowsWritten { get; init; }
    public required int RowsSkipped { get; init; }
    public required int RowsFailed { get; init; }
    public required IReadOnlyList<string> Errors { get; init; }

    internal static WizardImportEntityReport From(EntityReport entity) => new()
    {
        EntityName = entity.EntityName,
        RowsRead = entity.RowsRead,
        RowsConverted = entity.RowsConverted,
        RowsWritten = entity.RowsWritten,
        RowsSkipped = entity.RowsSkipped,
        RowsFailed = entity.RowsFailed,
        Errors = entity.Errors.ToList(),
    };
}

/// <summary>
/// Wires the M2 <see cref="MigrationRunner"/> into the M1.3 wizard
/// step so an operator can import a legacy-image database directly
/// from the browser instead of dropping to the headless
/// <c>cmremote-migrate</c> CLI.
///
/// Composed from the same converter / reader / writer triple set the
/// CLI uses (see <c>Migration.Cli/Program.cs</c>); the wizard and CLI
/// share one codepath end-to-end so behaviour cannot drift between
/// "import via UI" and "import via shell".
///
/// In addition to running the import this service also persists the
/// <see cref="MigrationReport"/> as <c>migration-report.json</c>
/// alongside the wizard-written <c>appsettings.Production.json</c>,
/// matching the M1.3 ROADMAP entry's "written
/// <c>migration-report.json</c> artefact" requirement.
/// </summary>
public interface ISetupImportService
{
    /// <summary>
    /// Detects the source schema only — does not stream any rows.
    /// Used by the wizard's "Detect" affordance to surface to the
    /// operator whether the source connection points at an
    /// importable upstream-legacy database before they commit to a
    /// real run.
    /// </summary>
    /// <returns>
    /// A <see cref="MigrationReport"/> populated with a single
    /// detection round. <see cref="MigrationReport.Entities"/> is
    /// empty; <see cref="MigrationReport.DetectedSchemaVersion"/>
    /// is the inspector's verdict.
    /// </returns>
    Task<WizardImportReport> DetectSourceAsync(
        string sourceConnectionString,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Runs a full migration. When <paramref name="dryRun"/> is
    /// <c>true</c> the inspector and converters fire but no target
    /// row is written; when <c>false</c> the Postgres writers persist
    /// rows via <c>INSERT … ON CONFLICT DO UPDATE</c> (idempotent).
    ///
    /// On every call (including dry runs) the resulting report is
    /// also persisted to disk as <c>migration-report.json</c> next to
    /// the wizard's settings file so an operator post-mortem after
    /// the wizard closes is straightforward.
    /// </summary>
    Task<WizardImportReport> RunImportAsync(
        string sourceConnectionString,
        string targetConnectionString,
        bool dryRun,
        CancellationToken cancellationToken = default);
}
