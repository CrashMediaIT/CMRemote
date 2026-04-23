using Microsoft.AspNetCore.Components;
using Remotely.Server.Hubs;
using Remotely.Server.Models.Messages;
using Remotely.Server.Services;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Models;

namespace Remotely.Server.Components.Pages.PackageManager;

public partial class DeviceInstalledApps : AuthComponentBase
{
    private static readonly InstalledApplicationSource[] _allSourceFilters =
    {
        // "All" is represented as InstalledApplicationSource.Unknown on
        // the toggle (zero sentinel value).
        InstalledApplicationSource.Unknown,
        InstalledApplicationSource.Win32,
        InstalledApplicationSource.Msi,
        InstalledApplicationSource.Appx,
    };

    private Device? _device;
    private bool _isLoading = true;
    private bool _isWindows;
    private bool _isOnline;
    private bool _isRefreshing;
    private bool _showSystemComponents;
    private string _searchTerm = string.Empty;
    private InstalledApplicationSource _activeSource = InstalledApplicationSource.Unknown;
    private DateTimeOffset? _lastFetchedAt;
    private IReadOnlyList<InstalledApplication> _allApps = Array.Empty<InstalledApplication>();
    private string? _pendingUninstallKey;

    private IReadOnlyList<InstalledApplicationSource> _availableSources = _allSourceFilters;

    [Parameter]
    public string DeviceId { get; set; } = string.Empty;

    [Inject]
    private ICircuitConnection CircuitConnection { get; set; } = null!;

    [Inject]
    private IDataService DataService { get; set; } = null!;

    [Inject]
    private IInstalledApplicationsService InstalledApps { get; set; } = null!;

    [Inject]
    private IAgentHubSessionCache AgentSessions { get; set; } = null!;

    [Inject]
    private IToastService ToastService { get; set; } = null!;

    private List<InstalledApplication> _visibleApps = new();

    protected override async Task OnInitializedAsync()
    {
        await base.OnInitializedAsync();
        EnsureUserSet();

        if (string.IsNullOrWhiteSpace(DeviceId))
        {
            _isLoading = false;
            return;
        }

        var deviceResult = await DataService.GetDevice(DeviceId);
        if (deviceResult.IsSuccess && DataService.DoesUserHaveAccessToDevice(deviceResult.Value.ID, User))
        {
            _device = deviceResult.Value;
            _isWindows = _device.Platform == "Windows";
            _isOnline = AgentSessions.TryGetConnectionId(_device.ID, out _);
        }

        await LoadCachedSnapshotAsync();
        await Register<InstalledApplicationsResultMessage>(HandleInventoryResult);
        await Register<UninstallApplicationResultMessage>(HandleUninstallResult);

        _isLoading = false;
    }

    private async Task LoadCachedSnapshotAsync()
    {
        if (_device is null)
        {
            return;
        }
        var snapshot = await InstalledApps.GetSnapshotAsync(_device.ID);
        if (snapshot is { } s)
        {
            _allApps = s.Applications;
            _lastFetchedAt = s.FetchedAt;
            UpdateVisibleApps();
        }
    }

    private async Task RefreshAsync()
    {
        if (_device is null || _isRefreshing)
        {
            return;
        }
        _isRefreshing = true;
        var requestId = await CircuitConnection.RequestInstalledApplications(_device.ID);
        if (string.IsNullOrEmpty(requestId))
        {
            _isRefreshing = false;
            ToastService.ShowToast2(
                "Could not request inventory. Device may be offline.",
                Enums.ToastType.Warning);
        }
        // Otherwise wait for HandleInventoryResult to fire.
    }

    private async Task UninstallAsync(InstalledApplication app)
    {
        if (_device is null || string.IsNullOrEmpty(app.ApplicationKey))
        {
            return;
        }
        if (!app.CanUninstallSilently)
        {
            ToastService.ShowToast2(
                $"\"{app.Name}\" does not advertise a silent uninstall command and cannot be removed from this UI.",
                Enums.ToastType.Warning);
            return;
        }

        _pendingUninstallKey = app.ApplicationKey;
        await InvokeAsync(StateHasChanged);

        var result = await CircuitConnection.UninstallApplication(_device.ID, app.ApplicationKey);
        if (!result.IsSuccess)
        {
            _pendingUninstallKey = null;
            ToastService.ShowToast2(result.Reason, Enums.ToastType.Warning);
            await InvokeAsync(StateHasChanged);
        }
        // Otherwise wait for HandleUninstallResult to fire.
    }

    private async Task HandleInventoryResult(object subscriber, InstalledApplicationsResultMessage message)
    {
        if (_device is null || !string.Equals(message.DeviceId, _device.ID, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        _isRefreshing = false;

        if (message.Result.Success)
        {
            await LoadCachedSnapshotAsync();
            ToastService.ShowToast2("Inventory refreshed.", Enums.ToastType.Success);
        }
        else
        {
            ToastService.ShowToast2(
                $"Agent could not enumerate installed applications: {message.Result.ErrorMessage}",
                Enums.ToastType.Warning);
        }

        await InvokeAsync(StateHasChanged);
    }

    private async Task HandleUninstallResult(object subscriber, UninstallApplicationResultMessage message)
    {
        if (_device is null || !string.Equals(message.DeviceId, _device.ID, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        _pendingUninstallKey = null;

        if (message.Result.Success)
        {
            ToastService.ShowToast2(
                $"Uninstall succeeded (exit code {message.Result.ExitCode}).",
                Enums.ToastType.Success);
            // Re-enumerate so the UI reflects the new state.
            await RefreshAsync();
        }
        else
        {
            var reason = message.Result.ErrorMessage ?? $"Exit code {message.Result.ExitCode}.";
            ToastService.ShowToast2($"Uninstall failed: {reason}", Enums.ToastType.Warning);
        }

        await InvokeAsync(StateHasChanged);
    }

    private void UpdateVisibleApps()
    {
        IEnumerable<InstalledApplication> query = _allApps;

        if (!_showSystemComponents)
        {
            query = query.Where(a => !a.IsSystemComponent);
        }

        if (_activeSource != InstalledApplicationSource.Unknown)
        {
            query = query.Where(a => a.Source == _activeSource);
        }

        if (!string.IsNullOrWhiteSpace(_searchTerm))
        {
            var term = _searchTerm.Trim();
            query = query.Where(a =>
                (a.Name?.Contains(term, StringComparison.OrdinalIgnoreCase) ?? false) ||
                (a.Publisher?.Contains(term, StringComparison.OrdinalIgnoreCase) ?? false));
        }

        _visibleApps = query.ToList();
    }

    protected override bool ShouldRender()
    {
        // Recompute visibility on every render (cheap; the list is at
        // most a few hundred items and Razor only re-renders on event).
        UpdateVisibleApps();
        return true;
    }

    private static string SourceLabel(InstalledApplicationSource src) => src switch
    {
        InstalledApplicationSource.Unknown => "All",
        InstalledApplicationSource.Win32 => "Win32",
        InstalledApplicationSource.Msi => "MSI",
        InstalledApplicationSource.Appx => "AppX / MSIX",
        _ => src.ToString(),
    };

    private static string FormatSize(long? bytes)
    {
        if (bytes is null or <= 0)
        {
            return string.Empty;
        }
        double value = bytes.Value;
        string[] units = { "B", "KB", "MB", "GB", "TB" };
        var unit = 0;
        while (value >= 1024 && unit < units.Length - 1)
        {
            value /= 1024;
            unit++;
        }
        return $"{value:0.##} {units[unit]}";
    }
}
