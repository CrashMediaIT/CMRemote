namespace Remotely.Migration.Legacy;

/// <summary>
/// Identifies a known schema layout produced by an upstream Remotely
/// release (or an unrelated layout we do not migrate from).
///
/// Per <c>ROADMAP.md</c> "M2 — Schema converter library" the importer
/// is **versioned**: each upstream layout maps through a dedicated set
/// of <see cref="IRowConverter{TLegacy, TV2}"/> implementations rather
/// than a single best-effort copy. The enum is open-ended on purpose —
/// future upstream releases append a new variant rather than mutate an
/// existing one, so previously-shipped converter sets stay byte-stable.
/// </summary>
public enum LegacySchemaVersion
{
    /// <summary>
    /// Schema fingerprint did not match any known upstream layout. The
    /// migration runner refuses to import in this state — a converter
    /// pass-through against an unknown schema risks silent data loss.
    /// </summary>
    Unknown = 0,

    /// <summary>
    /// No schema was detected at the source connection at all (no
    /// recognised tables, or the source DB is empty). The runner
    /// reports zero rows migrated and exits successfully — there was
    /// nothing to import.
    /// </summary>
    Empty = 1,

    /// <summary>
    /// Upstream Remotely as shipped in the legacy Docker image at the
    /// time the M2 scaffold landed. Detected by the presence of the
    /// canonical EF Core <c>__EFMigrationsHistory</c> table together
    /// with the <c>Organizations</c>, <c>Devices</c>, and
    /// <c>AspNetUsers</c> tables. Concrete row converters for this
    /// version land in subsequent M2 slices; the scaffold ships only
    /// the detection enum entry and the reference
    /// <see cref="Converters.OrganizationRowConverter"/>.
    /// </summary>
    UpstreamLegacy_2026_04 = 2,
}
