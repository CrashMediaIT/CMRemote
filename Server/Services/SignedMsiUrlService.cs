using System.Buffers.Text;
using System.Text.Json;
using Microsoft.AspNetCore.DataProtection;
using Remotely.Shared.Services;

namespace Remotely.Server.Services;

/// <summary>
/// Mints + validates short-lived, device-scoped download tokens for the
/// UploadedMsi flow (ROADMAP.md "Track S / S7 — Runtime security
/// posture: signed download URLs ... with a short TTL and a
/// device-scoped HMAC").
///
/// <para>Replaces the in-memory random-secret approach in
/// <see cref="ExpiringTokenService"/> for the MSI-download path: the
/// token is a <see cref="IDataProtector"/>-protected envelope over
/// <c>{deviceId, sharedFileId, expiresAt}</c> so:</para>
/// <list type="bullet">
///   <item>The device id and the shared-file id are inside the
///         protected payload — a relying party reads them after MAC
///         verification, so a substituted device id or file id is a
///         hard failure (not a silent rebind).</item>
///   <item>Validation is offline (no shared cache lookup) so server
///         restarts or horizontal scale-out do not invalidate
///         outstanding tokens.</item>
///   <item>Tokens expire on the server's clock, not the client's, and
///         survive only as long as <c>ExpiresAt</c> says they do.</item>
/// </list>
///
/// <para>The protector purpose string pins the protector to this exact
/// use — a token minted for any other purpose (the existing CSRF /
/// Identity protectors in the same key-ring) cannot be replayed
/// against the MSI download endpoint.</para>
/// </summary>
public interface ISignedMsiUrlService
{
    /// <summary>
    /// Mints a token for <paramref name="deviceId"/> downloading
    /// <paramref name="sharedFileId"/>, valid until
    /// <paramref name="expiresAt"/>. Returns a URL-safe base64 string
    /// suitable for use as an <c>X-CMRemote-Msi-Token</c> header
    /// value.
    /// </summary>
    string MintToken(string deviceId, string sharedFileId, DateTimeOffset expiresAt);

    /// <summary>
    /// Verifies <paramref name="token"/> binds to
    /// <paramref name="expectedSharedFileId"/> and is not expired. On
    /// success returns the decrypted payload (so the caller knows the
    /// device the token was minted for); on failure returns
    /// <c>null</c>. Failure modes (malformed, MAC failure, file
    /// mismatch, expired) are indistinguishable to the caller — the
    /// service logs the actual cause but the caller only sees
    /// <c>null</c>.
    /// </summary>
    SignedMsiTokenPayload? Validate(string token, string expectedSharedFileId);
}

/// <summary>
/// Decrypted, MAC-verified contents of a signed-MSI token.
/// </summary>
public sealed record SignedMsiTokenPayload(string DeviceId, string SharedFileId, DateTimeOffset ExpiresAt);

public class SignedMsiUrlService : ISignedMsiUrlService
{
    /// <summary>
    /// DataProtection purpose string. Changing this value invalidates
    /// every outstanding signed-MSI token; treat it as a constant.
    /// </summary>
    internal const string ProtectorPurpose = "CMRemote.S7.SignedMsiUrl.v1";

    private readonly IDataProtector _protector;
    private readonly ISystemTime _systemTime;
    private readonly ILogger<SignedMsiUrlService> _logger;

    public SignedMsiUrlService(
        IDataProtectionProvider provider,
        ISystemTime systemTime,
        ILogger<SignedMsiUrlService> logger)
    {
        _protector = provider.CreateProtector(ProtectorPurpose);
        _systemTime = systemTime;
        _logger = logger;
    }

    public string MintToken(string deviceId, string sharedFileId, DateTimeOffset expiresAt)
    {
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            throw new ArgumentException("Device ID is required.", nameof(deviceId));
        }
        if (string.IsNullOrWhiteSpace(sharedFileId))
        {
            throw new ArgumentException("Shared file ID is required.", nameof(sharedFileId));
        }

        var payload = new SignedMsiTokenPayload(deviceId, sharedFileId, expiresAt);
        var json = JsonSerializer.SerializeToUtf8Bytes(payload);
        var protectedBytes = _protector.Protect(json);
        return Base64UrlEncode(protectedBytes);
    }

    public SignedMsiTokenPayload? Validate(string token, string expectedSharedFileId)
    {
        if (string.IsNullOrWhiteSpace(token) ||
            string.IsNullOrWhiteSpace(expectedSharedFileId))
        {
            return null;
        }

        byte[] protectedBytes;
        try
        {
            protectedBytes = Base64UrlDecode(token);
        }
        catch (FormatException)
        {
            _logger.LogDebug("Signed MSI token rejected: malformed base64.");
            return null;
        }

        byte[] unprotected;
        try
        {
            unprotected = _protector.Unprotect(protectedBytes);
        }
        catch (System.Security.Cryptography.CryptographicException)
        {
            // MAC failure — caller never gets to know which check
            // failed (timing-equivalent + log-only).
            _logger.LogDebug("Signed MSI token rejected: MAC verification failed.");
            return null;
        }

        SignedMsiTokenPayload? payload;
        try
        {
            payload = JsonSerializer.Deserialize<SignedMsiTokenPayload>(unprotected);
        }
        catch (JsonException)
        {
            _logger.LogDebug("Signed MSI token rejected: payload not valid JSON.");
            return null;
        }

        if (payload is null ||
            string.IsNullOrEmpty(payload.DeviceId) ||
            string.IsNullOrEmpty(payload.SharedFileId))
        {
            return null;
        }

        if (!string.Equals(payload.SharedFileId, expectedSharedFileId, StringComparison.Ordinal))
        {
            _logger.LogWarning(
                "Signed MSI token rejected: shared-file-id mismatch (token={tokenFile} expected={expectedFile}).",
                payload.SharedFileId, expectedSharedFileId);
            return null;
        }

        if (payload.ExpiresAt <= _systemTime.Now)
        {
            _logger.LogDebug(
                "Signed MSI token rejected: expired at {expiresAt} (now={now}).",
                payload.ExpiresAt, _systemTime.Now);
            return null;
        }

        return payload;
    }

    /// <summary>
    /// URL-safe base64 (no padding, '-' / '_' alphabet). Inlined to
    /// avoid pulling a new dependency for one routine.
    /// </summary>
    internal static string Base64UrlEncode(byte[] bytes)
    {
        var s = Convert.ToBase64String(bytes);
        return s.TrimEnd('=').Replace('+', '-').Replace('/', '_');
    }

    internal static byte[] Base64UrlDecode(string token)
    {
        var s = token.Replace('-', '+').Replace('_', '/');
        switch (s.Length % 4)
        {
            case 2: s += "=="; break;
            case 3: s += "="; break;
            case 1: throw new FormatException("Invalid base64url length.");
        }
        return Convert.FromBase64String(s);
    }
}
