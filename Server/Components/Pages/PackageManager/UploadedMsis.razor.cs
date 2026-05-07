using Microsoft.AspNetCore.Components.Forms;
using Remotely.Server.Hubs;
using Remotely.Server.PackageManager;
using Remotely.Server.Services;
using Remotely.Server.Services.Devices;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.PackageManager;

namespace Remotely.Server.Components.Pages.PackageManager;

public partial class UploadedMsis : AuthComponentBase
{
    private const long MaxSizeMb = MsiFileValidator.MaxMsiSizeBytes / (1024 * 1024);

    private IReadOnlyList<UploadedMsi> _msis = Array.Empty<UploadedMsi>();
    private IReadOnlyList<Device> _devices = Array.Empty<Device>();

    private string _uploadName = string.Empty;
    private string? _uploadDescription;
    private string? _uploadProgress;
    private string? _uploadError;

    private string _dispatchDeviceId = string.Empty;
    private string? _dispatchMessage;
    private string _dispatchMessageClass = "info";

    private bool _isWorking;

    [Microsoft.AspNetCore.Components.Inject]
    public required IUploadedMsiService MsiService { get; init; }

    [Microsoft.AspNetCore.Components.Inject]
    public required IPackageService PackageService { get; init; }

    [Microsoft.AspNetCore.Components.Inject]
    public required IDataService DataService { get; init; }

    [Microsoft.AspNetCore.Components.Inject]
    public required IDeviceQueryService DeviceQueryService { get; init; }

    [Microsoft.AspNetCore.Components.Inject]
    public required ICircuitConnection CircuitConnection { get; init; }

    [Microsoft.AspNetCore.Components.Inject]
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
        _msis = await MsiService.GetForOrgAsync(User.OrganizationID);
        _devices = PackageManagerDeviceFilter
            .SupportedDevices(DeviceQueryService.GetDevicesForUser(UserName))
            .OrderBy(d => d.DeviceName)
            .ToArray();
    }

    private async Task HandleFileSelected(InputFileChangeEventArgs args)
    {
        _uploadError = null;
        _uploadProgress = null;

        if (User is null)
        {
            return;
        }
        if (string.IsNullOrWhiteSpace(_uploadName))
        {
            _uploadError = "Display name is required before uploading.";
            return;
        }

        var file = args.File;
        if (file is null)
        {
            return;
        }
        if (file.Size > MsiFileValidator.MaxMsiSizeBytes)
        {
            _uploadError = $"File exceeds the maximum allowed size of {MaxSizeMb} MiB.";
            return;
        }

        _isWorking = true;
        _uploadProgress = $"Uploading {file.Name} ({FormatBytes(file.Size)})…";
        try
        {
            var result = await MsiService.UploadAsync(
                User.OrganizationID,
                User.Id,
                _uploadName,
                file,
                _uploadDescription);

            if (!result.IsSuccess)
            {
                _uploadError = result.Message;
                _uploadProgress = null;
                return;
            }
            _uploadProgress = null;
            _uploadName = string.Empty;
            _uploadDescription = null;
            await ReloadAsync();
            ToastService.ShowToast("MSI uploaded.");
        }
        catch (Exception ex)
        {
            _uploadError = $"Upload failed: {ex.Message}";
            _uploadProgress = null;
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleDelete(Guid msiId)
    {
        if (User is null) return;
        _isWorking = true;
        try
        {
            var ok = await MsiService.TombstoneAsync(User.OrganizationID, msiId);
            if (!ok)
            {
                ToastService.ShowToast("Could not delete MSI.", classString: "bg-warning");
                return;
            }
            await ReloadAsync();
            ToastService.ShowToast("MSI deleted (will be purged once any in-flight jobs finish).");
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task<Package?> EnsurePackageForMsi(UploadedMsi msi)
    {
        if (User is null) return null;

        // Re-use an existing Package row pointing at the same MSI id so
        // we don't create a fresh one on every Register/Send click.
        var idAsString = msi.Id.ToString("D");
        var packages = await PackageService.GetPackagesForOrg(User.OrganizationID);
        var existing = packages.FirstOrDefault(p =>
            p.Provider == PackageProvider.UploadedMsi &&
            p.PackageIdentifier == idAsString);
        if (existing is not null)
        {
            return existing;
        }

        var created = await PackageService.CreatePackage(User.OrganizationID, User.Id, new Package
        {
            Name = msi.Name,
            Provider = PackageProvider.UploadedMsi,
            PackageIdentifier = idAsString,
            Description = msi.Description,
        });
        if (!created.IsSuccess)
        {
            ToastService.ShowToast(created.Reason, classString: "bg-warning");
            return null;
        }
        return created.Value;
    }

    private async Task HandleRegisterAsPackage(UploadedMsi msi)
    {
        if (User is null) return;
        _isWorking = true;
        try
        {
            var pkg = await EnsurePackageForMsi(msi);
            if (pkg is not null)
            {
                ToastService.ShowToast($"Registered as Package '{pkg.Name}'.");
            }
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleSendToDevice(UploadedMsi msi)
    {
        _dispatchMessage = null;
        if (User is null || string.IsNullOrEmpty(_dispatchDeviceId)) return;

        _isWorking = true;
        try
        {
            var pkg = await EnsurePackageForMsi(msi);
            if (pkg is null)
            {
                return;
            }
            var result = await CircuitConnection.QueueInstallPackage(
                _dispatchDeviceId, pkg.Id, PackageInstallAction.Install);
            if (!result.IsSuccess)
            {
                _dispatchMessage = result.Reason;
                _dispatchMessageClass = "danger";
                return;
            }
            _dispatchMessage =
                $"Job queued. Job ID: {result.Value:D}. Track progress under Job Status.";
            _dispatchMessageClass = "success";
        }
        finally
        {
            _isWorking = false;
        }
    }

    private static string FormatBytes(long bytes)
    {
        if (bytes < 1024) return $"{bytes} B";
        if (bytes < 1024L * 1024) return $"{bytes / 1024.0:0.#} KiB";
        if (bytes < 1024L * 1024 * 1024) return $"{bytes / (1024.0 * 1024):0.#} MiB";
        return $"{bytes / (1024.0 * 1024 * 1024):0.##} GiB";
    }
}
