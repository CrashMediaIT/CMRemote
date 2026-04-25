using Remotely.Shared.Enums;

namespace Remotely.Shared.Dtos;

/// <summary>
/// <c>SendSdpOffer(viewerConnectionId, sessionId, …, sdp)</c> — the
/// viewer is opening (or re-opening) the WebRTC negotiation against
/// the agent. .NET counterpart of the Rust
/// <c>cmremote_wire::desktop::signalling::SdpOffer</c> DTO; PascalCase
/// JSON wire form mirrors the Rust shape byte-for-byte (slice R7.g).
/// <para>
/// The <see cref="Sdp"/> field carries the raw SDP text. The agent
/// MUST treat it as untrusted UTF-8 — never as a shell argument,
/// file path, or HTML fragment — and MUST reject it if it exceeds
/// <see cref="MaxSdpBytes"/>.
/// </para>
/// </summary>
public class SdpOfferDto
{
    /// <summary>
    /// Maximum permitted size of the <see cref="Sdp"/> payload, in
    /// UTF-8 bytes. Mirrors the Rust constant <c>MAX_SDP_BYTES</c> in
    /// <c>cmremote_wire::desktop::signalling</c>.
    /// </summary>
    public const int MaxSdpBytes = 16 * 1024;

    /// <summary>
    /// Maximum permitted length of any single signalling envelope
    /// string field (operator name, organisation, viewer connection
    /// id, etc.). Mirrors the Rust constant
    /// <c>MAX_SIGNALLING_STRING_LEN</c>.
    /// </summary>
    public const int MaxSignallingStringLen = 1024;

    /// <summary>SignalR connection id of the viewer initiating the offer.</summary>
    public string ViewerConnectionId { get; set; } = string.Empty;

    /// <summary>
    /// Server-issued session UUID — same identity as the
    /// <c>RemoteControl</c> hub method's <c>sessionId</c> argument.
    /// </summary>
    public string SessionId { get; set; } = string.Empty;

    /// <summary>Operator display name, surfaced in the consent prompt.</summary>
    public string RequesterName { get; set; } = string.Empty;

    /// <summary>Operator organisation name.</summary>
    public string OrgName { get; set; } = string.Empty;

    /// <summary>
    /// Operator organisation UUID — the agent's cross-org guard
    /// compares this against its own <c>ConnectionInfo.organization_id</c>.
    /// </summary>
    public string OrgId { get; set; } = string.Empty;

    /// <summary>
    /// Discriminator — always <see cref="SdpKind.Offer"/> on this DTO;
    /// carried explicitly so the wire form is self-describing.
    /// Required on the wire — a missing <c>Kind</c> is a malformed
    /// payload that the agent refuses.
    /// </summary>
    public SdpKind Kind { get; set; } = SdpKind.Offer;

    /// <summary>Raw SDP blob produced by the viewer's <c>RTCPeerConnection</c>.</summary>
    public string Sdp { get; set; } = string.Empty;
}
