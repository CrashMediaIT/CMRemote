using Microsoft.Extensions.Logging;
using Remotely.Agent.Interfaces;
using Remotely.Shared.Dtos;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Services;

/// <summary>
/// Marker interface for the agent's single, composite
/// <see cref="IPackageProvider"/> entry point. Lets the hub resolve
/// "the router" without confusing it with the per-provider
/// implementations also registered as <see cref="IPackageProvider"/>.
/// </summary>
public interface ICompositePackageProvider : IPackageProvider
{
}

/// <summary>
/// Fans an incoming <see cref="PackageInstallRequestDto"/> out to the
/// first registered <see cref="IPackageProvider"/> whose
/// <see cref="IPackageProvider.CanHandle"/> returns true. This is the
/// single <c>IPackageProvider</c> the
/// <see cref="AgentHubConnection"/> depends on, so adding a new
/// provider (Executable in PR&nbsp;C2, etc.) is a one-line DI change
/// here rather than a touch on the hub.
///
/// <para>Order matters only as a tie-breaker — providers self-gate via
/// <c>CanHandle</c>, so registering Chocolatey before MsiPackageInstaller
/// has no effect on which one services a request.</para>
/// </summary>
public sealed class CompositePackageProvider : ICompositePackageProvider
{
    private readonly IReadOnlyList<IPackageProvider> _providers;
    private readonly ILogger<CompositePackageProvider> _logger;

    public CompositePackageProvider(
        IEnumerable<IPackageProvider> providers,
        ILogger<CompositePackageProvider> logger)
    {
        // Filter out self in case DI is misconfigured and the composite
        // somehow ends up listed among its own children — would
        // otherwise be an infinite recursion.
        _providers = providers.Where(p => p is not ICompositePackageProvider).ToArray();
        _logger = logger;
    }

    public bool CanHandle(PackageInstallRequestDto request)
    {
        foreach (var provider in _providers)
        {
            if (provider.CanHandle(request))
            {
                return true;
            }
        }
        return false;
    }

    public async Task<PackageInstallResultDto> ExecuteAsync(
        PackageInstallRequestDto request,
        CancellationToken cancellationToken)
    {
        if (request is null)
        {
            return new PackageInstallResultDto
            {
                Success = false,
                ExitCode = -1,
                ErrorMessage = "Request is required.",
            };
        }
        foreach (var provider in _providers)
        {
            if (provider.CanHandle(request))
            {
                _logger.LogDebug(
                    "Routing package job {jobId} (Provider={provider}) to {handler}.",
                    request.JobId, request.Provider, provider.GetType().Name);
                return await provider.ExecuteAsync(request, cancellationToken).ConfigureAwait(false);
            }
        }
        return new PackageInstallResultDto
        {
            JobId = request.JobId,
            Success = false,
            ExitCode = -1,
            ErrorMessage = $"No agent-side provider can handle '{request.Provider}'.",
        };
    }
}
