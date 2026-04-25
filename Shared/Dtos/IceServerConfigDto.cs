using System.Collections.Generic;
using Remotely.Shared.Enums;

namespace Remotely.Shared.Dtos;

/// <summary>
/// Full ICE configuration delivered to the agent's WebRTC peer
/// connection (slice R7.i) — mirrors the subset of W3C
/// <c>RTCConfiguration</c> the agent honours. .NET counterpart of the
/// Rust <c>cmremote_wire::desktop::signalling::IceServerConfig</c>
/// DTO; bounded by <see cref="MaxIceServers"/>.
/// </summary>
public class IceServerConfigDto
{
    /// <summary>
    /// Maximum number of <see cref="IceServers"/> entries the agent
    /// will accept. Mirrors the Rust constant <c>MAX_ICE_SERVERS</c>.
    /// </summary>
    public const int MaxIceServers = 8;

    /// <summary>
    /// Servers tried in declaration order. May be empty — in which
    /// case the WebRTC driver only attempts host candidates and will
    /// work only on the same LAN as the viewer.
    /// </summary>
    public List<IceServerDto> IceServers { get; set; } = new();

    /// <summary>
    /// Transport-policy hint. Defaults to
    /// <see cref="IceTransportPolicy.All"/>.
    /// </summary>
    public IceTransportPolicy IceTransportPolicy { get; set; } = IceTransportPolicy.All;
}
