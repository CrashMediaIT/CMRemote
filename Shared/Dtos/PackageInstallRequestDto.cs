using Remotely.Shared.Enums;

namespace Remotely.Shared.Dtos;

/// <summary>
/// Wire payload describing a single package install/uninstall request
/// sent from the server to an agent. The agent re-resolves the actual
/// command line locally (Chocolatey package id → <c>choco install …</c>)
/// — never accept an executable string from the wire.
/// </summary>
public class PackageInstallRequestDto
{
    public string JobId { get; set; } = string.Empty;

    public PackageProvider Provider { get; set; }

    public PackageInstallAction Action { get; set; }

    /// <summary>
    /// Provider-specific identifier (Chocolatey package id, MSI file
    /// id, etc.) — see <c>Package.PackageIdentifier</c>.
    /// </summary>
    public string PackageIdentifier { get; set; } = string.Empty;

    public string? Version { get; set; }

    /// <summary>
    /// Operator-supplied install arguments. Already validated server-side
    /// for shell metacharacters before reaching the agent — agents must
    /// still pass them as discrete args, never via a shell.
    /// </summary>
    public string? InstallArguments { get; set; }
}
