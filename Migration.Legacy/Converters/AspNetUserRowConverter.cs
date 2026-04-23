using Remotely.Migration.Legacy.Sources;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Converters;

/// <summary>
/// Maps an upstream <see cref="LegacyAspNetUser"/> row to the v2
/// <see cref="RemotelyUser"/> entity.
///
/// <para>
/// All ASP.NET Identity scalar columns round-trip verbatim
/// (<see cref="RemotelyUser.PasswordHash"/>,
/// <see cref="RemotelyUser.SecurityStamp"/>,
/// <see cref="RemotelyUser.ConcurrencyStamp"/>,
/// <see cref="RemotelyUser.LockoutEnd"/>, …) so that existing users
/// keep their passwords and 2FA state after the migration. This is
/// the entire reason an importer exists rather than asking operators
/// to re-invite users.
/// </para>
///
/// <para>
/// Skip / fail rules:
/// <list type="bullet">
///   <item><c>Id</c> missing → <see cref="ConverterResult{T}.Fail"/>.</item>
///   <item><c>UserName</c> missing → <see cref="ConverterResult{T}.Fail"/>
///         (Identity requires a username; a row without one would
///         either fail to upsert or shadow a legitimate user).</item>
///   <item><c>OrganizationID</c> missing → <see cref="ConverterResult{T}.Skip"/>
///         (orphaned-org leftover, same as <see cref="DeviceRowConverter"/>).</item>
/// </list>
/// </para>
/// </summary>
public class AspNetUserRowConverter : IRowConverter<LegacyAspNetUser, RemotelyUser>
{
    public string EntityName => "User";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public ConverterResult<RemotelyUser> Convert(LegacyAspNetUser legacyRow)
    {
        if (legacyRow is null)
        {
            return ConverterResult<RemotelyUser>.Fail("Legacy row was null.");
        }

        if (string.IsNullOrWhiteSpace(legacyRow.Id))
        {
            return ConverterResult<RemotelyUser>.Fail("Legacy user row has no Id.");
        }

        if (string.IsNullOrWhiteSpace(legacyRow.UserName))
        {
            return ConverterResult<RemotelyUser>.Fail(
                $"Legacy user {legacyRow.Id} has no UserName.");
        }

        if (string.IsNullOrWhiteSpace(legacyRow.OrganizationID))
        {
            return ConverterResult<RemotelyUser>.Skip(
                $"Legacy user {legacyRow.Id} has no OrganizationID.");
        }

        return ConverterResult<RemotelyUser>.Ok(new RemotelyUser
        {
            Id = legacyRow.Id,
            UserName = legacyRow.UserName,
            NormalizedUserName = legacyRow.NormalizedUserName
                ?? legacyRow.UserName.ToUpperInvariant(),
            Email = legacyRow.Email,
            NormalizedEmail = legacyRow.NormalizedEmail
                ?? legacyRow.Email?.ToUpperInvariant(),
            EmailConfirmed = legacyRow.EmailConfirmed,
            PasswordHash = legacyRow.PasswordHash,
            SecurityStamp = legacyRow.SecurityStamp,
            // ConcurrencyStamp is regenerated on every Identity write;
            // prefer the upstream value when present so an in-flight
            // optimistic-concurrency check on a parallel server still
            // sees the same stamp.
            ConcurrencyStamp = legacyRow.ConcurrencyStamp ?? Guid.NewGuid().ToString(),
            PhoneNumber = legacyRow.PhoneNumber,
            PhoneNumberConfirmed = legacyRow.PhoneNumberConfirmed,
            TwoFactorEnabled = legacyRow.TwoFactorEnabled,
            LockoutEnd = legacyRow.LockoutEnd,
            LockoutEnabled = legacyRow.LockoutEnabled,
            AccessFailedCount = legacyRow.AccessFailedCount,
            OrganizationID = legacyRow.OrganizationID!,
            IsAdministrator = legacyRow.IsAdministrator,
            IsServerAdmin = legacyRow.IsServerAdmin,
        });
    }
}
