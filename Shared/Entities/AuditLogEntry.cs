using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// A single, immutable entry on the per-organization audit log
/// (ROADMAP.md "Track S / S7 — Runtime security posture: an
/// **immutable audit log** ... was PR D was re-scoped as a Track S
/// item and lands with the install-job pipeline").
///
/// <para>Entries are tamper-evident: each row carries a SHA-256 hash
/// over <c>(prev_hash || canonical_serialized_body)</c>, where
/// <c>prev_hash</c> is the hash of the previous row in this org's
/// chain. Verification is one linear scan: any in-place edit, delete,
/// or reorder breaks the chain at the affected row and every row after
/// it. Operators see the broken row and the rows that followed it as
/// "tamper detected" in the M4 dashboard.</para>
///
/// <para>The chain is per-organization so a multi-tenant deployment's
/// audit log can be sharded / archived / GDPR-deleted per org without
/// breaking the chain for other orgs. Within an org the chain is
/// strictly serialized by <see cref="Sequence"/> — the
/// <see cref="IAuditLogService"/> uses a per-org write lock so two
/// concurrent appends never get the same sequence number.</para>
///
/// <para>Append-only: the entity has no setter on <see cref="EntryHash"/>
/// outside of construction time and the EF mapping marks the row
/// <c>UpdateBehavior = Restrict</c> so an accidental
/// <c>SaveChanges</c> after a property mutation throws.</para>
/// </summary>
public class AuditLogEntry
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    /// <summary>
    /// Org snapshot. The chain is per-org and verification scans rows
    /// for one org at a time.
    /// </summary>
    [StringLength(128)]
    public string OrganizationID { get; set; } = string.Empty;

    /// <summary>
    /// Monotonic per-org sequence number. Assigned by
    /// <see cref="IAuditLogService.AppendAsync"/> under a per-org lock
    /// so the sequence is dense and conflict-free.
    /// </summary>
    public long Sequence { get; set; }

    public DateTimeOffset OccurredAt { get; set; } = DateTimeOffset.UtcNow;

    /// <summary>
    /// Short stable identifier for the kind of event (e.g.
    /// <c>"package.install.dispatch"</c>, <c>"agent.upgrade.dispatch"</c>,
    /// <c>"auth.login.success"</c>, <c>"auth.login.failure"</c>). Used
    /// by the M4 dashboard to filter and aggregate; kept as a string
    /// rather than an enum so a new event type doesn't require a
    /// schema migration.
    /// </summary>
    [StringLength(64)]
    public string EventType { get; set; } = string.Empty;

    /// <summary>
    /// Actor responsible for the event. The user id when the event is
    /// driven by an authenticated operator; the device id when the
    /// event is driven by an agent; <c>"system"</c> when the event is
    /// driven by a background service. Not a foreign key so a deleted
    /// user / device does not break the chain.
    /// </summary>
    [StringLength(128)]
    public string ActorId { get; set; } = string.Empty;

    /// <summary>
    /// Subject of the event (e.g. the device id a job dispatched
    /// against, the package id installed, the org id created). Not a
    /// foreign key for the same reason as <see cref="ActorId"/>.
    /// </summary>
    [StringLength(256)]
    public string SubjectId { get; set; } = string.Empty;

    /// <summary>
    /// Human-readable summary line. The single most-quoted field on
    /// the M4 dashboard. Capped on write to avoid runaway rows.
    /// </summary>
    [StringLength(1024)]
    public string Summary { get; set; } = string.Empty;

    /// <summary>
    /// Optional JSON blob with structured event-specific detail. The
    /// <see cref="IAuditLogService"/> serialises this canonically (sorted
    /// keys, no whitespace) before hashing so the chain is deterministic
    /// across server restarts.
    /// </summary>
    [StringLength(8192)]
    public string? DetailJson { get; set; }

    /// <summary>
    /// Lower-case hex SHA-256 of the previous entry's
    /// <see cref="EntryHash"/>, or 64 zeros for the first entry in the
    /// chain. Stored explicitly so verification is a single scan; the
    /// invariant <c>this.PrevHash == prev.EntryHash</c> is the chain
    /// link.
    /// </summary>
    [StringLength(64)]
    public string PrevHash { get; set; } = new string('0', 64);

    /// <summary>
    /// Lower-case hex SHA-256 over the canonical serialization of this
    /// row's body (<see cref="OrganizationID"/>, <see cref="Sequence"/>,
    /// <see cref="OccurredAt"/>, <see cref="EventType"/>,
    /// <see cref="ActorId"/>, <see cref="SubjectId"/>,
    /// <see cref="Summary"/>, <see cref="DetailJson"/>) prefixed by
    /// <see cref="PrevHash"/>. Computed by
    /// <see cref="IAuditLogService.AppendAsync"/> at append time and
    /// re-computed on verification.
    /// </summary>
    [StringLength(64)]
    public string EntryHash { get; set; } = new string('0', 64);
}
