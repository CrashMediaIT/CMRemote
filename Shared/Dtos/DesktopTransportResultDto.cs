using System.Text.Json.Serialization;

namespace Remotely.Shared.Dtos;

/// <summary>
/// Generic agent → server completion DTO returned from every desktop
/// transport hub method (<c>RemoteControl</c>,
/// <c>RestartScreenCaster</c>, <c>ChangeWindowsSession</c>,
/// <c>InvokeCtrlAltDel</c>, <c>SendSdpOffer</c>, <c>SendSdpAnswer</c>,
/// <c>SendIceCandidate</c>, <c>ProvideIceServers</c>). .NET
/// counterpart of the Rust
/// <c>cmremote_wire::desktop::DesktopTransportResult</c> DTO
/// (slices R7 + R7.g + R7.j). Defaults fail closed:
/// <see cref="Success"/> defaults to <c>false</c>.
/// <para>
/// <see cref="ErrorMessage"/> MUST NOT contain sensitive payload
/// (e.g. the <c>AccessKey</c>, an SDP body, or a TURN credential) —
/// the agent-side guards already strip them before the failure
/// crosses the wire.
/// </para>
/// </summary>
public class DesktopTransportResultDto
{
    /// <summary>
    /// Session UUID this result corresponds to — matches the
    /// <c>SessionId</c> field on the originating request DTO.
    /// </summary>
    public string SessionId { get; set; } = string.Empty;

    /// <summary>
    /// <c>true</c> iff the agent accepted and processed the request.
    /// Defaults to <c>false</c> so a malformed / partial payload
    /// fails closed.
    /// </summary>
    public bool Success { get; set; }

    /// <summary>
    /// Human-readable reason the request failed, when
    /// <see cref="Success"/> is <c>false</c>. Omitted from the JSON
    /// wire form on success — mirrors the Rust
    /// <c>#[serde(skip_serializing_if = "Option::is_none")]</c> on
    /// <c>error_message</c>.
    /// </summary>
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? ErrorMessage { get; set; }
}
