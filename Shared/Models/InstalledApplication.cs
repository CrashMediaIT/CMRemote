using Remotely.Shared.Enums;

namespace Remotely.Shared.Models;

/// <summary>
/// Represents an installed application enumerated by the agent and surfaced
/// to the WebUI. <see cref="ApplicationKey"/> is a stable identifier the
/// server uses to issue an opaque uninstall token; the agent never accepts
/// raw uninstall strings from the wire.
/// </summary>
public class InstalledApplication
{
    /// <summary>
    /// Stable identifier for this application within a snapshot.
    /// For Win32/Msi this is the registry sub-key name (e.g.
    /// <c>{B7A0CE06-068E-11D6-97FD-0050BACBF861}</c> or a
    /// vendor-specific key). For AppX this is the PackageFullName.
    /// </summary>
    public string ApplicationKey { get; set; } = string.Empty;

    public InstalledApplicationSource Source { get; set; }

    public string Name { get; set; } = string.Empty;

    public string? Version { get; set; }

    public string? Publisher { get; set; }

    /// <summary>
    /// Original install date as reported by the registry (yyyyMMdd) or
    /// the AppX manifest. Stored as ISO-8601 string for portability.
    /// </summary>
    public string? InstallDate { get; set; }

    public long? SizeBytes { get; set; }

    public string? InstallLocation { get; set; }

    /// <summary>
    /// True for OS components / hidden updates that should be hidden by
    /// default in the UI.
    /// </summary>
    public bool IsSystemComponent { get; set; }

    /// <summary>
    /// True when the agent has a reliable silent-uninstall path
    /// (QuietUninstallString, MSI ProductCode, or AppX). When false the
    /// UI must surface a confirmation prompt before the operator can
    /// proceed.
    /// </summary>
    public bool CanUninstallSilently { get; set; }
}
