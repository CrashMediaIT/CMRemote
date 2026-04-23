namespace Remotely.Migration.Legacy;

/// <summary>
/// Caller-supplied options for one execution of
/// <see cref="IMigrationRunner.RunAsync"/>.
///
/// The first M2 slice (this scaffold) does not yet open the source
/// connection — the legacy-DB reader lands in the next slice. The
/// option shape is fixed up front so the wizard's import step (M1.3)
/// and the headless CLI (`cmremote migrate`) can both bind against the
/// same surface from day one.
/// </summary>
public class MigrationOptions
{
    /// <summary>
    /// ADO.NET-style connection string for the **source** (legacy)
    /// database. Provider is detected by the runner from the prefix /
    /// shape of the string (file path → SQLite, "Server=" → SQL
    /// Server, "Host=" → PostgreSQL).
    /// </summary>
    public required string SourceConnectionString { get; init; }

    /// <summary>
    /// ADO.NET-style connection string for the **target** v2
    /// PostgreSQL database. Postgres-only on the target side per
    /// `ROADMAP.md` "M1 — First-boot setup wizard" step 2.
    /// </summary>
    public required string TargetConnectionString { get; init; }

    /// <summary>
    /// When <c>true</c> the runner walks the source schema, runs every
    /// converter against every row, and counts everything into the
    /// <see cref="MigrationReport"/> — but writes nothing to the target
    /// database. Used by the wizard's "preview import" affordance and
    /// by the CLI's <c>--dry-run</c> flag.
    /// </summary>
    public bool DryRun { get; init; }

    /// <summary>
    /// Maximum number of source rows held in memory per converter pass
    /// before the runner flushes to the target. Defaults to a value
    /// that keeps the wizard's progress page responsive on a 2 GiB
    /// container without OOMing on a 50k-device fleet.
    /// </summary>
    public int BatchSize { get; init; } = 500;
}
