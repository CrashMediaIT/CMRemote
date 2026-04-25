using System.Text.Json.Serialization;

namespace Remotely.Shared.Enums;

/// <summary>
/// Discriminator for the <c>SdpOfferDto</c> / <c>SdpAnswerDto</c>
/// PascalCase JSON wire form (slice R7.g). Mirrors the Rust
/// <c>cmremote_wire::desktop::signalling::SdpKind</c> enum byte-for-byte
/// — variants serialise as the strings <c>"Offer"</c> / <c>"Answer"</c>
/// per W3C <c>RTCSdpType</c>.
/// </summary>
[JsonConverter(typeof(JsonStringEnumConverter))]
public enum SdpKind
{
    /// <summary><c>type: "offer"</c> per W3C <c>RTCSdpType</c>.</summary>
    Offer = 0,

    /// <summary><c>type: "answer"</c> per W3C <c>RTCSdpType</c>.</summary>
    Answer = 1,
}
