using Remotely.Shared.Models;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Interfaces;

/// <summary>
/// Enumerates installed applications on the local machine and uninstalls
/// them by stable application key. Implemented per-OS — only Windows is
/// supported in Phase 1; other platforms return a not-supported result.
/// </summary>
public interface IInstalledApplicationsProvider
{
    /// <summary>
    /// Returns true on platforms where enumeration / uninstall is
    /// supported by this build of the agent.
    /// </summary>
    bool IsSupported { get; }

    /// <summary>
    /// Enumerate installed apps. Implementations must not throw —
    /// failures are surfaced via the returned tuple.
    /// </summary>
    Task<(bool Success, string? ErrorMessage, IReadOnlyList<InstalledApplication> Applications)> GetInstalledApplicationsAsync(CancellationToken cancellationToken);

    /// <summary>
    /// Uninstall the application identified by <paramref name="applicationKey"/>.
    /// The implementation re-enumerates locally and resolves the
    /// uninstall command itself — never accept an executable string from
    /// the wire.
    /// </summary>
    Task<(bool Success, int ExitCode, string? Stdout, string? Stderr, string? ErrorMessage)> UninstallApplicationAsync(string applicationKey, CancellationToken cancellationToken);
}
