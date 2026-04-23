using Remotely.Agent.Interfaces;
using Remotely.Shared.Dtos;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Services;

/// <summary>
/// Used on platforms where no package provider is available (Linux,
/// macOS, or a Windows host without Chocolatey). Returns a clear
/// error so the WebUI can surface "this device cannot run the chosen
/// provider" instead of timing out.
/// </summary>
internal sealed class NotSupportedPackageProvider : IPackageProvider
{
    public bool CanHandle(PackageInstallRequestDto request) => false;

    public Task<PackageInstallResultDto> ExecuteAsync(PackageInstallRequestDto request, CancellationToken cancellationToken)
    {
        return Task.FromResult(new PackageInstallResultDto
        {
            JobId = request?.JobId ?? string.Empty,
            ExitCode = -1,
            Success = false,
            ErrorMessage = "No package provider is available on this device for the requested package type.",
        });
    }
}
