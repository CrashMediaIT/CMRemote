using Microsoft.AspNetCore.Components;
using Remotely.Server.Hubs;
using Remotely.Server.PackageManager;
using Remotely.Server.Services;
using Remotely.Server.Services.Devices;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;

namespace Remotely.Server.Components.Pages.PackageManager;

public partial class InstallPackages : AuthComponentBase
{
    private readonly Package _newPackage = NewBlankPackage();

    private IReadOnlyList<Package> _packages = Array.Empty<Package>();
    private IReadOnlyList<Device> _devices = Array.Empty<Device>();
    private string _dispatchPackageId = string.Empty;
    private string _dispatchDeviceId = string.Empty;
    private string? _dispatchMessage;
    private string _dispatchMessageClass = "info";
    private string? _createError;
    private bool _isWorking;

    [Inject]
    public required IPackageService PackageService { get; init; }

    [Inject]
    public required IDataService DataService { get; init; }

    [Inject]
    public required IDeviceQueryService DeviceQueryService { get; init; }

    [Inject]
    public required ICircuitConnection CircuitConnection { get; init; }

    [Inject]
    public required IToastService ToastService { get; init; }

    protected override async Task OnInitializedAsync()
    {
        await base.OnInitializedAsync();
        EnsureUserSet();
        await ReloadAsync();
    }

    private async Task ReloadAsync()
    {
        if (User is null || string.IsNullOrEmpty(User.OrganizationID) || UserName is null)
        {
            return;
        }

        _packages = await PackageService.GetPackagesForOrg(User.OrganizationID);

        // Restrict the device picker to Windows devices the caller can
        // access — Phase 2 only ships a Chocolatey provider, so a
        // non-Windows target would be guaranteed to fail.
        _devices = PackageManagerDeviceFilter
            .SupportedDevices(DeviceQueryService.GetDevicesForUser(UserName))
            .OrderBy(d => d.DeviceName)
            .ToArray();
    }

    private async Task HandleCreatePackage()
    {
        _createError = null;
        if (User is null) return;
        _isWorking = true;
        try
        {
            // The form binds Provider via the (always-Chocolatey for Phase 2)
            // default — we override here defensively in case the page is
            // ever extended to expose other providers in the form.
            _newPackage.Provider = PackageProvider.Chocolatey;
            var result = await PackageService.CreatePackage(User.OrganizationID, User.Id, _newPackage);
            if (!result.IsSuccess)
            {
                _createError = result.Reason;
                return;
            }
            ResetForm();
            await ReloadAsync();
            ToastService.ShowToast("Package added.");
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleDeletePackage(Guid packageId)
    {
        if (User is null) return;
        _isWorking = true;
        try
        {
            var result = await PackageService.DeletePackage(User.OrganizationID, packageId);
            if (!result.IsSuccess)
            {
                ToastService.ShowToast(result.Reason, classString: "bg-warning");
                return;
            }
            await ReloadAsync();
            ToastService.ShowToast("Package deleted.");
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleDispatch(PackageInstallAction action)
    {
        _dispatchMessage = null;
        if (User is null) return;
        if (!Guid.TryParse(_dispatchPackageId, out var packageId) ||
            string.IsNullOrEmpty(_dispatchDeviceId))
        {
            return;
        }

        _isWorking = true;
        try
        {
            var result = await CircuitConnection.QueueInstallPackage(_dispatchDeviceId, packageId, action);
            if (!result.IsSuccess)
            {
                _dispatchMessage = result.Reason;
                _dispatchMessageClass = "danger";
                return;
            }
            _dispatchMessage =
                $"Job queued ({action}). Job ID: {result.Value:D}. Track progress under Job Status.";
            _dispatchMessageClass = "success";
        }
        finally
        {
            _isWorking = false;
        }
    }

    private void ResetForm()
    {
        _newPackage.Name = string.Empty;
        _newPackage.PackageIdentifier = string.Empty;
        _newPackage.Version = null;
        _newPackage.InstallArguments = null;
        _newPackage.Description = null;
        _createError = null;
    }

    private static Package NewBlankPackage() => new()
    {
        Id = Guid.NewGuid(),
        Name = string.Empty,
        Provider = PackageProvider.Chocolatey,
        PackageIdentifier = string.Empty,
    };
}
