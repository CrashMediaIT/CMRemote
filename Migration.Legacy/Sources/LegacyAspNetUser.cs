namespace Remotely.Migration.Legacy.Sources;

/// <summary>
/// Read-only POCO mirror of the legacy upstream <c>AspNetUsers</c>
/// table. Only the ASP.NET Identity columns we round-trip to v2 are
/// materialised here.
///
/// <para>
/// Identity columns (<see cref="PasswordHash"/>,
/// <see cref="SecurityStamp"/>, <see cref="ConcurrencyStamp"/>,
/// <see cref="LockoutEnd"/>, …) are preserved verbatim by the
/// converter so existing user passwords + 2FA secrets keep working
/// after the import — the entire point of the migrator is that
/// operators don't have to ask their users to re-register.
/// </para>
///
/// <para>
/// Authenticator-key / recovery-code columns
/// (<c>AspNetUserTokens</c>, <c>AspNetUserClaims</c>,
/// <c>AspNetUserLogins</c>) live on adjacent tables and are out of
/// scope for the M2 first cut — those land in a follow-up if real
/// migrations turn up users who relied on them. The dominant
/// password + lockout flow round-trips fully here.
/// </para>
/// </summary>
public class LegacyAspNetUser
{
    public required string Id { get; set; }

    public string? UserName { get; set; }
    public string? NormalizedUserName { get; set; }
    public string? Email { get; set; }
    public string? NormalizedEmail { get; set; }
    public bool EmailConfirmed { get; set; }
    public string? PasswordHash { get; set; }
    public string? SecurityStamp { get; set; }
    public string? ConcurrencyStamp { get; set; }
    public string? PhoneNumber { get; set; }
    public bool PhoneNumberConfirmed { get; set; }
    public bool TwoFactorEnabled { get; set; }
    public DateTimeOffset? LockoutEnd { get; set; }
    public bool LockoutEnabled { get; set; }
    public int AccessFailedCount { get; set; }

    // CMRemote-specific columns layered onto IdentityUser by the
    // upstream schema.
    public string? OrganizationID { get; set; }
    public bool IsAdministrator { get; set; }
    public bool IsServerAdmin { get; set; }
}
