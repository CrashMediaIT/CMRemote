using Remotely.Shared.Enums;

namespace Remotely.Shared.Dtos;

/// <summary>
/// <c>SendSdpAnswer(viewerConnectionId, sessionId, …, sdp)</c> — the
/// viewer is accepting an agent-initiated renegotiation. Same shape
/// as <see cref="SdpOfferDto"/> with <see cref="SdpKind.Answer"/>.
/// .NET counterpart of the Rust
/// <c>cmremote_wire::desktop::signalling::SdpAnswer</c> DTO (slice R7.g).
/// </summary>
public class SdpAnswerDto
{
    /// <summary>SignalR connection id of the viewer.</summary>
    public string ViewerConnectionId { get; set; } = string.Empty;

    /// <summary>Session UUID.</summary>
    public string SessionId { get; set; } = string.Empty;

    /// <summary>Operator display name.</summary>
    public string RequesterName { get; set; } = string.Empty;

    /// <summary>Operator organisation name.</summary>
    public string OrgName { get; set; } = string.Empty;

    /// <summary>Operator organisation UUID.</summary>
    public string OrgId { get; set; } = string.Empty;

    /// <summary>
    /// Discriminator — always <see cref="SdpKind.Answer"/> on this
    /// DTO. Required on the wire.
    /// </summary>
    public SdpKind Kind { get; set; } = SdpKind.Answer;

    /// <summary>Raw SDP blob.</summary>
    public string Sdp { get; set; } = string.Empty;
}
