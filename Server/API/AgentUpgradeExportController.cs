using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.Mvc;
using Remotely.Server.Auth;
using Remotely.Server.Services;
using Remotely.Server.Services.AgentUpgrade;
using System.Globalization;
using System.Text;

namespace Remotely.Server.API;

/// <summary>
/// CSV export endpoint for the M4 admin "Agent upgrade" dashboard
/// (see ROADMAP.md "M4 — Admin 'Agent upgrade' dashboard"). Org-admin
/// authenticated; the response is scoped to the caller's organisation
/// so an operator cannot export rows outside their own org.
/// </summary>
[Route("api/agent-upgrade")]
[ApiController]
[Authorize(Policy = PolicyNames.OrganizationAdminRequired)]
public class AgentUpgradeExportController : ControllerBase
{
    /// <summary>
    /// Hard cap so a runaway organisation with millions of rows cannot
    /// hold a request open or balloon the response. The dashboard's
    /// table is paged independently so this only constrains the CSV.
    /// </summary>
    public const int MaxRows = 50_000;

    private readonly IAgentUpgradeService _service;
    private readonly IAuthService _authService;
    private readonly ILogger<AgentUpgradeExportController> _logger;

    public AgentUpgradeExportController(
        IAgentUpgradeService service,
        IAuthService authService,
        ILogger<AgentUpgradeExportController> logger)
    {
        _service = service;
        _authService = authService;
        _logger = logger;
    }

    [HttpGet("export.csv")]
    public async Task<IActionResult> ExportCsv(
        [FromQuery] string? search,
        CancellationToken cancellationToken)
    {
        var userResult = await _authService.GetUser();
        if (!userResult.IsSuccess || userResult.Value is null)
        {
            return Unauthorized();
        }
        var orgId = userResult.Value.OrganizationID;
        if (string.IsNullOrEmpty(orgId))
        {
            return Unauthorized();
        }

        var rows = await _service.GetRowsForOrganizationAsync(
            orgId, search, skip: 0, take: MaxRows, cancellationToken);

        var bytes = BuildCsv(rows);
        _logger.LogInformation(
            "Agent-upgrade CSV exported by operator. OrgId={orgId} RowCount={count}",
            orgId, rows.Count);

        // Stamp the filename with a UTC timestamp so an operator can
        // diff two snapshots without renaming downloads by hand.
        var stamp = DateTime.UtcNow.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture);
        return File(bytes, "text/csv; charset=utf-8", $"agent-upgrade-{stamp}.csv");
    }

    /// <summary>
    /// Builds the RFC 4180-style CSV body for the given rows. Exposed
    /// internally so the format can be unit-tested without standing up
    /// the controller pipeline.
    /// </summary>
    internal static byte[] BuildCsv(IReadOnlyList<AgentUpgradeRow> rows)
    {
        var sb = new StringBuilder();
        sb.AppendLine("DeviceId,DeviceName,State,FromVersion,ToVersion,LastOnlineUtc,AttemptCount,EligibleAtUtc,LastAttemptAtUtc,CompletedAtUtc,LastAttemptError");
        foreach (var r in rows)
        {
            sb.Append(EscapeCsv(r.DeviceId)).Append(',');
            sb.Append(EscapeCsv(r.DeviceName)).Append(',');
            sb.Append(EscapeCsv(r.State.ToString())).Append(',');
            sb.Append(EscapeCsv(r.FromVersion)).Append(',');
            sb.Append(EscapeCsv(r.ToVersion)).Append(',');
            sb.Append(EscapeCsv(FormatUtc(r.LastOnline))).Append(',');
            sb.Append(r.AttemptCount.ToString(CultureInfo.InvariantCulture)).Append(',');
            sb.Append(EscapeCsv(FormatUtc(r.EligibleAt))).Append(',');
            sb.Append(EscapeCsv(FormatUtc(r.LastAttemptAt))).Append(',');
            sb.Append(EscapeCsv(FormatUtc(r.CompletedAt))).Append(',');
            sb.Append(EscapeCsv(r.LastAttemptError));
            sb.Append('\n');
        }
        // UTF-8 with BOM so Excel opens the file in the right code page
        // without the operator picking the encoding from the import wizard.
        var preamble = Encoding.UTF8.GetPreamble();
        var body = Encoding.UTF8.GetBytes(sb.ToString());
        var output = new byte[preamble.Length + body.Length];
        Buffer.BlockCopy(preamble, 0, output, 0, preamble.Length);
        Buffer.BlockCopy(body, 0, output, preamble.Length, body.Length);
        return output;
    }

    private static string FormatUtc(DateTimeOffset? value) =>
        value is null ? string.Empty : value.Value.UtcDateTime.ToString("u", CultureInfo.InvariantCulture);

    private static string EscapeCsv(string? value)
    {
        if (string.IsNullOrEmpty(value))
        {
            return string.Empty;
        }
        // Quote if the value contains anything that would otherwise
        // confuse the parser (comma, double-quote, CR, LF). RFC 4180
        // doubles internal quotes inside the wrapping pair.
        var needsQuoting = false;
        for (var i = 0; i < value.Length; i++)
        {
            var ch = value[i];
            if (ch == ',' || ch == '"' || ch == '\r' || ch == '\n')
            {
                needsQuoting = true;
                break;
            }
        }
        if (!needsQuoting)
        {
            return value;
        }
        return "\"" + value.Replace("\"", "\"\"") + "\"";
    }
}
