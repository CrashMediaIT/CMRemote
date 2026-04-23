using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// Org-scoped grouping of <see cref="Package"/>s. Dispatching a bundle
/// queues one <see cref="PackageInstallJob"/> per <see cref="BundleItem"/>
/// per target device.
///
/// <para>Renamed from "Package Group" per the PR B spec — a "bundle"
/// captures the intended one-click multi-package deploy semantics
/// without colliding with the existing <c>DeviceGroup</c> concept.</para>
/// </summary>
public class DeploymentBundle
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    public string OrganizationID { get; set; } = string.Empty;

    public Organization? Organization { get; set; }

    [StringLength(120)]
    public string Name { get; set; } = string.Empty;

    [StringLength(1024)]
    public string? Description { get; set; }

    public ICollection<BundleItem> Items { get; set; } = new List<BundleItem>();

    public DateTimeOffset CreatedAt { get; set; } = DateTimeOffset.UtcNow;

    public string? CreatedByUserId { get; set; }
}
