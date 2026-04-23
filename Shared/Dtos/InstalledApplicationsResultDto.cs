using Remotely.Shared.Models;

namespace Remotely.Shared.Dtos;

/// <summary>
/// Payload sent from agent → server in response to
/// <c>RequestInstalledApplications</c>. Carries the full inventory so the
/// server can replace its single-row cache for the device.
/// </summary>
public class InstalledApplicationsResultDto
{
    public string RequestId { get; set; } = string.Empty;
    public bool Success { get; set; }
    public string? ErrorMessage { get; set; }
    public IReadOnlyList<InstalledApplication> Applications { get; set; } = [];
}
