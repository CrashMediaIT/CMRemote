using Remotely.Shared.Dtos;

namespace Remotely.Server.Models.Messages;

/// <summary>
/// Sent on the messenger bus when an agent reports the terminal result
/// of a package install job. Subscribers (the Package Manager status
/// page, per-device packages page) refresh in response.
/// </summary>
public record PackageInstallResultMessage(string DeviceId, PackageInstallResultDto Result);
