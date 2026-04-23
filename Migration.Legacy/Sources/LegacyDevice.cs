namespace Remotely.Migration.Legacy.Sources;

/// <summary>
/// Read-only POCO mirror of the legacy upstream <c>Devices</c> table.
/// Lives in <see cref="Remotely.Migration.Legacy"/> rather than
/// <see cref="Remotely.Shared.Entities"/> so the v2 entity definition
/// can evolve freely without breaking the importer's read shape.
///
/// <para>
/// Only the minimal scalar subset that the M2 first-cut Devices
/// converter actually needs is materialised here. Columns that
/// require richer marshalling on the source side (the legacy
/// <c>Drives</c> JSON column, the <c>MacAddresses</c> text[] / nvarchar
/// blob, telemetry counters that are reliably re-populated by the
/// agent on next check-in) are intentionally omitted to keep the
/// reader's per-provider SQL portable across SQLite / SQL Server /
/// PostgreSQL upstream installs. The converter defaults the missing
/// fields and the live agent re-fills them on its next check-in.
/// </para>
/// </summary>
public class LegacyDevice
{
    public required string ID { get; set; }

    public string? OrganizationID { get; set; }

    public string? DeviceName { get; set; }
    public string? Alias { get; set; }
    public string? Tags { get; set; }
    public string? Notes { get; set; }
    public string? Platform { get; set; }
    public string? OSDescription { get; set; }
    public string? AgentVersion { get; set; }
    public string? CurrentUser { get; set; }
    public string? PublicIP { get; set; }
    public string? DeviceGroupID { get; set; }
    public string? ServerVerificationToken { get; set; }

    public bool Is64Bit { get; set; }
    public bool IsOnline { get; set; }
    public DateTimeOffset LastOnline { get; set; }
    public int ProcessorCount { get; set; }
    public double CpuUtilization { get; set; }
    public double TotalMemory { get; set; }
    public double UsedMemory { get; set; }
    public double TotalStorage { get; set; }
    public double UsedStorage { get; set; }

    /// <summary>
    /// Raw integer value of the upstream <c>OSArchitecture</c>
    /// enum column; the converter casts to
    /// <see cref="System.Runtime.InteropServices.Architecture"/>.
    /// </summary>
    public int OSArchitecture { get; set; }
}
