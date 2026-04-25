using System.Text.Json.Serialization;

namespace Remotely.Shared.Enums;

/// <summary>
/// How an <c>IceServerDto.Credential</c> string should be interpreted
/// (slice R7.i). Mirrors the Rust
/// <c>cmremote_wire::desktop::signalling::IceCredentialType</c> enum;
/// variants serialise as <c>"Password"</c> / <c>"Oauth"</c>.
/// <para>
/// <c>Oauth</c> is reserved on the wire but the agent-side
/// <c>check_ice_server_config</c> guard fails closed on it until the
/// OAuth pipeline lands; the .NET side mirrors the wire shape so a
/// future server-side toggle can reach the Rust agent without a
/// further wire-protocol bump.
/// </para>
/// </summary>
[JsonConverter(typeof(JsonStringEnumConverter))]
public enum IceCredentialType
{
    /// <summary>Shared-secret / REST-issued password (default).</summary>
    Password = 0,

    /// <summary>OAuth bearer token. Reserved — the agent fails closed.</summary>
    Oauth = 1,
}
