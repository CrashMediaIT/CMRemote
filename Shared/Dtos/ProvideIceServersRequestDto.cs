namespace Remotely.Shared.Dtos;

/// <summary>
/// Request payload for the
/// <c>ProvideIceServers(iceServerConfig, sessionId, accessKey, …)</c>
/// hub method (slice R7.j). The .NET hub invokes this once per
/// session, before the agent emits its first SDP offer or trickled
/// ICE candidate. The agent treats the embedded
/// <see cref="IceServerConfig"/> as the authoritative
/// <c>RTCConfiguration.iceServers</c> /
/// <c>iceTransportPolicy</c> for the matching session.
/// <para>
/// .NET counterpart of the Rust
/// <c>cmremote_wire::desktop::signalling::ProvideIceServersRequest</c>
/// DTO. The envelope fields mirror <see cref="SdpOfferDto"/> verbatim
/// so the agent-side guards can be reused without a special case;
/// in particular the sensitive <see cref="AccessKey"/> is carried
/// here too so a race between the .NET server and the agent's
/// session-state cache can be resolved against the same
/// authenticator the matching <c>RemoteControl</c> request used.
/// </para>
/// </summary>
public class ProvideIceServersRequestDto
{
    /// <summary>
    /// SignalR connection id of the viewer the configuration is
    /// destined for.
    /// </summary>
    public string ViewerConnectionId { get; set; } = string.Empty;

    /// <summary>
    /// Existing remote-control session UUID — same shape as the
    /// <c>RemoteControl</c> hub method's <c>sessionId</c> argument.
    /// </summary>
    public string SessionId { get; set; } = string.Empty;

    /// <summary>
    /// <strong>Sensitive.</strong> One-shot access key paired with
    /// <see cref="SessionId"/>. MUST NOT be logged or echoed in
    /// rejection messages.
    /// </summary>
    public string AccessKey { get; set; } = string.Empty;

    /// <summary>
    /// Operator display name, surfaced in the audit trail when the
    /// configuration is accepted (or refused).
    /// </summary>
    public string RequesterName { get; set; } = string.Empty;

    /// <summary>Operator organisation name.</summary>
    public string OrgName { get; set; } = string.Empty;

    /// <summary>
    /// Operator organisation UUID — checked by the agent against its
    /// own <c>ConnectionInfo.organization_id</c> before the
    /// configuration is honoured.
    /// </summary>
    public string OrgId { get; set; } = string.Empty;

    /// <summary>
    /// The actual ICE / TURN server set the agent's WebRTC peer
    /// connection should use for this session. The credential inside
    /// any <c>turn(s):</c> entry is sensitive (see
    /// <see cref="IceServerDto.Credential"/>).
    /// </summary>
    public IceServerConfigDto IceServerConfig { get; set; } = new();
}
