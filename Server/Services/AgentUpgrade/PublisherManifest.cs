using System.Text.Json;
using System.Text.Json.Serialization;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Parsed in-memory representation of a CMRemote publisher manifest
/// (see <c>docs/publisher-manifest.md</c>). Consumed by the M3
/// <see cref="ManifestBackedAgentUpgradeDispatcher"/> to resolve which
/// build a device should be upgraded to and to verify the SHA-256 of
/// the artifact bytes before dispatch.
///
/// <para>The shape mirrors the JSON schema at
/// <c>docs/publisher-manifest.schema.json</c>. Unknown top-level fields
/// are ignored on read so the consumer is forward-compatible with
/// non-breaking additions.</para>
/// </summary>
public sealed class PublisherManifest
{
    /// <summary>
    /// Major schema version. Consumers MUST refuse a manifest whose
    /// schema version they do not recognise.
    /// </summary>
    [JsonPropertyName("schemaVersion")]
    public int SchemaVersion { get; init; }

    [JsonPropertyName("publisher")]
    public string Publisher { get; init; } = string.Empty;

    [JsonPropertyName("generatedAt")]
    public DateTimeOffset GeneratedAt { get; init; }

    [JsonPropertyName("channel")]
    public string Channel { get; init; } = string.Empty;

    [JsonPropertyName("version")]
    public string Version { get; init; } = string.Empty;

    [JsonPropertyName("notes")]
    public string? Notes { get; init; }

    [JsonPropertyName("builds")]
    public IReadOnlyList<PublisherManifestBuild> Builds { get; init; }
        = Array.Empty<PublisherManifestBuild>();
}

/// <summary>
/// One <c>builds[]</c> entry on a <see cref="PublisherManifest"/>.
/// </summary>
public sealed class PublisherManifestBuild
{
    [JsonPropertyName("agentVersion")]
    public string AgentVersion { get; init; } = string.Empty;

    [JsonPropertyName("target")]
    public string Target { get; init; } = string.Empty;

    [JsonPropertyName("format")]
    public string Format { get; init; } = string.Empty;

    [JsonPropertyName("file")]
    public string File { get; init; } = string.Empty;

    [JsonPropertyName("size")]
    public long Size { get; init; }

    [JsonPropertyName("sha256")]
    public string Sha256 { get; init; } = string.Empty;

    [JsonPropertyName("signature")]
    public string? Signature { get; init; }

    [JsonPropertyName("signedBy")]
    public string? SignedBy { get; init; }
}

/// <summary>
/// Failure modes for <see cref="PublisherManifestParser.Parse"/>.
/// </summary>
public enum PublisherManifestParseError
{
    None = 0,
    InvalidJson,
    UnsupportedSchemaVersion,
    MissingPublisher,
    MissingChannel,
    InvalidChannel,
    MissingVersion,
    InvalidVersion,
    InvalidBuildEntry,
}

/// <summary>
/// Parses + validates a <see cref="PublisherManifest"/>. The parser
/// holds the trust rules from <c>docs/publisher-manifest.md</c> §
/// <i>Trust rules</i> so they are one decision rather than scattered
/// checks: every consumer that wants a manifest goes through here.
/// </summary>
public static class PublisherManifestParser
{
    /// <summary>
    /// Schema version this build understands. Major increments are
    /// breaking — we refuse to parse a higher major version because we
    /// might silently mis-route a build whose semantics changed.
    /// </summary>
    public const int SupportedSchemaVersion = 1;

    private static readonly HashSet<string> _allowedChannels = new(StringComparer.Ordinal)
    {
        "stable", "preview", "previous",
    };

    // SemVer 2.0.0 (Backus-Naur translated to a regex). Pre-release and
    // build-metadata segments are accepted; an empty string or an
    // out-of-spec input fails closed. Mirrors the regex in the JSON
    // schema so the parser and the schema agree.
    private static readonly System.Text.RegularExpressions.Regex _semverRegex = new(
        @"^\d+\.\d+\.\d+(-[0-9A-Za-z\.-]+)?(\+[0-9A-Za-z\.-]+)?$",
        System.Text.RegularExpressions.RegexOptions.Compiled);

    private static readonly System.Text.RegularExpressions.Regex _safeFileRegex = new(
        @"^[A-Za-z0-9._-]+$",
        System.Text.RegularExpressions.RegexOptions.Compiled);

    private static readonly System.Text.RegularExpressions.Regex _sha256Regex = new(
        @"^[0-9a-f]{64}$",
        System.Text.RegularExpressions.RegexOptions.Compiled);

    private static readonly JsonSerializerOptions _options = new()
    {
        PropertyNameCaseInsensitive = false,
        ReadCommentHandling = JsonCommentHandling.Skip,
        AllowTrailingCommas = false,
    };

    /// <summary>
    /// Result of <see cref="Parse"/>. <see cref="Manifest"/> is non-null
    /// when <see cref="Error"/> is <see cref="PublisherManifestParseError.None"/>.
    /// </summary>
    public sealed record ParseResult(
        PublisherManifestParseError Error,
        string? ErrorDetail,
        PublisherManifest? Manifest)
    {
        public bool IsSuccess => Error == PublisherManifestParseError.None && Manifest is not null;
    }

    public static ParseResult Parse(string json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            return new ParseResult(
                PublisherManifestParseError.InvalidJson,
                "Manifest JSON is empty.",
                null);
        }

        PublisherManifest? raw;
        try
        {
            raw = JsonSerializer.Deserialize<PublisherManifest>(json, _options);
        }
        catch (JsonException ex)
        {
            return new ParseResult(
                PublisherManifestParseError.InvalidJson,
                ex.Message,
                null);
        }

        if (raw is null)
        {
            return new ParseResult(
                PublisherManifestParseError.InvalidJson,
                "Manifest JSON deserialised to null.",
                null);
        }

        if (raw.SchemaVersion != SupportedSchemaVersion)
        {
            return new ParseResult(
                PublisherManifestParseError.UnsupportedSchemaVersion,
                $"Manifest schemaVersion is {raw.SchemaVersion}; only {SupportedSchemaVersion} is supported.",
                null);
        }

        if (string.IsNullOrWhiteSpace(raw.Publisher))
        {
            return new ParseResult(
                PublisherManifestParseError.MissingPublisher,
                "Manifest 'publisher' is required.",
                null);
        }

        if (string.IsNullOrWhiteSpace(raw.Channel))
        {
            return new ParseResult(
                PublisherManifestParseError.MissingChannel,
                "Manifest 'channel' is required.",
                null);
        }

        if (!_allowedChannels.Contains(raw.Channel))
        {
            return new ParseResult(
                PublisherManifestParseError.InvalidChannel,
                $"Manifest 'channel' must be one of stable/preview/previous, got '{raw.Channel}'.",
                null);
        }

        if (string.IsNullOrWhiteSpace(raw.Version))
        {
            return new ParseResult(
                PublisherManifestParseError.MissingVersion,
                "Manifest 'version' is required.",
                null);
        }

        if (!_semverRegex.IsMatch(raw.Version))
        {
            return new ParseResult(
                PublisherManifestParseError.InvalidVersion,
                $"Manifest 'version' is not a valid SemVer 2.0.0 string: '{raw.Version}'.",
                null);
        }

        // Validate every build entry up-front. An invalid entry fails
        // the whole manifest because we can't safely route around it
        // (the dispatcher's "unique entry per (target,format)" rule is
        // a manifest-level invariant).
        for (int i = 0; i < raw.Builds.Count; i++)
        {
            var b = raw.Builds[i];
            var detail = ValidateBuild(raw.Version, b);
            if (detail is not null)
            {
                return new ParseResult(
                    PublisherManifestParseError.InvalidBuildEntry,
                    $"builds[{i}]: {detail}",
                    null);
            }
        }

        return new ParseResult(
            PublisherManifestParseError.None,
            null,
            raw);
    }

    private static string? ValidateBuild(string manifestVersion, PublisherManifestBuild b)
    {
        if (string.IsNullOrWhiteSpace(b.AgentVersion))
        {
            return "agentVersion is required.";
        }
        if (b.AgentVersion != manifestVersion)
        {
            return $"agentVersion '{b.AgentVersion}' must equal the manifest version '{manifestVersion}'.";
        }
        if (!_semverRegex.IsMatch(b.AgentVersion))
        {
            return $"agentVersion is not a valid SemVer 2.0.0 string: '{b.AgentVersion}'.";
        }
        if (string.IsNullOrWhiteSpace(b.Target))
        {
            return "target is required.";
        }
        if (string.IsNullOrWhiteSpace(b.Format))
        {
            return "format is required.";
        }
        if (string.IsNullOrWhiteSpace(b.File))
        {
            return "file is required.";
        }
        if (!_safeFileRegex.IsMatch(b.File) ||
            b.File.Contains("..", StringComparison.Ordinal))
        {
            return $"file '{b.File}' contains an unsafe character or '..' segment.";
        }
        if (b.Size <= 0)
        {
            return $"size must be > 0, got {b.Size}.";
        }
        if (string.IsNullOrWhiteSpace(b.Sha256))
        {
            return "sha256 is required.";
        }
        if (!_sha256Regex.IsMatch(b.Sha256))
        {
            return $"sha256 is not a 64-char lower-case hex string: '{b.Sha256}'.";
        }
        if (!string.IsNullOrEmpty(b.Signature) && string.IsNullOrEmpty(b.SignedBy))
        {
            return "signedBy is required when signature is present.";
        }
        return null;
    }

    /// <summary>
    /// Constant-time hex-string comparison. Used to verify a downloaded
    /// artifact's SHA-256 against the manifest entry without leaking
    /// timing information about which prefix matched.
    /// </summary>
    public static bool CtEqHex(string a, string b)
    {
        if (a is null || b is null)
        {
            return false;
        }
        if (a.Length != b.Length)
        {
            return false;
        }
        int diff = 0;
        for (int i = 0; i < a.Length; i++)
        {
            // Lower-case the inputs in constant time (within one byte)
            // so the comparison is canonical regardless of the caller's
            // hex case. The XOR keeps the loop branch-free.
            int ai = a[i] | 0x20;
            int bi = b[i] | 0x20;
            diff |= ai ^ bi;
        }
        return diff == 0;
    }
}
