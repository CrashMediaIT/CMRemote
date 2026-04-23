using System.ComponentModel.DataAnnotations;
using System.ComponentModel.DataAnnotations.Schema;

namespace Remotely.Shared.Entities;

public class Organization
{
    public ICollection<Alert> Alerts { get; set; } = [];

    public ICollection<ApiToken> ApiTokens { get; set; } = [];

    public BrandingInfo? BrandingInfo { get; set; }
    public string? BrandingInfoId { get; set; }

    public ICollection<ScriptResult> ScriptResults { get; set; } = [];

    public ICollection<ScriptRun> ScriptRuns { get; set; } = [];
    public ICollection<SavedScript> SavedScripts { get; set; } = [];

    public ICollection<ScriptSchedule> ScriptSchedules { get; set; } = [];

    public ICollection<DeviceGroup> DeviceGroups { get; set; } = [];

    public ICollection<Device> Devices { get; set; } = [];

    [Key]
    [DatabaseGenerated(DatabaseGeneratedOption.Identity)]
    public string ID { get; set; } = null!;

    public ICollection<InviteLink> InviteLinks { get; set; } = [];

    public bool IsDefaultOrganization { get; set; }

    [StringLength(25)]
    public required string OrganizationName { get; set; }

    /// <summary>
    /// When true, organization administrators may use the Package Manager
    /// (view installed apps, uninstall, install via Chocolatey). Off by
    /// default — installing/removing software is a high-impact action and
    /// must be opted in to per organization.
    /// </summary>
    public bool PackageManagerEnabled { get; set; }

    public ICollection<RemotelyUser> RemotelyUsers { get; set; } = [];
    public ICollection<SharedFile> SharedFiles { get; set; } = [];
}