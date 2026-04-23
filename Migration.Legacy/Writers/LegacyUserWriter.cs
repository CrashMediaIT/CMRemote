using Npgsql;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Writers;

/// <summary>
/// <see cref="ILegacyRowWriter{TV2}"/> for v2 <see cref="RemotelyUser"/>
/// rows. Upserts by primary key (<c>"Id"</c>; lower-case
/// <c>d</c>, per ASP.NET Identity convention).
///
/// <para>
/// Identity columns (<c>PasswordHash</c>, <c>SecurityStamp</c>, …)
/// are written verbatim so existing user passwords survive the
/// migration. The conflict update overwrites them too, so a re-run
/// against a target that already has the user is byte-stable rather
/// than just no-op.
/// </para>
/// </summary>
public class LegacyUserWriter : ILegacyRowWriter<RemotelyUser>
{
    public string EntityName => "User";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public async Task WriteAsync(
        RemotelyUser row,
        string targetConnectionString,
        CancellationToken cancellationToken = default)
    {
        if (row is null)
        {
            throw new ArgumentNullException(nameof(row));
        }

        await using var conn = PostgresWriterRuntime.ValidateAndCreate(targetConnectionString);
        await conn.OpenAsync(cancellationToken).ConfigureAwait(false);

        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            INSERT INTO "AspNetUsers" (
                "Id", "UserName", "NormalizedUserName", "Email", "NormalizedEmail",
                "EmailConfirmed", "PasswordHash", "SecurityStamp", "ConcurrencyStamp",
                "PhoneNumber", "PhoneNumberConfirmed", "TwoFactorEnabled",
                "LockoutEnd", "LockoutEnabled", "AccessFailedCount",
                "OrganizationID", "IsAdministrator", "IsServerAdmin")
            VALUES (
                @id, @userName, @normUserName, @email, @normEmail,
                @emailConfirmed, @pwHash, @secStamp, @concStamp,
                @phone, @phoneConfirmed, @twoFactor,
                @lockoutEnd, @lockoutEnabled, @accessFailed,
                @orgId, @isAdmin, @isServerAdmin)
            ON CONFLICT ("Id") DO UPDATE SET
                "UserName" = EXCLUDED."UserName",
                "NormalizedUserName" = EXCLUDED."NormalizedUserName",
                "Email" = EXCLUDED."Email",
                "NormalizedEmail" = EXCLUDED."NormalizedEmail",
                "EmailConfirmed" = EXCLUDED."EmailConfirmed",
                "PasswordHash" = EXCLUDED."PasswordHash",
                "SecurityStamp" = EXCLUDED."SecurityStamp",
                "ConcurrencyStamp" = EXCLUDED."ConcurrencyStamp",
                "PhoneNumber" = EXCLUDED."PhoneNumber",
                "PhoneNumberConfirmed" = EXCLUDED."PhoneNumberConfirmed",
                "TwoFactorEnabled" = EXCLUDED."TwoFactorEnabled",
                "LockoutEnd" = EXCLUDED."LockoutEnd",
                "LockoutEnabled" = EXCLUDED."LockoutEnabled",
                "AccessFailedCount" = EXCLUDED."AccessFailedCount",
                "OrganizationID" = EXCLUDED."OrganizationID",
                "IsAdministrator" = EXCLUDED."IsAdministrator",
                "IsServerAdmin" = EXCLUDED."IsServerAdmin";
            """;

        cmd.Parameters.AddWithValue("@id", row.Id);
        cmd.Parameters.AddWithValue("@userName", (object?)row.UserName ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@normUserName", (object?)row.NormalizedUserName ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@email", (object?)row.Email ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@normEmail", (object?)row.NormalizedEmail ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@emailConfirmed", row.EmailConfirmed);
        cmd.Parameters.AddWithValue("@pwHash", (object?)row.PasswordHash ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@secStamp", (object?)row.SecurityStamp ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@concStamp", (object?)row.ConcurrencyStamp ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@phone", (object?)row.PhoneNumber ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@phoneConfirmed", row.PhoneNumberConfirmed);
        cmd.Parameters.AddWithValue("@twoFactor", row.TwoFactorEnabled);
        cmd.Parameters.AddWithValue("@lockoutEnd", (object?)row.LockoutEnd ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@lockoutEnabled", row.LockoutEnabled);
        cmd.Parameters.AddWithValue("@accessFailed", row.AccessFailedCount);
        cmd.Parameters.AddWithValue("@orgId", row.OrganizationID);
        cmd.Parameters.AddWithValue("@isAdmin", row.IsAdministrator);
        cmd.Parameters.AddWithValue("@isServerAdmin", row.IsServerAdmin);

        await cmd.ExecuteNonQueryAsync(cancellationToken).ConfigureAwait(false);
    }
}
