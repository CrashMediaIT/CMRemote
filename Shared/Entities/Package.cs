using Remotely.Shared.Enums;
using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// Org-scoped definition of a package the operator can deploy. The
/// concrete bytes / install command live with the agent provider; this
/// entity only carries the metadata the WebUI needs to list, search,
/// and dispatch a job.
///
/// <para>For <see cref="PackageProvider.Chocolatey"/>, <see cref="PackageIdentifier"/>
/// is the choco package id (e.g. <c>googlechrome</c>) and the version is
/// optional (omitted ⇒ latest).</para>
///
/// <para>For <see cref="PackageProvider.UploadedMsi"/> and
/// <see cref="PackageProvider.Executable"/>, this entity is wired up in
/// PR C1 — the schema accommodates them so we don't migrate twice.</para>
/// </summary>
public class Package
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    public string OrganizationID { get; set; } = string.Empty;

    public Organization? Organization { get; set; }

    [StringLength(120)]
    public string Name { get; set; } = string.Empty;

    public PackageProvider Provider { get; set; }

    /// <summary>
    /// Provider-specific identifier. For Chocolatey this is the package
    /// id (no spaces, lowercase by convention). For UploadedMsi this is
    /// the <c>UploadedMsi.Id</c> as a string. For Executable this is the
    /// <c>SharedFile.ID</c> the agent should download.
    /// </summary>
    [StringLength(256)]
    public string PackageIdentifier { get; set; } = string.Empty;

    /// <summary>
    /// Optional version pin. Empty ⇒ latest. Free-form by provider.
    /// </summary>
    [StringLength(64)]
    public string? Version { get; set; }

    /// <summary>
    /// Operator-supplied install arguments appended after the
    /// provider's silent-install defaults. Validated server-side to
    /// reject shell metacharacters before dispatch.
    /// </summary>
    [StringLength(1024)]
    public string? InstallArguments { get; set; }

    [StringLength(1024)]
    public string? Description { get; set; }

    public DateTimeOffset CreatedAt { get; set; } = DateTimeOffset.UtcNow;

    public string? CreatedByUserId { get; set; }
}
