namespace Remotely.Shared.Enums;

/// <summary>
/// Identifies which agent-side provider is responsible for installing
/// or uninstalling a <c>Package</c>. Each provider corresponds to a
/// concrete implementation behind <c>IPackageProvider</c> on the agent.
/// </summary>
public enum PackageProvider
{
    Unknown = 0,

    /// <summary>
    /// Chocolatey (<c>choco install -y --no-progress &lt;id&gt;</c>). Windows-only.
    /// </summary>
    Chocolatey = 1,

    /// <summary>
    /// Org-uploaded MSI from the <c>UploadedMsis</c> library, installed
    /// via <c>msiexec /i &lt;file&gt; /qn /norestart</c>. Wired up in PR C1.
    /// </summary>
    UploadedMsi = 2,

    /// <summary>
    /// Operator-defined executable + silent-install switches, fetched as
    /// a SharedFile and executed. Wired up in PR C1.
    /// </summary>
    Executable = 3,
}
