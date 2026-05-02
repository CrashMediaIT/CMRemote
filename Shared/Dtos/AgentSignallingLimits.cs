namespace Remotely.Shared.Dtos;

/// <summary>
/// Length caps that mirror the Rust wire-side limits in
/// <c>cmremote_wire::desktop::signalling</c>. The .NET hub
/// re-validates every server-bound signalling DTO against these
/// limits so a non-conforming agent (legacy, malicious, or simply
/// out of sync with the wire spec) cannot blow past the bounds the
/// Rust agent enforces locally before its send (slice R7.n.7).
/// </summary>
public static class AgentSignallingLimits
{
    /// <summary>
    /// Maximum byte length permitted for an inline SDP blob. Mirrors
    /// <c>cmremote_wire::desktop::signalling::MAX_SDP_BYTES</c>
    /// (16 KiB — comfortably above the largest legitimate SDP a
    /// browser-side WebRTC stack produces, and well below the point
    /// where a malformed body could be used as a memory-exhaustion
    /// vector).
    /// </summary>
    public const int MaxSdpBytes = 16 * 1024;

    /// <summary>
    /// Maximum byte length permitted for any other signalling
    /// string — candidate line, sdp-mid, etc. Mirrors
    /// <c>cmremote_wire::desktop::signalling::MAX_SIGNALLING_STRING_LEN</c>
    /// (1 KiB — upper bound any RFC-compliant ICE candidate ever
    /// needs).
    /// </summary>
    public const int MaxSignallingStringLen = 1024;

    /// <summary>
    /// Maximum byte length permitted for a routing field (session
    /// id, viewer connection id). 256 bytes is comfortably above any
    /// legitimate canonical-UUID or SignalR connection-id length and
    /// well below the point where the field becomes a useful
    /// memory-exhaustion vector.
    /// </summary>
    public const int MaxRoutingStringLen = 256;
}
