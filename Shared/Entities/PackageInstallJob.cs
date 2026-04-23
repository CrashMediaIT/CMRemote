using Remotely.Shared.Enums;
using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// One package install/uninstall request targeted at a single device.
/// A bundle dispatch fans out into N jobs (one per item × one per
/// device); a single-package dispatch creates one job per device.
///
/// <para>Lifecycle is governed by <see cref="PackageInstallJobStatus"/>
/// and managed exclusively by <c>IPackageInstallJobService</c> — no
/// other code should mutate <see cref="Status"/> or the timestamps
/// directly. Terminal states are immutable.</para>
/// </summary>
public class PackageInstallJob
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    public string OrganizationID { get; set; } = string.Empty;

    public Organization? Organization { get; set; }

    public Guid PackageId { get; set; }

    public Package? Package { get; set; }

    /// <summary>
    /// Optional bundle this job was created from. Null for ad-hoc
    /// single-package dispatches.
    /// </summary>
    public Guid? DeploymentBundleId { get; set; }

    public DeploymentBundle? DeploymentBundle { get; set; }

    /// <summary>
    /// Target device. Snapshotted as a string so a job survives a
    /// device deletion and remains historically meaningful.
    /// </summary>
    [StringLength(128)]
    public string DeviceId { get; set; } = string.Empty;

    public PackageInstallAction Action { get; set; }

    public PackageInstallJobStatus Status { get; set; } = PackageInstallJobStatus.Queued;

    public DateTimeOffset CreatedAt { get; set; } = DateTimeOffset.UtcNow;

    public DateTimeOffset? StartedAt { get; set; }

    public DateTimeOffset? CompletedAt { get; set; }

    [StringLength(64)]
    public string? RequestedByUserId { get; set; }

    public PackageInstallResult? Result { get; set; }
}
