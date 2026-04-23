namespace Remotely.Migration.Legacy;

/// <summary>
/// Detects the upstream schema layout (if any) at a given source
/// connection so the runner can pick the matching set of
/// <see cref="IRowConverter{TLegacy, TV2}"/> implementations.
///
/// The concrete reader (which actually opens the source connection
/// and probes for known tables) lands in the next M2 slice. This
/// scaffold ships only the contract so the runner can compose
/// against it.
/// </summary>
public interface ILegacySchemaInspector
{
    /// <summary>
    /// Inspects the database referenced by <paramref name="sourceConnectionString"/>
    /// and returns the matching <see cref="LegacySchemaVersion"/>.
    /// Returns <see cref="LegacySchemaVersion.Empty"/> for a connectable
    /// database that contains no recognised schema (no tables at all,
    /// or none of the canonical upstream-Remotely tables), or
    /// <see cref="LegacySchemaVersion.Unknown"/> if the database does
    /// contain tables but their layout neither matches a known
    /// upstream version nor looks empty enough to safely no-op.
    /// </summary>
    Task<LegacySchemaVersion> DetectAsync(
        string sourceConnectionString,
        CancellationToken cancellationToken = default);
}
