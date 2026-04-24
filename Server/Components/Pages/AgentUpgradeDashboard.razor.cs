using Microsoft.AspNetCore.Components;
using Microsoft.AspNetCore.Components.Web;
using Remotely.Server.Services;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Enums;

namespace Remotely.Server.Components.Pages;

/// <summary>
/// M4 — Admin "Agent upgrade" dashboard
/// (see ROADMAP.md "M4 — Admin 'Agent upgrade' dashboard").
/// Surfaces totals from <see cref="IAgentUpgradeService.GetStateCountsAsync"/>
/// and a paged + searchable table from
/// <see cref="IAgentUpgradeService.GetRowsForOrganizationAsync"/>; the
/// per-row Retry / Skip / Force buttons go through the org-scoped
/// service overloads so an operator cannot mutate rows outside their
/// own organisation.
/// </summary>
public partial class AgentUpgradeDashboard : AuthComponentBase
{
    /// <summary>Default page size for the rows table.</summary>
    public const int PageSize = 25;

    private IReadOnlyDictionary<AgentUpgradeState, int> _counts =
        new Dictionary<AgentUpgradeState, int>();
    private IReadOnlyList<AgentUpgradeRow> _rows = Array.Empty<AgentUpgradeRow>();
    private int _totalRows;
    private int _page;
    private string _search = string.Empty;
    private string? _flashMessage;
    private string? _flashStyle;
    private bool _busy;

    [Inject]
    public required IAgentUpgradeService AgentUpgradeService { get; init; }

    protected override async Task OnInitializedAsync()
    {
        await base.OnInitializedAsync();
        EnsureUserSet();
        await ReloadAsync();
    }

    private string? OrgId => User?.OrganizationID;

    private int TotalPages =>
        _totalRows <= 0 ? 1 : (int)Math.Ceiling(_totalRows / (double)PageSize);

    private bool HasPrevious => _page > 0;
    private bool HasNext => _page + 1 < TotalPages;

    private async Task ReloadAsync()
    {
        if (string.IsNullOrEmpty(OrgId))
        {
            return;
        }
        _counts = await AgentUpgradeService.GetStateCountsAsync(OrgId);
        _totalRows = await AgentUpgradeService.CountRowsForOrganizationAsync(OrgId, _search);
        // Clamp the current page if the result set shrank under us
        // (e.g. a force-retry merged two states).
        if (_page >= TotalPages)
        {
            _page = Math.Max(0, TotalPages - 1);
        }
        _rows = await AgentUpgradeService.GetRowsForOrganizationAsync(
            OrgId, _search, _page * PageSize, PageSize);
    }

    private async Task OnSearchSubmitted(EventArgs _)
    {
        _page = 0;
        await ReloadAsync();
    }

    private async Task OnSearchKeyDownAsync(KeyboardEventArgs e)
    {
        if (e.Key == "Enter")
        {
            await OnSearchSubmitted(EventArgs.Empty);
        }
    }

    private async Task ClearSearchAsync()
    {
        _search = string.Empty;
        _page = 0;
        await ReloadAsync();
    }

    private async Task PreviousPageAsync()
    {
        if (!HasPrevious) return;
        _page -= 1;
        await ReloadAsync();
    }

    private async Task NextPageAsync()
    {
        if (!HasNext) return;
        _page += 1;
        await ReloadAsync();
    }

    private async Task ForceRetryAsync(AgentUpgradeRow row)
    {
        if (string.IsNullOrEmpty(OrgId) || _busy) return;
        _busy = true;
        try
        {
            var ok = await AgentUpgradeService.ForceRetryAsync(row.Id, OrgId);
            SetFlash(ok
                ? $"Force-retried {DescribeDevice(row)}."
                : $"Could not retry {DescribeDevice(row)} (row not found in this organisation).",
                ok);
            await ReloadAsync();
        }
        finally
        {
            _busy = false;
        }
    }

    private async Task SetOptOutAsync(AgentUpgradeRow row)
    {
        if (string.IsNullOrEmpty(OrgId) || _busy) return;
        _busy = true;
        try
        {
            var ok = await AgentUpgradeService.SetOptOutAsync(row.Id, OrgId);
            SetFlash(ok
                ? $"Skipped {DescribeDevice(row)}."
                : $"Could not skip {DescribeDevice(row)} (an upgrade is in progress).",
                ok);
            await ReloadAsync();
        }
        finally
        {
            _busy = false;
        }
    }

    private void SetFlash(string message, bool success)
    {
        _flashMessage = message;
        _flashStyle = success ? "alert alert-success" : "alert alert-warning";
    }

    private void DismissFlash()
    {
        _flashMessage = null;
        _flashStyle = null;
    }

    private static string DescribeDevice(AgentUpgradeRow row) =>
        !string.IsNullOrWhiteSpace(row.DeviceName) ? row.DeviceName! : row.DeviceId;

    private static string FormatLastOnlineAge(DateTimeOffset? lastOnline)
    {
        if (lastOnline is null)
        {
            return "—";
        }
        var age = DateTimeOffset.UtcNow - lastOnline.Value;
        if (age.TotalSeconds < 0)
        {
            return "now";
        }
        if (age.TotalMinutes < 1) return "<1m";
        if (age.TotalMinutes < 60) return $"{(int)age.TotalMinutes}m";
        if (age.TotalHours < 24) return $"{(int)age.TotalHours}h";
        if (age.TotalDays < 60) return $"{(int)age.TotalDays}d";
        return $"{(int)(age.TotalDays / 30)}mo";
    }

    private static string StateBadgeCss(AgentUpgradeState state) => state switch
    {
        AgentUpgradeState.Pending => "bg-secondary",
        AgentUpgradeState.Scheduled => "bg-info text-dark",
        AgentUpgradeState.InProgress => "bg-primary",
        AgentUpgradeState.Succeeded => "bg-success",
        AgentUpgradeState.Failed => "bg-danger",
        AgentUpgradeState.SkippedInactive => "bg-dark",
        AgentUpgradeState.SkippedOptOut => "bg-warning text-dark",
        _ => "bg-secondary",
    };

    private static string StateLabel(AgentUpgradeState state) => state switch
    {
        AgentUpgradeState.SkippedInactive => "Skipped (Inactive)",
        AgentUpgradeState.SkippedOptOut => "Skipped (Opt-out)",
        _ => state.ToString(),
    };
}
