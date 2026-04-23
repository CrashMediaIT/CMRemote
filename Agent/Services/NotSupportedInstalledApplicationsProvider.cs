using Remotely.Agent.Interfaces;
using Remotely.Shared.Models;
using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Services;

/// <summary>
/// Used on platforms where installed-application enumeration is not yet
/// implemented (Linux, macOS). Returns a clear error rather than throwing
/// so the WebUI can render a friendly "not supported on this OS" state.
/// </summary>
internal sealed class NotSupportedInstalledApplicationsProvider : IInstalledApplicationsProvider
{
    public bool IsSupported => false;

    public Task<(bool Success, string? ErrorMessage, IReadOnlyList<InstalledApplication> Applications)> GetInstalledApplicationsAsync(CancellationToken cancellationToken)
    {
        return Task.FromResult<(bool, string?, IReadOnlyList<InstalledApplication>)>(
            (false, "Installed-applications enumeration is only supported on Windows.", Array.Empty<InstalledApplication>()));
    }

    public Task<(bool Success, int ExitCode, string? Stdout, string? Stderr, string? ErrorMessage)> UninstallApplicationAsync(string applicationKey, CancellationToken cancellationToken)
    {
        return Task.FromResult<(bool, int, string?, string?, string?)>(
            (false, -1, null, null, "Uninstalling applications is only supported on Windows."));
    }
}
