using System.Collections.Generic;
using Remotely.Shared.Enums;

namespace Remotely.Shared.Dtos;

/// <summary>
/// One STUN / TURN server entry in an
/// <see cref="IceServerConfigDto"/> (slice R7.i). .NET counterpart of
/// the Rust <c>cmremote_wire::desktop::signalling::IceServer</c> DTO.
/// <para>
/// The <see cref="Credential"/> field is sensitive — implementations
/// MUST NOT log, print, or echo it.
/// <see cref="Urls"/> MUST contain at least one entry; an empty list
/// is a malformed payload that the agent-side guard refuses.
/// </para>
/// </summary>
public class IceServerDto
{
    /// <summary>
    /// Maximum permitted length of any single ICE / TURN URL string.
    /// Mirrors the Rust constant <c>MAX_ICE_URL_LEN</c>.
    /// </summary>
    public const int MaxIceUrlLen = 512;

    /// <summary>
    /// Maximum permitted length of an
    /// <see cref="IceCredentialType.Password"/> credential string.
    /// Mirrors the Rust constant <c>MAX_ICE_CREDENTIAL_LEN</c>.
    /// </summary>
    public const int MaxIceCredentialLen = 512;

    /// <summary>
    /// Maximum number of <see cref="Urls"/> entries permitted per
    /// logical server. Mirrors the Rust constant
    /// <c>MAX_URLS_PER_ICE_SERVER</c>.
    /// </summary>
    public const int MaxUrlsPerIceServer = 4;

    /// <summary>
    /// One or more <c>stun:</c> / <c>stuns:</c> / <c>turn:</c> /
    /// <c>turns:</c> URLs that describe the same logical server.
    /// Length-capped per entry by <see cref="MaxIceUrlLen"/>; the list
    /// itself is bounded by <see cref="MaxUrlsPerIceServer"/>.
    /// </summary>
    public List<string> Urls { get; set; } = new();

    /// <summary>
    /// TURN username when <see cref="CredentialType"/> is
    /// <see cref="IceCredentialType.Password"/>. Absent for plain
    /// <c>stun:</c>.
    /// </summary>
    public string? Username { get; set; }

    /// <summary>
    /// <strong>Sensitive.</strong> TURN credential (shared secret or
    /// REST-issued password). Length-capped at
    /// <see cref="MaxIceCredentialLen"/>. Implementations MUST NOT
    /// log, print, or echo this value.
    /// </summary>
    public string? Credential { get; set; }

    /// <summary>
    /// How <see cref="Credential"/> should be interpreted. Defaults
    /// to <see cref="IceCredentialType.Password"/>.
    /// </summary>
    public IceCredentialType CredentialType { get; set; } = IceCredentialType.Password;
}
