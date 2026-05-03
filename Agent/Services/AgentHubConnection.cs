using Microsoft.AspNetCore.SignalR.Client;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;
using Remotely.Agent.Extensions;
using Remotely.Agent.Interfaces;
using Remotely.Shared;
using Remotely.Shared.Dtos;
using Remotely.Shared.Enums;
using Remotely.Shared.Interfaces;
using Remotely.Shared.Models;
using System;
using System.Diagnostics.CodeAnalysis;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Timers;
using Timer = System.Timers.Timer;
using Remotely.Desktop.Native.Windows;

namespace Remotely.Agent.Services;

public interface IAgentHubConnection : IAgentHubClient
{
    bool IsConnected { get; }

    Task Connect();
    Task SendHeartbeat();
}

public class AgentHubConnection : IAgentHubConnection, IDisposable
{
    private readonly IAppLauncher _appLauncher;
    private readonly IHostApplicationLifetime _appLifetime;
    private readonly IChatClientService _chatService;
    private readonly IConfigService _configService;
    private readonly IDeviceInformationService _deviceInfoService;
    private readonly IFileLogsManager _fileLogsManager;
    private readonly IHttpClientFactory _httpFactory;
    private readonly IInstalledApplicationsProvider _installedAppsProvider;
    private readonly IPackageProvider _packageProvider;
    private readonly ILogger<AgentHubConnection> _logger;
    private readonly IScriptExecutor _scriptExecutor;
    private readonly IScriptingShellFactory _scriptingShellFactory;
    private readonly IUninstaller _uninstaller;
    private readonly IUpdater _updater;
    private readonly IWakeOnLanService _wakeOnLanService;
    private ConnectionInfo? _connectionInfo;
    private Timer? _heartbeatTimer;
    private HubConnection? _hubConnection;
    private bool _isServerVerified;

    public AgentHubConnection(
        IConfigService configService,
        IUninstaller uninstaller,
        IScriptExecutor scriptExecutor,
        IChatClientService chatService,
        IAppLauncher appLauncher,
        IUpdater updater,
        IDeviceInformationService deviceInfoService,
        IHttpClientFactory httpFactory,
        IWakeOnLanService wakeOnLanService,
        IFileLogsManager fileLogsManager,
        IHostApplicationLifetime appLifetime,
        IScriptingShellFactory scriptingShellFactory,
        IInstalledApplicationsProvider installedAppsProvider,
        IPackageProvider packageProvider,
        ILogger<AgentHubConnection> logger)
    {
        _configService = configService;
        _uninstaller = uninstaller;
        _scriptExecutor = scriptExecutor;
        _appLauncher = appLauncher;
        _chatService = chatService;
        _updater = updater;
        _deviceInfoService = deviceInfoService;
        _httpFactory = httpFactory;
        _wakeOnLanService = wakeOnLanService;
        _logger = logger;
        _fileLogsManager = fileLogsManager;
        _appLifetime = appLifetime;
        _scriptingShellFactory = scriptingShellFactory;
        _installedAppsProvider = installedAppsProvider;
        _packageProvider = packageProvider;
    }

    public bool IsConnected => _hubConnection?.State == HubConnectionState.Connected;

    public async Task ChangeWindowsSession(string viewerConnectionId, string sessionId, string accessKey, string userConnectionId, string requesterName, string orgName, string orgId, int targetSessionId)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("Session change attempted before server was verified.");
                return;
            }

            await _appLauncher.RestartScreenCaster(new[] { viewerConnectionId }, sessionId, accessKey, userConnectionId, requesterName, orgName, orgId, _hubConnection, targetSessionId);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while handling ChangeWindowsSession.");
        }
    }

    public async Task Connect()
    {
        using var throttle = new SemaphoreSlim(1, 1);

        for (var i = 1; true; i++)
        {
            try
            {
                var waitSeconds = Math.Min(60, Math.Pow(i, 2));
                // This will allow the first attempt to go through immediately, but
                // subsequent attempts will have an exponential delay.
                _ = await throttle.WaitAsync(TimeSpan.FromSeconds(waitSeconds));

                _logger.LogInformation("Attempting to connect to server.");

                _connectionInfo = _configService.GetConnectionInfo();

                if (string.IsNullOrWhiteSpace(_connectionInfo.OrganizationID))
                {
                    _logger.LogError("Organization ID is not set.  Please set it in the config file.");
                    continue;
                }

                if (string.IsNullOrWhiteSpace(_connectionInfo.Host))
                {
                    _logger.LogError("Host (server URL) is not set.  Please set it in the config file.");
                    continue;
                }

                if (_hubConnection is not null)
                {
                    await _hubConnection.DisposeAsync();
                }

                _hubConnection = new HubConnectionBuilder()
                    .WithUrl(_connectionInfo.Host + "/hubs/service")
                    .WithAutomaticReconnect(new RetryPolicy(_logger))
                    .AddMessagePackProtocol()
                    .Build();

                RegisterMessageHandlers();

                _hubConnection.Reconnected += HubConnection_Reconnected;

                await _hubConnection.StartAsync();

                _logger.LogInformation("Connected to server.");

                var device = await _deviceInfoService.CreateDevice(_connectionInfo.DeviceID, _connectionInfo.OrganizationID);

                var result = await _hubConnection.InvokeAsync<bool>("DeviceCameOnline", device);

                if (!result)
                {
                    // Orgnanization ID wasn't found, or this device is already connected.
                    // The above can be caused by temporary issues on the server.  So we'll do
                    // nothing here and wait for it to get resolved.
                    _logger.LogError("There was an issue registering with the server.  The server might be undergoing maintenance, or the supplied organization ID might be incorrect.");
                    continue;
                }

                if (!await VerifyServer())
                {
                    continue;
                }

                if (await CheckForServerMigration())
                {
                    continue;
                }

                // TODO: Move to background service.
                _heartbeatTimer?.Dispose();
                _heartbeatTimer = new Timer(TimeSpan.FromMinutes(5).TotalMilliseconds);
                _heartbeatTimer.Elapsed += HeartbeatTimer_Elapsed;
                _heartbeatTimer.Start();

                await _hubConnection.SendAsync("CheckForPendingSriptRuns");
                await _hubConnection.SendAsync("CheckForPendingRemoteControlSessions");

                break;
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Error while connecting to server.");
            }
        }
    }

    public async Task DeleteLogs()
    {
        try
        {
            await _fileLogsManager.DeleteLogs(_appLifetime.ApplicationStopping);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while deleting logs.");
        }
    }

    public void Dispose()
    {
        GC.SuppressFinalize(this);
        _heartbeatTimer?.Dispose();
    }

    public async Task ExecuteCommand(ScriptingShell shell, string command, string authToken, string senderUsername, string senderConnectionId)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning(
                    "Command attempted before server was verified.  Shell: {shell}.  Command: {command}.  Sender: {senderConnectionID}",
                    shell,
                    command,
                    senderConnectionId);
                return;
            }

            await _scriptExecutor.RunCommandFromTerminal(
                    shell,
                    command,
                    authToken,
                    senderUsername,
                    senderConnectionId,
                    ScriptInputType.Terminal,
                    TimeSpan.FromSeconds(30),
                    _hubConnection)
                .ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while executing command.");
        }
    }

    public async Task ExecuteCommandFromApi(ScriptingShell shell, string authToken, string requestID, string command, string senderUsername)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning(
                    "Command attempted before server was verified.  Shell: {shell}.  Command: {command}.  Sender: {senderUsername}",
                    shell,
                    command,
                    senderUsername);
                return;
            }

            await _scriptExecutor
                .RunCommandFromApi(shell, requestID, command, senderUsername, authToken, _hubConnection)
                .ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while executing command from API.");
        }
    }

    public async Task GetLogs(string senderConnectionId)
    {
        try
        {
            EnsureHubConnection();

            if (!await _fileLogsManager.AnyLogsExist(_appLifetime.ApplicationStopping))
            {
                var message = "There are no log entries written.";
                await _hubConnection.InvokeAsync("SendLogs", message, senderConnectionId).ConfigureAwait(false);
                return;
            }

            await foreach (var chunk in _fileLogsManager.ReadAllBytes(_appLifetime.ApplicationStopping))
            {
                var lines = Encoding.UTF8.GetString(chunk);
                await _hubConnection.InvokeAsync("SendLogs", lines, senderConnectionId).ConfigureAwait(false);
            }
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while retrieving logs.");
        }
    }

    public async Task GetPowerShellCompletions(string inputText, int currentIndex, CompletionIntent intent, bool? forward, string senderConnectionId)
    {
        try
        {
            EnsureHubConnection();
            var session = _scriptingShellFactory.GetOrCreatePsCoreShell(senderConnectionId);
            var completion = session.GetCompletions(inputText, currentIndex, forward);
            var completionModel = completion.ToPwshCompletion();
            await _hubConnection
                .InvokeAsync("ReturnPowerShellCompletions", completionModel, intent, senderConnectionId)
                .ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while getting PowerShell completions.");
        }
    }

    public Task InvokeCtrlAltDel()
    {
        try
        {
            if (!OperatingSystem.IsWindows())
            {
                return Task.CompletedTask;
            }

            if (!_isServerVerified)
            {
                _logger.LogWarning("CtrlAltDel attempted before server was verified.");
                return Task.CompletedTask;
            }

            User32.SendSAS(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while invoking CtrlAltDel.");
        }
        return Task.CompletedTask;
    }

    public async Task ReinstallAgent()
    {
        try
        {
            await _updater.InstallLatestVersion();
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while reinstalling agent.");
        }
    }

    public async Task RemoteControl(Guid sessionId, string accessKey, string userConnectionId, string requesterName, string orgName, string orgId)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("Remote control attempted before server was verified.");
                return;
            }
            await _appLauncher.LaunchRemoteControl(-1, $"{sessionId}", accessKey, userConnectionId, requesterName, orgName, orgId, _hubConnection);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while starting remote control.");
        }
    }

    public async Task RestartScreenCaster(string[] viewerIds, string sessionId, string accessKey, string userConnectionId, string requesterName, string orgName, string orgId)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("Remote control attempted before server was verified.");
                return;
            }
            await _appLauncher.RestartScreenCaster(
                viewerIds,
                sessionId,
                accessKey,
                userConnectionId,
                requesterName,
                orgName,
                orgId,
                _hubConnection);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while restarting screen caster.");
        }
    }

    public async Task RunScript(
        Guid savedScriptId,
        int scriptRunId,
        string initiator,
        ScriptInputType scriptInputType,
        string authToken)
    {
        try
        {
            if (!_isServerVerified)
            {
                _logger.LogWarning(
                    "Script run attempted before server was verified.  Script ID: {savedScriptId}.  Initiator: {initiator}",
                    savedScriptId,
                    initiator);
                return;
            }

            await _scriptExecutor.RunScript(savedScriptId, scriptRunId, initiator, scriptInputType, authToken);

        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while running script.");
        }
    }

    public async Task SendChatMessage(string senderName, string message, string orgName, string orgId, bool disconnected, string senderConnectionId)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("Chat attempted before server was verified.");
                return;
            }

            await _chatService
                .SendMessage(senderName, message, orgName, orgId, disconnected, senderConnectionId, _hubConnection)
                .ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while handling chat message.");
        }
    }

    public async Task SendHeartbeat()
    {
        try
        {
            if (_connectionInfo is null || _hubConnection is null)
            {
                return;
            }

            if (string.IsNullOrWhiteSpace(_connectionInfo.OrganizationID))
            {
                _logger.LogError("Organization ID is not set.  Please set it in the config file.");
                return;
            }

            var currentInfo = await _deviceInfoService.CreateDevice(_connectionInfo.DeviceID, _connectionInfo.OrganizationID);
            await _hubConnection
                .SendAsync("DeviceHeartbeat", currentInfo)
                .ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Error while sending heartbeat.");
        }
    }

    public async Task TransferFileFromBrowserToAgent(string transferId, string[] fileIds, string requesterId, string expiringToken)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("File upload attempted before server was verified.");
                return;
            }

            _logger.LogInformation("File upload started by {requesterID}.", requesterId);

            var sharedFilePath = Directory.CreateDirectory(Path.Combine(Path.GetTempPath(), "RemotelySharedFiles")).FullName;

            foreach (var fileID in fileIds)
            {
                var url = $"{_connectionInfo?.Host}/API/FileSharing/{fileID}";
                using var client = _httpFactory.CreateClient();
                client.DefaultRequestHeaders.Add(AppConstants.ExpiringTokenHeaderName, expiringToken);
                using var response = await client.GetAsync(url);

                var filename = response.Content.Headers.ContentDisposition?.FileName ?? Path.GetRandomFileName();
                var invalidChars = Path.GetInvalidFileNameChars().ToHashSet();
                var legalChars = filename.ToCharArray().Where(x => !invalidChars.Contains(x));

                filename = new string(legalChars.ToArray());

                using var rs = await response.Content.ReadAsStreamAsync();
                using var fs = new FileStream(Path.Combine(sharedFilePath, filename), FileMode.Create);
                rs.CopyTo(fs);
            }
            await _hubConnection.SendAsync("TransferCompleted", transferId, requesterId);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while transfering file from browser to agent.");
        }
    }

    public Task TriggerHeartbeat() => SendHeartbeat();

    public Task UninstallAgent()
    {
        try
        {
            _uninstaller.UninstallAgent();
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while uninstalling agent.");
        }
        return Task.CompletedTask;
    }

    /// <summary>
    /// Manifest-backed agent-upgrade dispatch (slice R8) targets the
    /// Rust agent only — the legacy .NET agent ships PR E's polling
    /// updater (<see cref="IUpdater.InstallLatestVersion"/>) which keeps
    /// the existing zip-replace path. The hub method is wired so a
    /// mixed fleet doesn't see "method not handled" hub completions on
    /// every dispatch attempt; the actual install happens through the
    /// updater's normal poll loop.
    /// </summary>
    public Task InstallAgentUpdate(
        string downloadUrl,
        string version,
        string sha256,
        string signatureUrl,
        string signedBy)
    {
        _logger.LogInformation(
            "Server requested agent upgrade to version {version} via manifest URL {downloadUrl}; " +
            "legacy .NET agent ignores hub-pushed upgrades and relies on the polling updater.",
            version, downloadUrl);
        return Task.CompletedTask;
    }

    public async Task WakeDevice(string macAddress)
    {
        try
        {
            _logger.LogInformation(
                    "Received request to wake device with MAC address {macAddress}.",
                    macAddress);
            await _wakeOnLanService.WakeDevice(macAddress);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while waking device.");
        }
    }

    public async Task RequestInstalledApplications(string requestId)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("RequestInstalledApplications attempted before server was verified.");
                return;
            }

            _logger.LogInformation("Enumerating installed applications. RequestId={requestId}", requestId);

            var (success, error, apps) = await _installedAppsProvider
                .GetInstalledApplicationsAsync(_appLifetime.ApplicationStopping)
                .ConfigureAwait(false);

            var dto = new InstalledApplicationsResultDto
            {
                RequestId = requestId,
                Success = success,
                ErrorMessage = error,
                Applications = apps,
            };

            await _hubConnection
                .SendAsync("InstalledApplicationsResult", dto, _appLifetime.ApplicationStopping)
                .ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            // Service shutting down — nothing to report.
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while enumerating installed applications.");
            await TrySendInstalledApplicationsFailure(requestId, ex.Message).ConfigureAwait(false);
        }
    }

    public async Task UninstallApplication(string requestId, string applicationKey)
    {
        var stopwatch = System.Diagnostics.Stopwatch.StartNew();
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("UninstallApplication attempted before server was verified.");
                return;
            }

            _logger.LogInformation(
                "Uninstall request received. RequestId={requestId} ApplicationKey={applicationKey}",
                requestId, applicationKey);

            var (success, exitCode, stdout, stderr, error) = await _installedAppsProvider
                .UninstallApplicationAsync(applicationKey, _appLifetime.ApplicationStopping)
                .ConfigureAwait(false);

            stopwatch.Stop();

            var dto = new UninstallApplicationResultDto
            {
                RequestId = requestId,
                ApplicationKey = applicationKey,
                Success = success,
                ExitCode = exitCode,
                Stdout = stdout,
                Stderr = stderr,
                ErrorMessage = error,
                DurationMs = stopwatch.ElapsedMilliseconds,
            };

            _logger.LogInformation(
                "Uninstall completed. RequestId={requestId} ApplicationKey={applicationKey} " +
                "Success={success} ExitCode={exitCode} DurationMs={durationMs}",
                requestId, applicationKey, success, exitCode, stopwatch.ElapsedMilliseconds);

            await _hubConnection
                .SendAsync("UninstallApplicationResult", dto, _appLifetime.ApplicationStopping)
                .ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex)
        {
            stopwatch.Stop();
            _logger.LogError(ex, "Error while uninstalling application.");
            try
            {
                await _hubConnection!.SendAsync(
                    "UninstallApplicationResult",
                    new UninstallApplicationResultDto
                    {
                        RequestId = requestId,
                        ApplicationKey = applicationKey,
                        Success = false,
                        ExitCode = -1,
                        ErrorMessage = ex.Message,
                        DurationMs = stopwatch.ElapsedMilliseconds,
                    });
            }
            catch
            {
                // Best-effort; original exception already logged.
            }
        }
    }

    public async Task InstallPackage(PackageInstallRequestDto request)
    {
        try
        {
            EnsureHubConnection();

            if (!_isServerVerified)
            {
                _logger.LogWarning("InstallPackage attempted before server was verified.");
                return;
            }
            if (request is null || string.IsNullOrWhiteSpace(request.JobId))
            {
                _logger.LogWarning("InstallPackage received with missing request or JobId.");
                return;
            }

            _logger.LogInformation(
                "Package operation received. JobId={jobId} Provider={provider} " +
                "Action={action} PackageId={packageId}",
                request.JobId, request.Provider, request.Action, request.PackageIdentifier);

            PackageInstallResultDto result;
            if (_packageProvider.CanHandle(request))
            {
                result = await _packageProvider
                    .ExecuteAsync(request, _appLifetime.ApplicationStopping)
                    .ConfigureAwait(false);
            }
            else
            {
                result = new PackageInstallResultDto
                {
                    JobId = request.JobId,
                    Success = false,
                    ExitCode = -1,
                    ErrorMessage = $"No provider on this device can handle '{request.Provider}'.",
                };
            }
            // Defensive: providers SHOULD echo the JobId, but make sure
            // the wire payload is always tied to the originating job.
            result.JobId = request.JobId;

            _logger.LogInformation(
                "Package operation completed. JobId={jobId} Success={success} " +
                "ExitCode={exitCode} DurationMs={duration}",
                result.JobId, result.Success, result.ExitCode, result.DurationMs);

            await _hubConnection!
                .SendAsync("PackageInstallResult", result, _appLifetime.ApplicationStopping)
                .ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            // Service shutting down — best-effort.
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error while handling InstallPackage. JobId={jobId}", request?.JobId);
            try
            {
                if (request is not null && _hubConnection is not null)
                {
                    await _hubConnection.SendAsync("PackageInstallResult", new PackageInstallResultDto
                    {
                        JobId = request.JobId,
                        Success = false,
                        ExitCode = -1,
                        ErrorMessage = ex.Message,
                    }).ConfigureAwait(false);
                }
            }
            catch
            {
                // Best-effort; original exception already logged.
            }
        }
    }

    // -----------------------------------------------------------------
    // Slice R7.g / R7.j — WebRTC signalling + ICE-server-config
    // forwarding hub methods.
    //
    // The Rust agent's dispatcher routes these through the desktop
    // transport provider (see agent-rs/crates/cmremote-platform/src/
    // desktop/{guards,session,webrtc}.rs). The legacy .NET agent does
    // NOT speak the new typed methods — viewers paired with a legacy
    // agent continue to use the existing byte-array DtoWrapper channel
    // (SendDtoToClient / SendDtoToViewer) for SDP and ICE traffic.
    // These stubs exist purely to satisfy the IAgentHubClient contract
    // so the legacy agent keeps building during the cut-over to the
    // Rust workspace; they log at Debug and return immediately.
    // -----------------------------------------------------------------

    public Task SendSdpOffer(SdpOfferDto offer)
    {
        _logger.LogDebug(
            "SendSdpOffer received on legacy .NET agent — ignored. " +
            "WebRTC signalling on this agent flows through the byte-array " +
            "DtoWrapper channel; the typed hub method is honoured by the Rust agent only.");
        return Task.CompletedTask;
    }

    public Task SendSdpAnswer(SdpAnswerDto answer)
    {
        _logger.LogDebug(
            "SendSdpAnswer received on legacy .NET agent — ignored. " +
            "Use the byte-array DtoWrapper channel for SDP traffic on this agent.");
        return Task.CompletedTask;
    }

    public Task SendIceCandidate(IceCandidateDto candidate)
    {
        _logger.LogDebug(
            "SendIceCandidate received on legacy .NET agent — ignored. " +
            "Use the byte-array DtoWrapper channel for ICE candidates on this agent.");
        return Task.CompletedTask;
    }

    public Task ProvideIceServers(ProvideIceServersRequestDto request)
    {
        _logger.LogDebug(
            "ProvideIceServers received on legacy .NET agent — ignored. " +
            "ICE server configuration on this agent is read from the embedded server data; " +
            "the typed hub method is honoured by the Rust agent only.");
        return Task.CompletedTask;
    }

    private async Task TrySendInstalledApplicationsFailure(string requestId, string error)
    {
        try
        {
            if (_hubConnection is null)
            {
                return;
            }
            await _hubConnection.SendAsync(
                "InstalledApplicationsResult",
                new InstalledApplicationsResultDto
                {
                    RequestId = requestId,
                    Success = false,
                    ErrorMessage = error,
                });
        }
        catch
        {
            // Best-effort.
        }
    }

    private async Task<bool> CheckForServerMigration()
    {
        if (_connectionInfo is null || _hubConnection is null)
        {
            return false;
        }

        var serverUrl = await _hubConnection.InvokeAsync<string>("GetServerUrl");

        if (Uri.TryCreate(serverUrl, UriKind.Absolute, out var serverUri) &&
            Uri.TryCreate(_connectionInfo.Host, UriKind.Absolute, out var savedUri) &&
            serverUri.Host != savedUri.Host)
        {
            _connectionInfo.Host = serverUrl.Trim().TrimEnd('/');
            _connectionInfo.ServerVerificationToken = null;
            _configService.SaveConnectionInfo(_connectionInfo);
            await _hubConnection.DisposeAsync();
            return true;
        }
        return false;
    }

    [MemberNotNull(nameof(_hubConnection))]
    private void EnsureHubConnection()
    {
        if (_hubConnection is null || _hubConnection.State != HubConnectionState.Connected)
        {
            throw new InvalidOperationException("Hub connection is not established.");
        }
    }
    private async void HeartbeatTimer_Elapsed(object? sender, ElapsedEventArgs e)
    {
        await SendHeartbeat();
    }

    private async Task HubConnection_Reconnected(string? arg)
    {
        if (_connectionInfo is null || _hubConnection is null)
        {
            return;
        }

        _logger.LogInformation("Reconnected to server.");
        await _updater.CheckForUpdates();

        var device = await _deviceInfoService.CreateDevice(_connectionInfo.DeviceID, $"{_connectionInfo.OrganizationID}");

        if (!await _hubConnection.InvokeAsync<bool>("DeviceCameOnline", device))
        {
            await Connect();
            return;
        }

        if (await CheckForServerMigration())
        {
            await Connect();
            return;
        }
    }

    private void RegisterMessageHandlers()
    {
        if (_hubConnection is null)
        {
            throw new InvalidOperationException("Hub connection is null.");
        }

        // TODO: Replace all these parameters with a single DTO per method.
        _hubConnection.On<string, string, string, string, string, string, string, int>(
            nameof(ChangeWindowsSession),
            ChangeWindowsSession);

        _hubConnection.On<string, string, string, string, bool, string>(nameof(SendChatMessage), SendChatMessage);

        _hubConnection.On(nameof(InvokeCtrlAltDel), InvokeCtrlAltDel);

        _hubConnection.On(nameof(DeleteLogs), DeleteLogs);

        _hubConnection.On<ScriptingShell, string, string, string, string>(nameof(ExecuteCommand), ExecuteCommand);

        _hubConnection.On<ScriptingShell, string, string, string, string>(nameof(ExecuteCommandFromApi), ExecuteCommandFromApi);

        _hubConnection.On<string>(nameof(GetLogs), GetLogs);

        _hubConnection.On<string, int, CompletionIntent, bool?, string>(nameof(GetPowerShellCompletions), GetPowerShellCompletions);

        _hubConnection.On(nameof(ReinstallAgent), ReinstallAgent);

        _hubConnection.On(nameof(UninstallAgent), UninstallAgent);

        _hubConnection.On<string, string, string, string, string>(nameof(InstallAgentUpdate), InstallAgentUpdate);

        _hubConnection.On<Guid, string, string, string, string, string>(nameof(RemoteControl), RemoteControl);

        _hubConnection.On<string[], string, string, string, string, string, string>(
            nameof(RestartScreenCaster),
            RestartScreenCaster);

        _hubConnection.On<Guid, int, string, ScriptInputType, string>(nameof(RunScript), RunScript);

        _hubConnection.On<string, string[], string, string>(
            nameof(TransferFileFromBrowserToAgent),
            TransferFileFromBrowserToAgent);

        _hubConnection.On(nameof(TriggerHeartbeat), TriggerHeartbeat);

        _hubConnection.On<string>(nameof(WakeDevice), WakeDevice);

        _hubConnection.On<string>(nameof(RequestInstalledApplications), RequestInstalledApplications);

        _hubConnection.On<string, string>(nameof(UninstallApplication), UninstallApplication);

        _hubConnection.On<PackageInstallRequestDto>(nameof(InstallPackage), InstallPackage);
    }

    private async Task<bool> VerifyServer()
    {
        if (_connectionInfo is null || _hubConnection is null)
        {
            return false;
        }

        if (string.IsNullOrWhiteSpace(_connectionInfo.ServerVerificationToken))
        {
            _isServerVerified = true;
            _connectionInfo.ServerVerificationToken = Guid.NewGuid().ToString();
            await _hubConnection.SendAsync("SetServerVerificationToken", _connectionInfo.ServerVerificationToken).ConfigureAwait(false);
            _configService.SaveConnectionInfo(_connectionInfo);
        }
        else
        {
            var verificationToken = await _hubConnection.InvokeAsync<string>("GetServerVerificationToken");

            if (verificationToken == _connectionInfo.ServerVerificationToken)
            {
                _isServerVerified = true;
            }
            else
            {
                _logger.LogWarning("Server sent an incorrect verification token.  Token Sent: {verificationToken}.", verificationToken);
                return false;
            }
        }

        return true;
    }

    private class RetryPolicy : IRetryPolicy
    {
        private readonly ILogger<AgentHubConnection> _logger;

        public RetryPolicy(ILogger<AgentHubConnection> logger)
        {
            _logger = logger;
        }

        public TimeSpan? NextRetryDelay(RetryContext retryContext)
        {
            if (retryContext.PreviousRetryCount == 0)
            {
                return TimeSpan.FromSeconds(3);
            }

            var waitSeconds = Random.Shared.Next(3, 10);
            _logger.LogDebug("Attempting to reconnect in {seconds} seconds.", waitSeconds);
            return TimeSpan.FromSeconds(waitSeconds);
        }
    }
}
