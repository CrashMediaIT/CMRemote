using Remotely.Shared.Entities;
using System;
using System.Collections.Generic;
using System.Linq;

namespace Remotely.Server.PackageManager;

/// <summary>
/// Centralizes the "which devices can the Package Manager actually
/// dispatch to?" rule so the Razor pages and the dispatch service
/// agree on a single definition. Keeps that policy decision in one
/// place — currently Windows-only because Phase 2 only ships a
/// Chocolatey provider on Windows; PR&nbsp;C1 / PR&nbsp;C2 will widen
/// this set as MSI / Executable providers become available on more
/// platforms.
/// </summary>
public static class PackageManagerDeviceFilter
{
    public static bool IsSupportedPlatform(string? platform)
    {
        return string.Equals(platform, "Windows", StringComparison.OrdinalIgnoreCase);
    }

    public static IEnumerable<Device> SupportedDevices(IEnumerable<Device> devices)
    {
        if (devices is null)
        {
            return Array.Empty<Device>();
        }
        return devices.Where(d => IsSupportedPlatform(d.Platform));
    }
}
