namespace Remotely.Shared.Enums;

/// <summary>
/// Whether a package operation is an install or an uninstall. A single
/// enum lets <c>PackageInstallJob</c> represent both flows uniformly.
/// </summary>
public enum PackageInstallAction
{
    Install = 0,
    Uninstall = 1,
}
