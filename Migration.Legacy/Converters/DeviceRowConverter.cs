using System.Runtime.InteropServices;
using Remotely.Migration.Legacy.Sources;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Converters;

/// <summary>
/// Maps an upstream <see cref="LegacyDevice"/> row to the v2
/// <see cref="Device"/> entity.
///
/// <para>
/// Identity preservation per ROADMAP M1.3 — the v2 row keeps the
/// upstream <c>Device.ID</c> verbatim, so the agent's persisted
/// device id keeps matching after the migration. The
/// <see cref="Device.OrganizationID"/> FK is also preserved
/// (matched against the rows produced by
/// <see cref="OrganizationRowConverter"/>) so devices stay attached
/// to the right org.
/// </para>
///
/// <para>
/// Skip / fail rules:
/// <list type="bullet">
///   <item><c>ID</c> missing → <see cref="ConverterResult{T}.Fail"/>
///         (a device without a primary key is not legally writable).</item>
///   <item><c>OrganizationID</c> missing → <see cref="ConverterResult{T}.Skip"/>
///         (an orphaned device row is left over from a half-deleted
///         org and is not worth aborting the run).</item>
/// </list>
/// </para>
/// </summary>
public class DeviceRowConverter : IRowConverter<LegacyDevice, Device>
{
    public string EntityName => "Device";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public ConverterResult<Device> Convert(LegacyDevice legacyRow)
    {
        if (legacyRow is null)
        {
            return ConverterResult<Device>.Fail("Legacy row was null.");
        }

        if (string.IsNullOrWhiteSpace(legacyRow.ID))
        {
            return ConverterResult<Device>.Fail("Legacy device row has no ID.");
        }

        if (string.IsNullOrWhiteSpace(legacyRow.OrganizationID))
        {
            return ConverterResult<Device>.Skip(
                $"Legacy device {legacyRow.ID} has no OrganizationID.");
        }

        // The v2 Alias column is capped at 100 chars; truncate
        // rather than skip — same rationale as the org-name
        // truncation in OrganizationRowConverter.
        var alias = legacyRow.Alias;
        if (alias is not null && alias.Length > 100)
        {
            alias = alias.Substring(0, 100);
        }

        var tags = legacyRow.Tags;
        if (tags is not null && tags.Length > 200)
        {
            tags = tags.Substring(0, 200);
        }

        var notes = legacyRow.Notes;
        if (notes is not null && notes.Length > 5000)
        {
            notes = notes.Substring(0, 5000);
        }

        return ConverterResult<Device>.Ok(new Device
        {
            ID = legacyRow.ID,
            OrganizationID = legacyRow.OrganizationID!,
            DeviceName = legacyRow.DeviceName,
            Alias = alias,
            Tags = tags,
            Notes = notes,
            Platform = legacyRow.Platform,
            OSDescription = legacyRow.OSDescription,
            AgentVersion = legacyRow.AgentVersion,
            CurrentUser = legacyRow.CurrentUser,
            PublicIP = legacyRow.PublicIP,
            DeviceGroupID = legacyRow.DeviceGroupID,
            ServerVerificationToken = legacyRow.ServerVerificationToken,
            Is64Bit = legacyRow.Is64Bit,
            // After a migration every device starts offline — the
            // agent's next check-in re-asserts liveness.
            IsOnline = false,
            LastOnline = legacyRow.LastOnline,
            ProcessorCount = legacyRow.ProcessorCount,
            CpuUtilization = legacyRow.CpuUtilization,
            TotalMemory = legacyRow.TotalMemory,
            UsedMemory = legacyRow.UsedMemory,
            TotalStorage = legacyRow.TotalStorage,
            UsedStorage = legacyRow.UsedStorage,
            OSArchitecture = (Architecture)legacyRow.OSArchitecture,
            // The complex Drives + MacAddresses columns are populated
            // by the next agent check-in; default to empty here.
            MacAddresses = Array.Empty<string>(),
        });
    }
}
