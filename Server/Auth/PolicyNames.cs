namespace Remotely.Server.Auth;

public static class PolicyNames
{
    public const string TwoFactorRequired = nameof(TwoFactorRequired);
    public const string OrganizationAdminRequired = nameof(OrganizationAdminRequired);
    public const string ServerAdminRequired = nameof(ServerAdminRequired);
    public const string PackageManagerRequired = nameof(PackageManagerRequired);
}
