using Microsoft.AspNetCore.Components;
using Remotely.Server.PackageManager;
using Remotely.Server.Services;
using Remotely.Server.Services.Devices;
using Remotely.Shared.Entities;

namespace Remotely.Server.Components.Pages.PackageManager;

public partial class Devices : AuthComponentBase
{
    private IReadOnlyList<Device> _devices = Array.Empty<Device>();

    [Inject]
    public required IDataService DataService { get; init; }

    [Inject]
    public required IDeviceQueryService DeviceQueryService { get; init; }

    protected override async Task OnInitializedAsync()
    {
        await base.OnInitializedAsync();
        EnsureUserSet();
        _devices = PackageManagerDeviceFilter
            .SupportedDevices(DeviceQueryService.GetDevicesForUser(UserName))
            .OrderBy(d => d.DeviceName)
            .ToArray();
    }
}
