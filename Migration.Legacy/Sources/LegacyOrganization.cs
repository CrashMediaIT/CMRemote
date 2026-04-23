namespace Remotely.Migration.Legacy.Sources;

/// <summary>
/// Read-only POCO mirror of the legacy upstream <c>Organizations</c>
/// table. Lives in <see cref="Remotely.Migration.Legacy"/> rather than
/// <see cref="Remotely.Shared.Entities"/> so the v2 entity definition
/// can evolve freely without breaking the importer's read shape.
///
/// The legacy-DB reader (next M2 slice) populates instances of these
/// POCOs from the source connection; this scaffold only declares the
/// shape so the reference <c>OrganizationRowConverter</c> has
/// something concrete to convert from.
/// </summary>
public class LegacyOrganization
{
    public required string ID { get; set; }

    public string? OrganizationName { get; set; }

    public bool IsDefaultOrganization { get; set; }
}
