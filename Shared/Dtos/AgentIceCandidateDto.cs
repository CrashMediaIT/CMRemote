namespace Remotely.Shared.Dtos;

/// <summary>
/// Server-bound counterpart of <see cref="IceCandidateDto"/> — the
/// agent invokes <c>AgentHub.SendIceCandidate(AgentIceCandidateDto)</c>
/// once per locally-trickled ICE candidate, plus once with an empty
/// <see cref="Candidate"/> string and <see cref="SdpMid"/> /
/// <see cref="SdpMlineIndex"/> both <c>null</c> to signal
/// end-of-candidates (RFC 8838 marker) (slice R7.n.7).
/// <para>
/// Smaller than the inbound <see cref="IceCandidateDto"/> on purpose
/// — see <see cref="AgentSdpAnswerDto"/> for the rationale.
/// </para>
/// </summary>
public class AgentIceCandidateDto
{
    /// <summary>SignalR connection id of the viewer the candidate is destined for.</summary>
    public string ViewerConnectionId { get; set; } = string.Empty;

    /// <summary>Server-issued session UUID.</summary>
    public string SessionId { get; set; } = string.Empty;

    /// <summary>
    /// <c>candidate:</c> line, RFC 5245 / 8445 grammar. Empty string
    /// is the end-of-candidates marker.
    /// </summary>
    public string Candidate { get; set; } = string.Empty;

    /// <summary>
    /// <c>a=mid</c> of the m-line this candidate belongs to. Absent
    /// for the end-of-candidates marker.
    /// </summary>
    public string? SdpMid { get; set; }

    /// <summary>
    /// 0-based index of the m-line this candidate belongs to.
    /// Absent for the end-of-candidates marker.
    /// </summary>
    public ushort? SdpMlineIndex { get; set; }
}
