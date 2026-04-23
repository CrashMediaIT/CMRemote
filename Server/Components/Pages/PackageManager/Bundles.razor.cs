using Microsoft.AspNetCore.Components;
using Remotely.Server.Services;
using Remotely.Shared.Entities;

namespace Remotely.Server.Components.Pages.PackageManager;

public partial class Bundles : AuthComponentBase
{
    private IReadOnlyList<DeploymentBundle> _bundles = Array.Empty<DeploymentBundle>();
    private IReadOnlyList<Package> _availablePackages = Array.Empty<Package>();

    private readonly Dictionary<Guid, string> _addItemSelections = new();
    private readonly Dictionary<Guid, int> _addItemOrders = new();
    private readonly Dictionary<Guid, bool> _addItemContinue = new();

    private string _newName = string.Empty;
    private string _newDescription = string.Empty;
    private string? _createError;
    private bool _isWorking;

    [Inject]
    public required IPackageService PackageService { get; init; }

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
        if (User is null || string.IsNullOrEmpty(User.OrganizationID))
        {
            return;
        }
        _bundles = await PackageService.GetBundlesForOrg(User.OrganizationID);
        _availablePackages = await PackageService.GetPackagesForOrg(User.OrganizationID);

        // Seed per-bundle add-item form state so two-way binding has a
        // stable backing entry — Razor's @bind requires the dictionary
        // key to already exist.
        foreach (var bundle in _bundles)
        {
            _addItemSelections.TryAdd(bundle.Id, string.Empty);
            _addItemOrders.TryAdd(bundle.Id, bundle.Items.Count);
            _addItemContinue.TryAdd(bundle.Id, false);
        }
    }

    private async Task HandleCreateBundle()
    {
        _createError = null;
        if (User is null) return;
        _isWorking = true;
        try
        {
            var result = await PackageService.CreateBundle(
                User.OrganizationID, User.Id, _newName, _newDescription);
            if (!result.IsSuccess)
            {
                _createError = result.Reason;
                return;
            }
            _newName = string.Empty;
            _newDescription = string.Empty;
            await ReloadAsync();
            ToastService.ShowToast("Bundle created.");
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleDeleteBundle(Guid bundleId)
    {
        if (User is null) return;
        _isWorking = true;
        try
        {
            var result = await PackageService.DeleteBundle(User.OrganizationID, bundleId);
            if (!result.IsSuccess)
            {
                ToastService.ShowToast(result.Reason, classString: "bg-warning");
                return;
            }
            await ReloadAsync();
            ToastService.ShowToast("Bundle deleted.");
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleAddItem(Guid bundleId)
    {
        if (User is null) return;
        if (!_addItemSelections.TryGetValue(bundleId, out var selection) ||
            !Guid.TryParse(selection, out var packageId))
        {
            return;
        }
        _addItemOrders.TryGetValue(bundleId, out var order);
        _addItemContinue.TryGetValue(bundleId, out var continueOnFailure);

        _isWorking = true;
        try
        {
            var result = await PackageService.AddBundleItem(
                User.OrganizationID, bundleId, packageId, order, continueOnFailure);
            if (!result.IsSuccess)
            {
                ToastService.ShowToast(result.Reason, classString: "bg-warning");
                return;
            }
            _addItemSelections[bundleId] = string.Empty;
            await ReloadAsync();
        }
        finally
        {
            _isWorking = false;
        }
    }

    private async Task HandleRemoveItem(Guid bundleId, Guid itemId)
    {
        if (User is null) return;
        _isWorking = true;
        try
        {
            var result = await PackageService.RemoveBundleItem(User.OrganizationID, bundleId, itemId);
            if (!result.IsSuccess)
            {
                ToastService.ShowToast(result.Reason, classString: "bg-warning");
                return;
            }
            await ReloadAsync();
        }
        finally
        {
            _isWorking = false;
        }
    }
}
