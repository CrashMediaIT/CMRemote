using Remotely.Shared.Dtos;

namespace Remotely.Server.Models.Messages;

/// <summary>
/// Sent on the <see cref="Bitbound.SimpleMessenger.IMessenger"/> bus when
/// an agent returns a fresh installed-applications inventory. Subscribers
/// (the per-device package page) use this to refresh their UI without
/// polling.
/// </summary>
public record InstalledApplicationsResultMessage(string DeviceId, InstalledApplicationsResultDto Result);

/// <summary>
/// Sent when an agent reports the outcome of an uninstall request.
/// </summary>
public record UninstallApplicationResultMessage(string DeviceId, UninstallApplicationResultDto Result);
