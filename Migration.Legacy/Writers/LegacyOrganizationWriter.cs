using Npgsql;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Writers;

/// <summary>
/// <see cref="ILegacyRowWriter{TV2}"/> for v2 <see cref="Organization"/>
/// rows. Upserts by primary key (<c>"ID"</c>) against the Postgres
/// target so a resumed run re-overwrites the previously written row
/// in place rather than failing on the conflict.
/// </summary>
public class LegacyOrganizationWriter : ILegacyRowWriter<Organization>
{
    public string EntityName => "Organization";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public async Task WriteAsync(
        Organization row,
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
            INSERT INTO "Organizations"
                ("ID", "OrganizationName", "IsDefaultOrganization")
            VALUES (@id, @name, @isDefault)
            ON CONFLICT ("ID") DO UPDATE SET
                "OrganizationName" = EXCLUDED."OrganizationName",
                "IsDefaultOrganization" = EXCLUDED."IsDefaultOrganization";
            """;
        cmd.Parameters.AddWithValue("@id", row.ID);
        cmd.Parameters.AddWithValue("@name", (object?)row.OrganizationName ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@isDefault", row.IsDefaultOrganization);

        await cmd.ExecuteNonQueryAsync(cancellationToken).ConfigureAwait(false);
    }
}
