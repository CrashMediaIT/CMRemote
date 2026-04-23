using Npgsql;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Writers;

/// <summary>
/// <see cref="ILegacyRowWriter{TV2}"/> for v2 <see cref="Device"/>
/// rows. Upserts by primary key (<c>"ID"</c>).
///
/// <para>
/// Telemetry counters (CPU / memory / storage) are written verbatim
/// from the converter; the live agent re-asserts them on its next
/// check-in, so a stale snapshot from the upstream DB is acceptable
/// in the meantime. <see cref="Device.IsOnline"/> is forced to
/// <c>false</c> by the converter so the panel doesn't claim devices
/// are online before the agent has re-handshaked.
/// </para>
/// </summary>
public class LegacyDeviceWriter : ILegacyRowWriter<Device>
{
    public string EntityName => "Device";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public async Task WriteAsync(
        Device row,
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
            INSERT INTO "Devices" (
                "ID", "OrganizationID", "DeviceName", "Alias", "Tags", "Notes",
                "Platform", "OSDescription", "AgentVersion", "CurrentUser",
                "PublicIP", "DeviceGroupID", "ServerVerificationToken",
                "Is64Bit", "IsOnline", "LastOnline", "ProcessorCount",
                "CpuUtilization", "TotalMemory", "UsedMemory", "TotalStorage",
                "UsedStorage", "OSArchitecture", "MacAddresses")
            VALUES (
                @id, @orgId, @deviceName, @alias, @tags, @notes,
                @platform, @osDesc, @agentVer, @currentUser,
                @publicIp, @deviceGroupId, @serverToken,
                @is64bit, @isOnline, @lastOnline, @procCount,
                @cpu, @totalMem, @usedMem, @totalStorage,
                @usedStorage, @osArch, @macs)
            ON CONFLICT ("ID") DO UPDATE SET
                "OrganizationID" = EXCLUDED."OrganizationID",
                "DeviceName" = EXCLUDED."DeviceName",
                "Alias" = EXCLUDED."Alias",
                "Tags" = EXCLUDED."Tags",
                "Notes" = EXCLUDED."Notes",
                "Platform" = EXCLUDED."Platform",
                "OSDescription" = EXCLUDED."OSDescription",
                "AgentVersion" = EXCLUDED."AgentVersion",
                "CurrentUser" = EXCLUDED."CurrentUser",
                "PublicIP" = EXCLUDED."PublicIP",
                "DeviceGroupID" = EXCLUDED."DeviceGroupID",
                "ServerVerificationToken" = EXCLUDED."ServerVerificationToken",
                "Is64Bit" = EXCLUDED."Is64Bit",
                "IsOnline" = EXCLUDED."IsOnline",
                "LastOnline" = EXCLUDED."LastOnline",
                "ProcessorCount" = EXCLUDED."ProcessorCount",
                "CpuUtilization" = EXCLUDED."CpuUtilization",
                "TotalMemory" = EXCLUDED."TotalMemory",
                "UsedMemory" = EXCLUDED."UsedMemory",
                "TotalStorage" = EXCLUDED."TotalStorage",
                "UsedStorage" = EXCLUDED."UsedStorage",
                "OSArchitecture" = EXCLUDED."OSArchitecture",
                "MacAddresses" = EXCLUDED."MacAddresses";
            """;

        cmd.Parameters.AddWithValue("@id", row.ID);
        cmd.Parameters.AddWithValue("@orgId", row.OrganizationID);
        cmd.Parameters.AddWithValue("@deviceName", (object?)row.DeviceName ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@alias", (object?)row.Alias ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@tags", (object?)row.Tags ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@notes", (object?)row.Notes ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@platform", (object?)row.Platform ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@osDesc", (object?)row.OSDescription ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@agentVer", (object?)row.AgentVersion ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@currentUser", (object?)row.CurrentUser ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@publicIp", (object?)row.PublicIP ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@deviceGroupId", (object?)row.DeviceGroupID ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@serverToken", (object?)row.ServerVerificationToken ?? DBNull.Value);
        cmd.Parameters.AddWithValue("@is64bit", row.Is64Bit);
        cmd.Parameters.AddWithValue("@isOnline", row.IsOnline);
        cmd.Parameters.AddWithValue("@lastOnline", row.LastOnline);
        cmd.Parameters.AddWithValue("@procCount", row.ProcessorCount);
        cmd.Parameters.AddWithValue("@cpu", row.CpuUtilization);
        cmd.Parameters.AddWithValue("@totalMem", row.TotalMemory);
        cmd.Parameters.AddWithValue("@usedMem", row.UsedMemory);
        cmd.Parameters.AddWithValue("@totalStorage", row.TotalStorage);
        cmd.Parameters.AddWithValue("@usedStorage", row.UsedStorage);
        cmd.Parameters.AddWithValue("@osArch", (int)row.OSArchitecture);
        cmd.Parameters.AddWithValue("@macs", row.MacAddresses ?? Array.Empty<string>());

        await cmd.ExecuteNonQueryAsync(cancellationToken).ConfigureAwait(false);
    }
}
