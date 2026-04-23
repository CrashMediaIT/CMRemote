using Remotely.Shared.Dtos;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Interfaces;

/// <summary>
/// Per-OS / per-provider implementation of the agent-side install /
/// uninstall workflow. The server hands the agent a
/// <see cref="PackageInstallRequestDto"/>; the provider resolves it
/// locally to a real command line — the wire MUST NOT carry an
/// executable string. Implementations must not throw — failures are
/// surfaced via the returned <see cref="PackageInstallResultDto"/>.
/// </summary>
public interface IPackageProvider
{
    /// <summary>
    /// True when this provider can service the request on the current
    /// host (e.g. <c>choco.exe</c> exists on PATH for the Chocolatey
    /// provider).
    /// </summary>
    bool CanHandle(PackageInstallRequestDto request);

    Task<PackageInstallResultDto> ExecuteAsync(PackageInstallRequestDto request, CancellationToken cancellationToken);
}
