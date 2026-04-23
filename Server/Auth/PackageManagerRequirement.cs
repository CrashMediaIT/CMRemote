using Microsoft.AspNetCore.Authorization;

namespace Remotely.Server.Auth;

/// <summary>
/// Marker requirement satisfied when the current user is an organization
/// administrator AND their organization has the Package Manager feature
/// enabled. Used to gate the per-device package-manager pages and hub
/// methods.
/// </summary>
public class PackageManagerRequirement : IAuthorizationRequirement
{
}
