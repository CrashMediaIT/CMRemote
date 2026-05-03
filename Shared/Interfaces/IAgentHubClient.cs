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

    /// <summary>
    /// Asks the agent to download, verify, and install a specific build
    /// (slice R8 — the M3 manifest-backed agent-upgrade dispatcher).
    /// The agent MUST recompute the SHA-256 of the downloaded artifact
    /// and refuse to install if it doesn't match <paramref name="sha256"/>.
    /// When <paramref name="signatureUrl"/> and <paramref name="signedBy"/>
    /// are supplied, the Rust agent MUST verify the Sigstore cosign bundle
    /// against the published certificate identity before installer handoff.
    /// On success the agent restarts and re-handshakes with its new
    /// <c>AgentVersion</c>; the dispatcher observes that bump via the
    /// session cache and marks the upgrade row Succeeded.
    /// </summary>
    /// <param name="downloadUrl">
    /// Absolute https URL pointing at the artifact (e.g. an
    /// <c>.msi</c> / <c>.deb</c> / <c>.rpm</c> / <c>.pkg</c>) the
    /// publisher manifest resolved for this device.
    /// </param>
    /// <param name="version">
    /// Target SemVer version. The dispatcher uses this to decide when
    /// the upgrade has succeeded (heartbeat reports this version).
    /// </param>
    /// <param name="sha256">
    /// Expected lowercase-hex SHA-256 of the artifact bytes from the
    /// publisher manifest. The agent re-verifies independently.
    /// </param>
    /// <param name="signatureUrl">
    /// Absolute https URL pointing at the Sigstore cosign bundle for the
    /// artifact. Required for signed-release channels.
    /// </param>
    /// <param name="signedBy">
    /// Expected certificate identity from the publisher manifest.
    /// </param>
    Task InstallAgentUpdate(
        string downloadUrl,
        string version,
        string sha256,
        string signatureUrl,
        string signedBy);

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

    /// <summary>
    /// Forwards a viewer's WebRTC SDP offer to the agent (slice R7.g).
    /// The Rust agent's dispatcher routes this to the desktop transport
    /// provider's <c>on_sdp_offer</c> hook; the legacy .NET agent ignores
    /// it (it still uses the byte-array <c>SendDtoToClient</c> channel).
    /// The agent reports its terminal outcome via
    /// <c>DesktopTransportResultDto</c>.
    /// </summary>
    Task SendSdpOffer(SdpOfferDto offer);

    /// <summary>
    /// Forwards a viewer's WebRTC SDP answer to the agent (slice R7.g).
    /// Same dispatch and rollout shape as <see cref="SendSdpOffer"/>.
    /// </summary>
    Task SendSdpAnswer(SdpAnswerDto answer);

    /// <summary>
    /// Forwards a single trickled ICE candidate from the viewer to
    /// the agent (slice R7.g). An empty <c>Candidate</c> string with
    /// <c>SdpMid</c> / <c>SdpMlineIndex</c> both <c>null</c> is the
    /// RFC 8838 end-of-candidates marker. Same dispatch and rollout
    /// shape as <see cref="SendSdpOffer"/>.
    /// </summary>
    Task SendIceCandidate(IceCandidateDto candidate);

    /// <summary>
    /// Delivers the authoritative ICE / TURN server configuration to
    /// the agent before the WebRTC peer connection starts gathering
    /// candidates (slice R7.j). Invoked once per remote-control
    /// session; the embedded
    /// <c>IceServerConfigDto.IceServers</c> become the agent's
    /// <c>RTCConfiguration.iceServers</c> for the matching session.
    /// The Rust agent's <c>check_provide_ice_servers</c> guard
    /// validates the envelope and the per-server URL / credential
    /// shape before any of it reaches the WebRTC driver. The agent
    /// reports acceptance or refusal via
    /// <c>DesktopTransportResultDto</c>.
    /// </summary>
    Task ProvideIceServers(ProvideIceServersRequestDto request);
}
