namespace Remotely.Migration.Legacy;

/// <summary>
/// Writes converted v2 rows of one entity type
/// (<typeparamref name="TV2"/>) to the target database, so the runner
/// can persist what each <see cref="IRowConverter{TLegacy, TV2}"/>
/// produced.
///
/// <para>
/// One implementation per (<see cref="LegacySchemaVersion"/>, entity)
/// pair, mirroring the converter / reader shape: a future upstream
/// version of an entity gets a new writer rather than mutating an
/// existing one, so previously-shipped writer / converter / reader
/// sets stay byte-stable.
/// </para>
///
/// <para>
/// Implementations must:
/// <list type="bullet">
///   <item>Be **idempotent by primary key** — a resumed run that
///         re-feeds the same row must upsert (not duplicate). The
///         identity-preservation rule from ROADMAP M1.3 means the
///         row's primary key is byte-stable across reruns, so an
///         upsert keyed off it is sufficient.</item>
///   <item>Open exactly one target connection per
///         <see cref="WriteAsync"/> call, or hold one for the
///         lifetime of the writer instance — the runner does not
///         manage connection scope.</item>
///   <item>Throw on persistent failure — the runner catches the
///         exception, records it against the row, and continues with
///         the next row. Writers must not swallow errors silently.</item>
/// </list>
/// </para>
/// </summary>
public interface ILegacyRowWriter<TV2>
{
    /// <summary>
    /// Logical entity name surfaced in
    /// <see cref="EntityReport.EntityName"/> — must match the
    /// matching converter's
    /// <see cref="IRowConverter{TLegacy, TV2}.EntityName"/> so the
    /// runner can pair them up.
    /// </summary>
    string EntityName { get; }

    /// <summary>
    /// Schema version this writer handles (single-version, like the
    /// converter and reader).
    /// </summary>
    LegacySchemaVersion HandlesSchemaVersion { get; }

    /// <summary>
    /// Writes a single converted v2 row to the target. Must be
    /// idempotent by primary key (upsert semantics).
    /// </summary>
    Task WriteAsync(
        TV2 row,
        string targetConnectionString,
        CancellationToken cancellationToken = default);
}
