using Remotely.Server.Services;
using Remotely.Server.Services.Devices;
using Bitbound.SimpleMessenger;
using Microsoft.AspNetCore.SignalR;
using Microsoft.Extensions.Caching.Memory;
using Remotely.Server.Models.Messages;
using Remotely.Shared;
using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Interfaces;
using Remotely.Shared.Models;
using Remotely.Shared.Services;
using Remotely.Shared.Utilities;

namespace Remotely.Server.Hubs;

public class AgentHub : Hub<IAgentHubClient>
{
    private readonly IDataService _dataService;
    private readonly IDeviceQueryService _deviceQueryService;
    private readonly IDeviceCommandService _deviceCommandService;
    private readonly IInstalledApplicationsService _installedApplicationsService;
    private readonly IPackageInstallJobService _packageInstallJobService;
    private readonly ICircuitManager _circuitManager;
    private readonly IExpiringTokenService _expiringTokenService;
    private readonly ILogger<AgentHub> _logger;
    private readonly IMessenger _messenger;
    private readonly IRemoteControlSessionCache _remoteControlSessions;
    private readonly IAgentHubSessionCache _serviceSessionCache;
    private readonly Remotely.Server.Services.AgentUpgrade.IAgentUpgradeService _agentUpgradeService;
    private readonly ISystemTime _systemTime;
    private readonly IHubContext<ViewerHub> _viewerHubContext;

    public AgentHub(
        IDataService dataService,
        IDeviceQueryService deviceQueryService,
        IDeviceCommandService deviceCommandService,
        IAgentHubSessionCache serviceSessionCache,
        IHubContext<ViewerHub> viewerHubContext,
        ICircuitManager circuitManager,
        IExpiringTokenService expiringTokenService,
        IRemoteControlSessionCache remoteControlSessionCache,
        IMessenger messenger,
        IInstalledApplicationsService installedApplicationsService,
        IPackageInstallJobService packageInstallJobService,
        Remotely.Server.Services.AgentUpgrade.IAgentUpgradeService agentUpgradeService,
        ISystemTime systemTime,
        ILogger<AgentHub> logger)
    {
        _dataService = dataService;
        _deviceQueryService = deviceQueryService;
        _deviceCommandService = deviceCommandService;
        _serviceSessionCache = serviceSessionCache;
        _viewerHubContext = viewerHubContext;
        _circuitManager = circuitManager;
        _expiringTokenService = expiringTokenService;
        _remoteControlSessions = remoteControlSessionCache;
        _messenger = messenger;
        _installedApplicationsService = installedApplicationsService;
        _packageInstallJobService = packageInstallJobService;
        _agentUpgradeService = agentUpgradeService;
        _systemTime = systemTime;
        _logger = logger;
    }

    // TODO: Replace with new invoke capability in .NET 7 in ScriptingController.
    public static IMemoryCache ApiScriptResults { get; } = new MemoryCache(new MemoryCacheOptions());

    private Device? Device
    {
        get
        {
            if (Context.Items["Device"] is Device device)
            {
                return device;
            }
            _logger.LogWarning("Device has not been set in the context items.");
            return null;
        }
        set
        {
            Context.Items["Device"] = value;
        }
    }

    public async Task Chat(string messageText, bool disconnected, string browserConnectionId)
    {
        if (Device is null)
        {
            return;
        }

        if (_circuitManager.TryGetConnection(browserConnectionId, out _))
        {
            var message = new ChatReceivedMessage(Device.ID, $"{Device.DeviceName}", messageText, disconnected);
            await _messenger.Send(message, browserConnectionId);
        }
        else
        {
            await Clients.Caller.SendChatMessage(
                senderName: string.Empty,
                message: string.Empty,
                orgName: string.Empty,
                orgId: string.Empty,
                disconnected: true,
                senderConnectionId: browserConnectionId);
        }
    }

    public async Task CheckForPendingRemoteControlSessions()
    {
        try
        {
            if (Device is null)
            {
                return;
            }

            _logger.LogDebug(
                "Checking for pending remote control sessions for device {deviceId}.",
                Device.ID);

            var waitingSessions = _remoteControlSessions
                .Sessions
                .Where(x => x.DeviceId == Device.ID);

            foreach (var session in waitingSessions)
            {
                _logger.LogDebug(
                    "Restarting remote control session {sessionId}.",
                    session.UnattendedSessionId);

                session.AgentConnectionId = Context.ConnectionId;
                await Clients.Caller.RestartScreenCaster(
                    session.ViewerList.ToArray(),
                    $"{session.UnattendedSessionId}",
                    session.AccessKey,
                    session.UserConnectionId,
                    session.RequesterName,
                    session.OrganizationName,
                    session.OrganizationId);
            }
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while checking for pending remote control sessions.");
        }
    }

    public async Task CheckForPendingScriptRuns()
    {
        if (Device is null)
        {
            return;
        }

        var authToken = _expiringTokenService.GetToken(Time.Now.AddMinutes(AppConstants.ScriptRunExpirationMinutes));
        var scriptRuns = await _dataService.GetPendingScriptRuns(Device.ID);

        foreach (var run in scriptRuns)
        {
            if (run.SavedScriptId is null)
            {
                continue;
            }
            await Clients.Caller.RunScript(
                run.SavedScriptId.Value,
                run.Id,
                run.Initiator ?? "Unknown Initiator",
                run.InputType,
                authToken);
        }
    }

    public async Task<bool> DeviceCameOnline(DeviceClientDto device)
    {
        try
        {
            if (await CheckForDeviceBan(device.ID, device.DeviceName))
            {
                return false;
            }

            var ip = Context.GetHttpContext()?.Connection?.RemoteIpAddress;
            if (ip != null && ip.IsIPv4MappedToIPv6)
            {
                ip = ip.MapToIPv4();
            }
            device.PublicIP = $"{ip}";

            if (await CheckForDeviceBan(device.PublicIP))
            {
                return false;
            }

            var result = await _deviceCommandService.AddOrUpdateDevice(device);
            if (!result.IsSuccess)
            {
                // Organization wasn't found.
                return false;
            }

            Device = result.Value;

            _serviceSessionCache.AddOrUpdateByConnectionId(Context.ConnectionId, Device);

            // ROADMAP.md "M3 — Background agent-upgrade pipeline":
            // on-connect path. Enrol the device into the pipeline if it
            // isn't already, then flip a SkippedInactive row back to
            // Pending so the orchestrator's next sweep dispatches the
            // upgrade. We never block the connection on this — failures
            // are swallowed + logged, the orchestrator will pick the
            // device up on its own cadence.
            try
            {
                await _agentUpgradeService.EnrolDeviceAsync(
                    Device.OrganizationID,
                    Device.ID,
                    Device.AgentVersion,
                    Device.LastOnline,
                    targetVersion: null);
                await _agentUpgradeService.MarkDeviceCameOnlineAsync(Device.ID);
            }
            catch (Exception upgradeEx)
            {
                _logger.LogWarning(upgradeEx,
                    "Failed to update agent-upgrade row on connect for {deviceId}.", Device.ID);
            }

            var userIDs = _circuitManager.Connections.Select(x => x.User.Id);

            var filteredUserIDs = _deviceQueryService.FilterUsersByDevicePermission(userIDs, Device.ID);

            var connections = _circuitManager.Connections
                .Where(x => x.User.OrganizationID == Device.OrganizationID &&
                    filteredUserIDs.Contains(x.User.Id));

            foreach (var connection in connections)
            {
                var message = new DeviceStateChangedMessage(Device);
                await _messenger.Send(message, connection.ConnectionId);
            }

            return true;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while setting device to online status.");
        }

        Context.Abort();
        return false;
    }

    public async Task DeviceHeartbeat(DeviceClientDto device)
    {
        if (await CheckForDeviceBan(device.ID, device.DeviceName))
        {
            return;
        }

        var ip = Context.GetHttpContext()?.Connection?.RemoteIpAddress;
        if (ip != null && ip.IsIPv4MappedToIPv6)
        {
            ip = ip.MapToIPv4();
        }
        device.PublicIP = $"{ip}";

        if (await CheckForDeviceBan(device.PublicIP))
        {
            return;
        }


        var result = await _deviceCommandService.AddOrUpdateDevice(device);

        if (!result.IsSuccess)
        {
            return;
        }

        Device = result.Value;

        _serviceSessionCache.AddOrUpdateByConnectionId(Context.ConnectionId, Device);

        var userIDs = _circuitManager.Connections.Select(x => x.User.Id);

        var filteredUserIDs = _deviceQueryService.FilterUsersByDevicePermission(userIDs, Device.ID);

        var connections = _circuitManager.Connections
            .Where(x => x.User.OrganizationID == Device.OrganizationID &&
                filteredUserIDs.Contains(x.User.Id));

        foreach (var connection in connections)
        {
            var message = new DeviceStateChangedMessage(Device);
            await _messenger.Send(message, connection.ConnectionId);
        }


        await CheckForPendingScriptRuns();
    }

    public Task DisplayMessage(string consoleMessage, string popupMessage, string className, string requesterId)
    {
        var message = new DisplayNotificationMessage(consoleMessage, popupMessage, className);
        return _messenger.Send(message, requesterId);
    }

    public Task DownloadFile(string fileID, string requesterId)
    {
        var message = new DownloadFileMessage(fileID);
        return _messenger.Send(message, requesterId);
    }

    public Task DownloadFileProgress(int progressPercent, string requesterId)
    {
        var message = new DownloadFileProgressMessage(progressPercent);
        return _messenger.Send(message, requesterId);
    }

    public async Task<string> GetServerUrl()
    {
        var settings = await _dataService.GetSettings();
        return settings.ServerUrl;
    }

    public string GetServerVerificationToken()
    {
        return $"{Device?.ServerVerificationToken}";
    }

    public override async Task OnDisconnectedAsync(Exception? exception)
    {
        try
        {
            if (Device != null)
            {
                _deviceCommandService.DeviceDisconnected(Device.ID);

                Device.IsOnline = false;

                var userIDs = _circuitManager.Connections.Select(x => x.User.Id);

                var filteredUserIDs = _deviceQueryService.FilterUsersByDevicePermission(userIDs, Device.ID);

                var connections = _circuitManager.Connections
                    .Where(x => x.User.OrganizationID == Device.OrganizationID &&
                        filteredUserIDs.Contains(x.User.Id));

                foreach (var connection in connections)
                {
                    var message = new DeviceStateChangedMessage(Device);
                    await _messenger.Send(message, connection.ConnectionId);
                }
            }
            await base.OnDisconnectedAsync(exception);
        }
        finally
        {
            _serviceSessionCache.TryRemoveByConnectionId(Context.ConnectionId, out _);
        }
    }

    public Task ReturnPowerShellCompletions(PwshCommandCompletion completion, CompletionIntent intent, string senderConnectionId)
    {
        var message = new PowerShellCompletionsMessage(completion, intent);
        return _messenger.Send(message, senderConnectionId);
    }

    public async Task ScriptResult(string scriptResultId)
    {
        var result = await _dataService.GetScriptResult(scriptResultId);
        if (!result.IsSuccess)
        {
            return;
        }

        var message = new ScriptResultMessage(result.Value);
        await _messenger.Send(message, $"{result.Value.SenderConnectionID}");
    }

    public void ScriptResultViaApi(string commandID, string requestID)
    {
        ApiScriptResults.Set(requestID, commandID, DateTimeOffset.Now.AddHours(1));
    }

    public Task SendConnectionFailedToViewers(List<string> viewerIDs)
    {
        return _viewerHubContext.Clients.Clients(viewerIDs).SendAsync("ConnectionFailed");
    }

    /// <summary>
    /// Slice R7.n.7 — server-bound counterpart of
    /// <see cref="IAgentHubClient.SendSdpAnswer"/>. The Rust agent's
    /// WebRTC driver invokes this with a locally-produced SDP answer
    /// once it has accepted a viewer's offer; the hub validates that
    /// the answer belongs to the calling agent's own device + an
    /// expected viewer in the session's viewer list, then forwards
    /// the DTO to the viewer's SignalR circuit as a
    /// <c>ReceiveSdpAnswer</c> client method invocation.
    /// </summary>
    public async Task SendSdpAnswer(AgentSdpAnswerDto answer)
    {
        if (Device is null || answer is null)
        {
            return;
        }

        if (!TryAuthorizeViewerSignalling(
                answer.SessionId,
                answer.ViewerConnectionId,
                out var viewerConnectionId))
        {
            return;
        }

        // Reject SDP bodies that exceed the wire-side cap. Mirrors
        // `cmremote_wire::desktop::signalling::MAX_SDP_BYTES` (16 KiB)
        // — well above any legitimate browser-emitted answer and
        // well below the point where a malformed body could be used
        // as an amplification vector.
        if (answer.Sdp.Length > AgentSignallingLimits.MaxSdpBytes)
        {
            _logger.LogWarning(
                "Rejecting SendSdpAnswer from device {DeviceId}: SDP body {Length} bytes exceeds {Cap} byte cap.",
                Device.ID,
                answer.Sdp.Length,
                AgentSignallingLimits.MaxSdpBytes);
            return;
        }

        await _viewerHubContext.Clients
            .Client(viewerConnectionId)
            .SendAsync("ReceiveSdpAnswer", answer);
    }

    /// <summary>
    /// Slice R7.n.7 — server-bound counterpart of
    /// <see cref="IAgentHubClient.SendIceCandidate"/>. The Rust
    /// agent's WebRTC driver invokes this once per locally-trickled
    /// ICE candidate (and once with an empty candidate string +
    /// <c>null</c> mid / mline index for the RFC 8838
    /// end-of-candidates marker). The hub validates the same
    /// session-ownership invariants as <see cref="SendSdpAnswer"/>
    /// and forwards as a <c>ReceiveIceCandidate</c> client method
    /// invocation.
    /// </summary>
    public async Task SendIceCandidate(AgentIceCandidateDto candidate)
    {
        if (Device is null || candidate is null)
        {
            return;
        }

        if (!TryAuthorizeViewerSignalling(
                candidate.SessionId,
                candidate.ViewerConnectionId,
                out var viewerConnectionId))
        {
            return;
        }

        // Cap the candidate string at the wire-side limit. Mirrors
        // `cmremote_wire::desktop::signalling::MAX_SIGNALLING_STRING_LEN`
        // (1 KiB).
        if (candidate.Candidate.Length > AgentSignallingLimits.MaxSignallingStringLen)
        {
            _logger.LogWarning(
                "Rejecting SendIceCandidate from device {DeviceId}: candidate {Length} bytes exceeds {Cap} byte cap.",
                Device.ID,
                candidate.Candidate.Length,
                AgentSignallingLimits.MaxSignallingStringLen);
            return;
        }
        if (candidate.SdpMid is { Length: var midLen } &&
            midLen > AgentSignallingLimits.MaxSignallingStringLen)
        {
            _logger.LogWarning(
                "Rejecting SendIceCandidate from device {DeviceId}: SdpMid {Length} bytes exceeds {Cap} byte cap.",
                Device.ID,
                midLen,
                AgentSignallingLimits.MaxSignallingStringLen);
            return;
        }

        await _viewerHubContext.Clients
            .Client(viewerConnectionId)
            .SendAsync("ReceiveIceCandidate", candidate);
    }

    /// <summary>
    /// Cross-cutting authorisation helper for the two agent → server
    /// signalling methods (<see cref="SendSdpAnswer"/> /
    /// <see cref="SendIceCandidate"/>). Validates that:
    /// <list type="bullet">
    /// <item>the routing fields are non-empty,</item>
    /// <item><paramref name="sessionId"/> resolves to a session in the cache,</item>
    /// <item>the session's <c>DeviceId</c> matches the calling agent's <c>Device.ID</c>
    /// (cross-device routing prevention),</item>
    /// <item><paramref name="viewerConnectionId"/> appears in the
    /// session's <c>ViewerList</c> (cross-viewer routing prevention).</item>
    /// </list>
    /// All failures are warn-logged (with the device id, never the
    /// SDP / candidate body) and translated into a silent return at
    /// the call site so a hostile / mis-configured agent cannot
    /// distinguish between "wrong session", "wrong viewer", and
    /// "session expired".
    /// </summary>
    private bool TryAuthorizeViewerSignalling(
        string sessionId,
        string viewerConnectionId,
        out string resolvedViewerConnectionId)
    {
        resolvedViewerConnectionId = string.Empty;

        if (string.IsNullOrWhiteSpace(sessionId) ||
            string.IsNullOrWhiteSpace(viewerConnectionId))
        {
            _logger.LogWarning(
                "Rejecting agent → viewer signalling from device {DeviceId}: empty routing field.",
                Device?.ID);
            return false;
        }

        if (sessionId.Length > AgentSignallingLimits.MaxRoutingStringLen ||
            viewerConnectionId.Length > AgentSignallingLimits.MaxRoutingStringLen)
        {
            _logger.LogWarning(
                "Rejecting agent → viewer signalling from device {DeviceId}: routing field exceeds {Cap} byte cap.",
                Device?.ID,
                AgentSignallingLimits.MaxRoutingStringLen);
            return false;
        }

        if (!_remoteControlSessions.TryGetValue(sessionId, out var session))
        {
            _logger.LogWarning(
                "Rejecting agent → viewer signalling from device {DeviceId}: session {SessionId} not in cache.",
                Device?.ID,
                sessionId);
            return false;
        }

        if (!string.Equals(session.DeviceId, Device?.ID, StringComparison.Ordinal))
        {
            _logger.LogWarning(
                "Rejecting agent → viewer signalling from device {DeviceId}: session {SessionId} belongs to a different device.",
                Device?.ID,
                sessionId);
            return false;
        }

        if (!session.ViewerList.Contains(viewerConnectionId))
        {
            _logger.LogWarning(
                "Rejecting agent → viewer signalling from device {DeviceId}: viewer {ViewerConnectionId} not in session {SessionId} viewer list.",
                Device?.ID,
                viewerConnectionId,
                sessionId);
            return false;
        }

        resolvedViewerConnectionId = viewerConnectionId;
        return true;
    }

    public Task SendLogs(string logChunk, string requesterConnectionId)
    {
        var message = new ReceiveLogsMessage(logChunk);
        return _messenger.Send(message, requesterConnectionId);
    }

    public void SetServerVerificationToken(string verificationToken)
    {
        if (Device is null)
        {
            return;
        }
        Device.ServerVerificationToken = verificationToken;
        _dataService.SetServerVerificationToken(Device.ID, verificationToken);
    }

    public Task TransferCompleted(string transferId, string requesterId)
    {
        var message = new TransferCompleteMessage(transferId);
        return _messenger.Send(message, requesterId);
    }

    /// <summary>
    /// Receives the installed-applications inventory pushed by the agent
    /// in response to a <c>RequestInstalledApplications</c> call.
    /// Persists the snapshot and broadcasts the result so the requesting
    /// browser circuit can refresh.
    /// </summary>
    public async Task InstalledApplicationsResult(InstalledApplicationsResultDto result)
    {
        if (Device is null || result is null)
        {
            return;
        }

        if (result.Success)
        {
            await _installedApplicationsService.SaveSnapshotAsync(
                Device.ID,
                result.Applications,
                _systemTime.Now);
        }
        else
        {
            _logger.LogWarning(
                "InstalledApplicationsResult reported failure. Device={deviceId} Error={error}",
                Device.ID,
                result.ErrorMessage);
        }

        await _messenger.Send(new InstalledApplicationsResultMessage(Device.ID, result));
    }

    /// <summary>
    /// Receives the result of an uninstall operation. Stored only as a
    /// log line and broadcast to subscribers; the operation itself is
    /// audit-logged on the agent.
    /// </summary>
    public async Task UninstallApplicationResult(UninstallApplicationResultDto result)
    {
        if (Device is null || result is null)
        {
            return;
        }

        _logger.LogInformation(
            "UninstallApplicationResult. Device={deviceId} App={applicationKey} " +
            "Success={success} ExitCode={exitCode} DurationMs={durationMs}",
            Device.ID,
            result.ApplicationKey,
            result.Success,
            result.ExitCode,
            result.DurationMs);

        await _messenger.Send(new UninstallApplicationResultMessage(Device.ID, result));
    }

    /// <summary>
    /// Receives the terminal result of a package install/uninstall job
    /// from the agent. Persists the outcome via
    /// <see cref="IPackageInstallJobService"/> (which enforces the legal
    /// state-machine transition) and broadcasts a refresh message to
    /// any subscribed circuits.
    /// </summary>
    public async Task PackageInstallResult(PackageInstallResultDto result)
    {
        if (Device is null || result is null)
        {
            return;
        }

        if (!Guid.TryParse(result.JobId, out var jobId))
        {
            _logger.LogWarning(
                "PackageInstallResult received with invalid JobId. Device={deviceId} JobId={jobId}",
                Device.ID, result.JobId);
            return;
        }

        // Cross-org safety: confirm the job belongs to the same org as
        // the reporting device before accepting the result. A
        // misbehaving agent must not be able to terminate a job it
        // doesn't own.
        var job = await _packageInstallJobService.GetJobAsync(Device.OrganizationID, jobId);
        if (job is null)
        {
            _logger.LogWarning(
                "PackageInstallResult ignored — job not found in device's organization. " +
                "Device={deviceId} OrgId={orgId} JobId={jobId}",
                Device.ID, Device.OrganizationID, jobId);
            return;
        }

        if (!string.Equals(job.DeviceId, Device.ID, StringComparison.OrdinalIgnoreCase))
        {
            _logger.LogWarning(
                "PackageInstallResult ignored — device mismatch. " +
                "ReportingDevice={deviceId} JobDevice={jobDevice} JobId={jobId}",
                Device.ID, job.DeviceId, jobId);
            return;
        }

        await _packageInstallJobService.CompleteJobAsync(jobId, result);

        _logger.LogInformation(
            "PackageInstallResult. Device={deviceId} JobId={jobId} " +
            "Success={success} ExitCode={exitCode} DurationMs={durationMs}",
            Device.ID, jobId, result.Success, result.ExitCode, result.DurationMs);

        await _messenger.Send(new PackageInstallResultMessage(Device.ID, result));
    }

    private async Task<bool> CheckForDeviceBan(params string[] deviceIdNameOrIPs)
    {
        var settings = await _dataService.GetSettings();
        foreach (var device in deviceIdNameOrIPs)
        {
            if (string.IsNullOrWhiteSpace(device))
            {
                continue;
            }

            if (settings.BannedDevices.Any(x => !string.IsNullOrWhiteSpace(x) &&
                x.Equals(device, StringComparison.OrdinalIgnoreCase)))
            {
                _logger.LogWarning("Device ID/name/IP ({device}) is banned.  Sending uninstall command.", device);

                await Clients.Caller.UninstallAgent();
                return true;
            }
        }

        return false;
    }
}
