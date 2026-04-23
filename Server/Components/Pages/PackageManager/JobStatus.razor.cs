using Bitbound.SimpleMessenger;
using Microsoft.AspNetCore.Components;
using Remotely.Server.Models.Messages;
using Remotely.Server.Services;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;

namespace Remotely.Server.Components.Pages.PackageManager;

public partial class JobStatus : AuthComponentBase
{
    private IReadOnlyList<PackageInstallJob> _jobs = Array.Empty<PackageInstallJob>();

    [Inject]
    public required IPackageInstallJobService JobService { get; init; }

    protected override async Task OnInitializedAsync()
    {
        await base.OnInitializedAsync();
        EnsureUserSet();
        // Live-refresh on agent-reported result. Subscriber pattern
        // mirrors the per-device installed-apps page; the base
        // MessengerSubscriber owns subscription disposal.
        await Register<PackageInstallResultMessage>(async (_, _) =>
        {
            await ReloadAsync();
            await InvokeAsync(StateHasChanged);
        });
        await ReloadAsync();
    }

    private async Task ReloadAsync()
    {
        if (User is null || string.IsNullOrEmpty(User.OrganizationID))
        {
            return;
        }
        _jobs = await JobService.GetRecentJobsForOrgAsync(User.OrganizationID, 100);
    }

    private static string StatusBadge(PackageInstallJobStatus status) => status switch
    {
        PackageInstallJobStatus.Queued => "bg-secondary",
        PackageInstallJobStatus.Running => "bg-info text-dark",
        PackageInstallJobStatus.Success => "bg-success",
        PackageInstallJobStatus.Failed => "bg-danger",
        PackageInstallJobStatus.Cancelled => "bg-warning text-dark",
        _ => "bg-secondary",
    };
}
