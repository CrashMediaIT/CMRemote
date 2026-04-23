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

    /// <summary>
    /// SharedFile id the agent fetches from
    /// <c>&lt;server&gt;/api/filesharing/{MsiSharedFileId}</c>. Populated
    /// by the server only when <see cref="Provider"/> is
    /// <c>UploadedMsi</c>; ignored otherwise. The agent must include
    /// <see cref="MsiAuthToken"/> in the <c>X-Expiring-Token</c> header
    /// — the URL itself carries no secret, so a leaked log line cannot
    /// be replayed.
    /// </summary>
    public string? MsiSharedFileId { get; set; }

    /// <summary>
    /// Short-lived expiring auth token (minted via
    /// <c>IExpiringTokenService</c>) the agent presents in the
    /// <c>X-Expiring-Token</c> header when fetching
    /// <see cref="MsiSharedFileId"/>. TTL is bounded server-side
    /// (currently a few minutes) so a copy of the request DTO can't be
    /// replayed indefinitely.
    /// </summary>
    public string? MsiAuthToken { get; set; }

    /// <summary>
    /// Lowercase hex SHA-256 of the MSI bytes recorded at upload time.
    /// The agent re-hashes what it downloads and refuses to install on
    /// mismatch — protects against a tampered cache or an in-flight
    /// substitution at the storage layer.
    /// </summary>
    public string? MsiSha256 { get; set; }

    /// <summary>
    /// Operator-uploaded filename, used only as the on-disk leaf name
    /// the agent writes to before invoking <c>msiexec</c>. Must already
    /// be sanitised server-side (no path separators, no NUL).
    /// </summary>
    public string? MsiFileName { get; set; }
}
