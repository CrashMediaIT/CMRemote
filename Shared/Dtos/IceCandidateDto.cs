namespace Remotely.Shared.Dtos;

/// <summary>
/// <c>SendIceCandidate(viewerConnectionId, sessionId, …, candidate,
/// sdpMid, sdpMlineIndex)</c> — a trickled ICE candidate from the
/// viewer (slice R7.g). .NET counterpart of the Rust
/// <c>cmremote_wire::desktop::signalling::IceCandidate</c> DTO.
/// <para>
/// Fields mirror W3C <c>RTCIceCandidateInit</c>.
/// <see cref="SdpMid"/> and <see cref="SdpMlineIndex"/> may be
/// absent (or both <c>null</c>) when the viewer signals an
/// end-of-candidates marker; the wire form preserves that by
/// serialising them as JSON <c>null</c>.
/// </para>
/// </summary>
public class IceCandidateDto
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
    /// <c>candidate:</c> line, RFC 5245 / 8445 grammar. Empty string
    /// is the legacy end-of-candidates signal (RFC 8838 marker).
    /// </summary>
    public string Candidate { get; set; } = string.Empty;

    /// <summary>
    /// <c>a=mid</c> of the m-line this candidate belongs to. Absent
    /// for the end-of-candidates marker.
    /// </summary>
    public string? SdpMid { get; set; }

    /// <summary>
    /// 0-based index of the m-line this candidate belongs to. Absent
    /// for the end-of-candidates marker.
    /// </summary>
    public ushort? SdpMlineIndex { get; set; }
}
