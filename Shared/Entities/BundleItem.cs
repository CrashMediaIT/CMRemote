using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// One <see cref="Package"/> reference inside a <see cref="DeploymentBundle"/>.
/// <see cref="Order"/> controls the agent-side install sequence within a
/// fan-out so dependencies (runtime → application) install in the
/// expected order.
/// </summary>
public class BundleItem
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    public Guid DeploymentBundleId { get; set; }

    public DeploymentBundle? DeploymentBundle { get; set; }

    public Guid PackageId { get; set; }

    public Package? Package { get; set; }

    public int Order { get; set; }

    /// <summary>
    /// When true, a failure of this item aborts the rest of the bundle
    /// for that device. Default true — bundles model dependent installs.
    /// </summary>
    public bool ContinueOnFailure { get; set; } = false;
}
