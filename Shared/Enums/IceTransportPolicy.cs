using System.Text.Json.Serialization;

namespace Remotely.Shared.Enums;

/// <summary>
/// W3C <c>RTCIceTransportPolicy</c> hint delivered to the agent's
/// WebRTC peer connection (slice R7.i). Mirrors the Rust
/// <c>cmremote_wire::desktop::signalling::IceTransportPolicy</c> enum;
/// variants serialise as <c>"All"</c> / <c>"Relay"</c>.
/// </summary>
[JsonConverter(typeof(JsonStringEnumConverter))]
public enum IceTransportPolicy
{
    /// <summary>Try host, server-reflexive, and relayed candidates (default).</summary>
    All = 0,

    /// <summary>Only attempt relayed (TURN) candidates.</summary>
    Relay = 1,
}
