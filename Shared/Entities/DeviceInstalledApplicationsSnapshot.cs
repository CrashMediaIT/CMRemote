using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// Latest installed-applications inventory for a device, stored as a JSON
/// blob to keep the schema minimal across the three supported providers.
/// One row per device — upserted on each agent refresh.
/// </summary>
public class DeviceInstalledApplicationsSnapshot
{
    /// <summary>
    /// Foreign key to <see cref="Device.ID"/>. Also the primary key —
    /// snapshots are 1:1 with devices.
    /// </summary>
    [Key]
    public string DeviceId { get; set; } = string.Empty;

    public Device? Device { get; set; }

    public DateTimeOffset FetchedAt { get; set; }

    /// <summary>
    /// JSON-serialized <see cref="System.Collections.Generic.IReadOnlyList{T}"/>
    /// of <c>Remotely.Shared.Models.InstalledApplication</c>.
    /// </summary>
    public string ApplicationsJson { get; set; } = "[]";
}
