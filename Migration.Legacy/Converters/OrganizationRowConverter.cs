using Remotely.Migration.Legacy.Sources;
using Remotely.Shared.Entities;

namespace Remotely.Migration.Legacy.Converters;

/// <summary>
/// Reference <see cref="IRowConverter{TLegacy, TV2}"/> implementation:
/// maps an upstream <see cref="LegacyOrganization"/> row to the v2
/// <see cref="Organization"/> entity.
///
/// Converters in M2 are deliberately small and dumb. The mapping below
/// is the trivial "preserve identity, copy name, default the v2-only
/// fields" case — everything that's harder than this (devices' shared
/// secrets, scripts' encrypted columns, alerts' FK chains) lands in
/// per-entity converter classes in subsequent M2 slices.
/// </summary>
public class OrganizationRowConverter : IRowConverter<LegacyOrganization, Organization>
{
    public string EntityName => "Organization";

    public LegacySchemaVersion HandlesSchemaVersion =>
        LegacySchemaVersion.UpstreamLegacy_2026_04;

    public ConverterResult<Organization> Convert(LegacyOrganization legacyRow)
    {
        if (legacyRow is null)
        {
            return ConverterResult<Organization>.Fail("Legacy row was null.");
        }

        if (string.IsNullOrWhiteSpace(legacyRow.ID))
        {
            return ConverterResult<Organization>.Fail("Legacy organization row has no ID.");
        }

        // The v2 Organization.OrganizationName column is required (per
        // its [StringLength(25)] / required modifier). An upstream row
        // without one cannot legally be written to the v2 schema, so
        // skip rather than fail — these are typically left over from
        // half-deleted test orgs and are not worth aborting the run.
        if (string.IsNullOrWhiteSpace(legacyRow.OrganizationName))
        {
            return ConverterResult<Organization>.Skip(
                $"Legacy organization {legacyRow.ID} has no name.");
        }

        var name = legacyRow.OrganizationName!.Trim();
        if (name.Length > 25)
        {
            // v2 enforces a 25-character cap; truncate rather than skip
            // because the operator-visible field is "best-effort
            // searchable" and dropping a whole org over a long name is
            // worse than truncating. The truncation is recorded as a
            // converted row (not a skip) so the operator sees no
            // missing-data warning in the report.
            name = name.Substring(0, 25);
        }

        return ConverterResult<Organization>.Ok(new Organization
        {
            // Identity preservation per ROADMAP M1.3 — devices keyed
            // off this org keep their existing OrganizationID.
            ID = legacyRow.ID,
            OrganizationName = name,
            IsDefaultOrganization = legacyRow.IsDefaultOrganization,
        });
    }
}
