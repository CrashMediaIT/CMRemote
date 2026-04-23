using System.Text.Json;
using System.Text.Json.Serialization;

namespace Remotely.Migration.Legacy;

/// <summary>
/// Per-execution audit trail produced by <see cref="IMigrationRunner"/>.
///
/// Serialised to <c>migration-report.json</c> per `ROADMAP.md` "M1 step
/// 3" so the wizard's progress page and the headless CLI agree on the
/// same artefact format. The shape is intentionally schema-stable: the
/// import step is **resumable** per the roadmap, which means a later
/// run must be able to read a report written by an earlier run.
/// </summary>
public class MigrationReport
{
    /// <summary>UTC timestamp the run started.</summary>
    public DateTimeOffset StartedAtUtc { get; set; }

    /// <summary>UTC timestamp the run finished, or <c>null</c> while in flight.</summary>
    public DateTimeOffset? CompletedAtUtc { get; set; }

    /// <summary>
    /// Result of source-schema detection (the runner refuses to write
    /// when this is <see cref="LegacySchemaVersion.Unknown"/>).
    /// </summary>
    public LegacySchemaVersion DetectedSchemaVersion { get; set; }

    /// <summary>True if the run was a dry-run (no rows written to target).</summary>
    public bool DryRun { get; set; }

    /// <summary>
    /// One entry per <see cref="IRowConverter{TLegacy, TV2}"/> the
    /// runner enumerated. Entries are appended in execution order so
    /// the wizard can render a live progress list without re-sorting.
    /// </summary>
    public List<EntityReport> Entities { get; set; } = new();

    /// <summary>
    /// Hard-error messages that aborted the run (empty on success).
    /// Per-row conversion errors are recorded under the relevant
    /// <see cref="EntityReport.Errors"/> instead so a single bad row
    /// does not block the rest of the import.
    /// </summary>
    public List<string> FatalErrors { get; set; } = new();

    /// <summary>
    /// Stable schema version of the report document itself. Bumped
    /// whenever a non-additive change lands in this class so resuming
    /// against a report written by an older build is a hard error
    /// instead of a silent miscount.
    /// </summary>
    public int ReportSchemaVersion { get; set; } = 1;

    public string ToJson() => JsonSerializer.Serialize(this, ReportJsonOptions);

    public static MigrationReport FromJson(string json) =>
        JsonSerializer.Deserialize<MigrationReport>(json, ReportJsonOptions)
            ?? throw new InvalidOperationException("Migration report JSON deserialised to null.");

    internal static readonly JsonSerializerOptions ReportJsonOptions = new()
    {
        WriteIndented = true,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
        Converters = { new JsonStringEnumConverter() },
    };
}

/// <summary>
/// Per-entity-type slice of <see cref="MigrationReport"/>.
/// </summary>
public class EntityReport
{
    /// <summary>Logical entity name, e.g. <c>Organization</c>, <c>Device</c>.</summary>
    public required string EntityName { get; set; }

    /// <summary>Total rows the runner read from the source for this entity.</summary>
    public int RowsRead { get; set; }

    /// <summary>Rows the converter mapped successfully into a v2 row.</summary>
    public int RowsConverted { get; set; }

    /// <summary>
    /// Rows that were actually persisted to the target by the matching
    /// <see cref="ILegacyRowWriter{TV2}"/>. Always &lt;=
    /// <see cref="RowsConverted"/>; equals zero on a dry-run or when no
    /// writer is registered for the entity yet.
    /// </summary>
    public int RowsWritten { get; set; }

    /// <summary>
    /// Rows the converter rejected with an
    /// <see cref="ConverterResult{T}.IsSkipped"/> verdict (e.g.
    /// orphaned FKs, soft-deleted rows). Counted but not written.
    /// </summary>
    public int RowsSkipped { get; set; }

    /// <summary>Rows that crashed the converter or the writer.</summary>
    public int RowsFailed { get; set; }

    /// <summary>Per-row error messages (capped — see <see cref="MaxErrorsPerEntity"/>).</summary>
    public List<string> Errors { get; set; } = new();

    /// <summary>Cap on per-entity error retention to keep the report bounded.</summary>
    public const int MaxErrorsPerEntity = 100;
}
