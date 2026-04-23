using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// Org-scoped Windows Installer (MSI) bundle uploaded by an operator and
/// available for silent install on managed devices via the Package
/// Manager. The MSI bytes themselves live in <see cref="SharedFile"/>
/// (re-used so the existing
/// <c>FileSharingController</c> + expiring-token download path applies);
/// this row carries the metadata, an authoritative SHA-256, and the
/// soft-delete (tombstone) flag so deletes can wait until in-flight jobs
/// referencing the MSI have drained.
///
/// <para>Wire-up rule: a <see cref="Package"/> with
/// <c>Provider = PackageProvider.UploadedMsi</c> stores this row's
/// <see cref="Id"/> (string form of the GUID) in
/// <c>Package.PackageIdentifier</c>. The agent never sees the
/// <c>SharedFileId</c> directly — the server hands the agent a
/// short-lived signed download URL on dispatch.</para>
/// </summary>
public class UploadedMsi
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    public string OrganizationID { get; set; } = string.Empty;

    public Organization? Organization { get; set; }

    /// <summary>
    /// FK into <see cref="SharedFile"/> — the actual MSI bytes. Re-using
    /// <c>SharedFiles</c> means we get free integrity (the table is
    /// already content-addressed by id) and we don't add a second
    /// large-blob table just for MSIs.
    /// </summary>
    [StringLength(64)]
    public string SharedFileId { get; set; } = string.Empty;

    public SharedFile? SharedFile { get; set; }

    /// <summary>
    /// Operator-friendly display name. Distinct from the on-disk
    /// filename so the same MSI can be re-uploaded (e.g. a newer
    /// version) without breaking existing <see cref="Package"/> rows
    /// that point at this id.
    /// </summary>
    [StringLength(120)]
    public string Name { get; set; } = string.Empty;

    /// <summary>
    /// Original filename as supplied by the browser. Sanitised before
    /// persistence — only the leaf name (no path) is stored, and only
    /// after passing the magic-byte check.
    /// </summary>
    [StringLength(255)]
    public string FileName { get; set; } = string.Empty;

    public long SizeBytes { get; set; }

    /// <summary>
    /// Lowercase hex SHA-256 of the uploaded bytes. The agent re-hashes
    /// the bytes it downloads and refuses to install on mismatch. Index
    /// to allow operator-driven dedupe.
    /// </summary>
    [StringLength(64)]
    public string Sha256 { get; set; } = string.Empty;

    [StringLength(1024)]
    public string? Description { get; set; }

    public DateTimeOffset UploadedAt { get; set; } = DateTimeOffset.UtcNow;

    public string? UploadedByUserId { get; set; }

    /// <summary>
    /// True once the operator has requested deletion. The row stays so
    /// in-flight <see cref="PackageInstallJob"/>s referencing this MSI
    /// can still resolve their bytes; the periodic cleanup hard-deletes
    /// it (along with the underlying <see cref="SharedFile"/>) once no
    /// non-terminal job references the matching <see cref="Package"/>.
    /// </summary>
    public bool IsTombstoned { get; set; }

    public DateTimeOffset? TombstonedAt { get; set; }
}
