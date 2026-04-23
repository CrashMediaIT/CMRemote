namespace Remotely.Migration.Legacy;

/// <summary>
/// Reads pages of legacy rows of one entity type
/// (<typeparamref name="TLegacy"/>) from a source database connection,
/// so the runner can stream them through the matching
/// <see cref="IRowConverter{TLegacy, TV2}"/> without holding the entire
/// table in memory.
///
/// <para>
/// One implementation per (<see cref="LegacySchemaVersion"/>, entity)
/// pair, mirroring the converter shape: when a future upstream
/// release reshapes a table, a new reader gets added rather than the
/// existing one mutated, so previously-shipped converter / reader
/// sets stay byte-stable.
/// </para>
///
/// <para>
/// Implementations must:
/// <list type="bullet">
///   <item>Stream — never load the whole table; honour
///         <see cref="MigrationOptions.BatchSize"/>.</item>
///   <item>Order rows deterministically (by primary key) so a resumed
///         run sees the same sequence and can skip already-imported
///         rows by id.</item>
///   <item>Open exactly one source connection per
///         <see cref="ReadAsync"/> call and close it before the
///         enumerator completes.</item>
/// </list>
/// </para>
/// </summary>
public interface ILegacyRowReader<TLegacy>
{
    /// <summary>
    /// Logical entity name surfaced in
    /// <see cref="EntityReport.EntityName"/> — must match the
    /// matching converter's <see cref="IRowConverter{TLegacy, TV2}.EntityName"/>
    /// so the runner can pair them up.
    /// </summary>
    string EntityName { get; }

    /// <summary>
    /// Schema version this reader handles (single-version, like the
    /// converter).
    /// </summary>
    LegacySchemaVersion HandlesSchemaVersion { get; }

    /// <summary>
    /// Streams every legacy row of this entity from the source.
    /// Implementations open their own ADO.NET connection (driver
    /// picked from the connection-string shape via
    /// <see cref="LegacyDbProviderDetector"/>) and yield rows one at
    /// a time — the runner is responsible for batching writes.
    /// </summary>
    IAsyncEnumerable<TLegacy> ReadAsync(
        string sourceConnectionString,
        int batchSize,
        CancellationToken cancellationToken = default);
}
