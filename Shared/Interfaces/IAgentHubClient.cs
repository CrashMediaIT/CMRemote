using Remotely.Shared.Dtos;
using Remotely.Shared.Enums;

namespace Remotely.Shared.Interfaces;
public interface IAgentHubClient
{
    Task ChangeWindowsSession(
        string viewerConnectionId,
        string sessionId,
        string accessKey,
        string userConnectionId,
        string requesterName,
        string orgName,
        string orgId,
        int targetSessionId);

    Task SendChatMessage(
        string senderName, 
        string message, 
        string orgName, 
        string orgId, 
        bool disconnected, 
        string senderConnectionId);

    Task InvokeCtrlAltDel();

    Task DeleteLogs();

    Task ExecuteCommand(
        ScriptingShell shell, 
        string command, 
        string authToken, 
        string senderUsername, 
        string senderConnectionId);

    Task ExecuteCommandFromApi(ScriptingShell shell,
            string authToken,
            string requestID,
            string command,
            string senderUsername);

    Task GetLogs(string senderConnectionId);

    Task GetPowerShellCompletions(
        string inputText, 
        int currentIndex, 
        CompletionIntent intent, 
        bool? forward, 
        string senderConnectionId);

    Task ReinstallAgent();

    Task UninstallAgent();

    Task RemoteControl(
        Guid sessionId, 
        string accessKey, 
        string userConnectionId, 
        string requesterName, 
        string orgName, 
        string orgId);

    Task RestartScreenCaster(
        string[] viewerIds, 
        string sessionId, 
        string accessKey, 
        string userConnectionId, 
        string requesterName, 
        string orgName, 
        string orgId);

    Task RunScript(
        Guid savedScriptId, 
        int scriptRunId, 
        string initiator, 
        ScriptInputType scriptInputType, 
        string authToken);

    Task TransferFileFromBrowserToAgent(
        string transferId, 
        string[] fileIds, 
        string requesterId, 
        string expiringToken);

    Task TriggerHeartbeat();

    Task WakeDevice(string macAddress);

    /// <summary>
    /// Asks the agent to enumerate all installed applications (Win32 +
    /// MSI + AppX) and return them via
    /// <c>InstalledApplicationsResult</c>. Windows agents only — non-Windows
    /// agents will return a result with <c>Success=false</c>.
    /// </summary>
    Task RequestInstalledApplications(string requestId);

    /// <summary>
    /// Asks the agent to uninstall the application identified by
    /// <paramref name="applicationKey"/>. The agent re-enumerates its
    /// inventory and uses the locally-resolved uninstall command — the
    /// wire never carries an executable string the user controlled.
    /// </summary>
    Task UninstallApplication(string requestId, string applicationKey);

    /// <summary>
    /// Asks the agent to install (or uninstall, per
    /// <c>PackageInstallRequestDto.Action</c>) a package via its local
    /// provider (Chocolatey today; UploadedMsi / Executable in PR C1).
    /// The agent reports its terminal outcome via
    /// <c>PackageInstallResult</c>.
    /// </summary>
    Task InstallPackage(PackageInstallRequestDto request);
}
