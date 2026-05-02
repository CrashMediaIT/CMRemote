namespace Remotely.Shared.Dtos;

/// <summary>
/// Server-bound counterpart of <see cref="SdpAnswerDto"/> — the agent
/// invokes <c>AgentHub.SendSdpAnswer(AgentSdpAnswerDto)</c> with a
/// locally-produced SDP answer once its WebRTC peer connection has
/// accepted the viewer's offer (slice R7.n.7).
/// <para>
/// Smaller than the inbound <see cref="SdpAnswerDto"/> on purpose:
/// the hub already knows which operator initiated the negotiation
/// (it is the originator of the matching <c>SendSdpOffer</c>
/// invocation), so the agent only re-states the routing fields plus
/// the SDP payload itself. The wire form is PascalCase so the .NET
/// hub can deserialise the SignalR arguments array directly into
/// this DTO and the Rust agent can serialise its
/// <c>cmremote_wire::AgentSdpAnswer</c> with the same field names.
/// </para>
/// </summary>
public class AgentSdpAnswerDto
{
    /// <summary>SignalR connection id of the viewer this answer is destined for.</summary>
    public string ViewerConnectionId { get; set; } = string.Empty;

    /// <summary>Server-issued session UUID — same identity as <c>RemoteControlSessionRequest.SessionId</c>.</summary>
    public string SessionId { get; set; } = string.Empty;

    /// <summary>Raw SDP blob produced by the agent's <c>RTCPeerConnection</c>.</summary>
    public string Sdp { get; set; } = string.Empty;
}
