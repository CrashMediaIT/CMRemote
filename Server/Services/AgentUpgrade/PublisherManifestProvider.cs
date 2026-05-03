using System.Collections.Concurrent;
using System.Net;
using System.Net.Http;
using Microsoft.Extensions.Options;
using Remotely.Shared.Services;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Configuration for <see cref="ManifestBackedAgentUpgradeDispatcher"/>.
/// Bound from the <c>AgentUpgrade</c> configuration section so the same
/// section drives both the orchestrator's tunables and the dispatcher's
/// manifest URLs.
/// </summary>
public class AgentUpgradeManifestOptions
{
    /// <summary>Configuration section: <c>AgentUpgrade</c>.</summary>
    public const string SectionName = "AgentUpgrade";

    /// <summary>
    /// Per-channel manifest sources. The key is the channel name
    /// (<c>stable</c> / <c>preview</c> / <c>previous</c>) and the value
    /// is either an absolute URL (<c>https://…/publisher-manifest.json</c>)
    /// or an absolute filesystem path. When unset for a channel the
    /// dispatcher returns <c>null</c> from <c>ResolveTargetAsync</c>
    /// (i.e. behaves like the no-op).
    /// </summary>
    public Dictionary<string, string> ManifestUrls { get; set; } =
        new(StringComparer.OrdinalIgnoreCase);

    /// <summary>
    /// How long a successfully-fetched manifest is cached before the
    /// next refresh. Defaults to 5 minutes; increase for low-churn
    /// fleets, decrease for canary-style rollouts.
    /// </summary>
    public TimeSpan ManifestCacheLifetime { get; set; } = TimeSpan.FromMinutes(5);

    /// <summary>
    /// Default channel for devices that have not opted in to a specific
    /// channel. Defaults to <c>stable</c>.
    /// </summary>
    public string DefaultChannel { get; set; } = "stable";

    /// <summary>
    /// Legacy compatibility knob retained for existing configuration
    /// files. S5 close-out now makes signature metadata mandatory for
    /// agent-update dispatch regardless of this value because the Rust
    /// agent verifies the cosign bundle before installer handoff.
    /// </summary>
    public bool RequireSignature { get; set; }

    /// <summary>
    /// How often the dispatcher polls the agent-hub session cache for
    /// the device's <c>AgentVersion</c> to flip to the target version
    /// after pushing the upgrade. Defaults to 5 seconds. The outer
    /// <see cref="AgentUpgradeOrchestratorOptions.DispatchTimeout"/>
    /// caps the total wait.
    /// </summary>
    public TimeSpan VersionWatchInterval { get; set; } = TimeSpan.FromSeconds(5);
}

/// <summary>
/// Loads + caches a <see cref="PublisherManifest"/> per configured
/// channel. Backed by either an HTTP fetch or a filesystem read so the
/// production deployment can point at a CDN URL while a local dev
/// deployment can point at a checked-in <c>publisher-manifest.json</c>.
/// </summary>
public interface IPublisherManifestProvider
{
    /// <summary>
    /// Resolves the manifest for the named channel. Returns <c>null</c>
    /// when no source URL is configured for the channel, when the fetch
    /// fails, or when the manifest fails the trust rules in
    /// <see cref="PublisherManifestParser"/>. Errors are logged with the
    /// channel name; never throws.
    /// </summary>
    Task<PublisherManifest?> GetAsync(string channel, CancellationToken cancellationToken);
}

/// <summary>
/// Default <see cref="IPublisherManifestProvider"/> backed by an
/// <see cref="HttpClient"/> for <c>http(s)://</c> sources and a direct
/// filesystem read for absolute paths. Caches the parsed manifest
/// per-channel for <see cref="AgentUpgradeManifestOptions.ManifestCacheLifetime"/>
/// to keep the orchestrator's per-sweep cost bounded under a fleet of
/// thousands of devices.
/// </summary>
public class PublisherManifestProvider : IPublisherManifestProvider
{
    private readonly IHttpClientFactory _httpClientFactory;
    private readonly AgentUpgradeManifestOptions _options;
    private readonly ILogger<PublisherManifestProvider> _logger;
    private readonly ISystemTime _systemTime;

    private readonly ConcurrentDictionary<string, CacheEntry> _cache =
        new(StringComparer.OrdinalIgnoreCase);

    public PublisherManifestProvider(
        IHttpClientFactory httpClientFactory,
        IOptions<AgentUpgradeManifestOptions> options,
        ISystemTime systemTime,
        ILogger<PublisherManifestProvider> logger)
    {
        _httpClientFactory = httpClientFactory;
        _options = options.Value;
        _systemTime = systemTime;
        _logger = logger;
    }

    public async Task<PublisherManifest?> GetAsync(string channel, CancellationToken cancellationToken)
    {
        if (string.IsNullOrWhiteSpace(channel))
        {
            return null;
        }

        if (_cache.TryGetValue(channel, out var entry) &&
            entry.ExpiresAt > _systemTime.Now)
        {
            return entry.Manifest;
        }

        if (!_options.ManifestUrls.TryGetValue(channel, out var source) ||
            string.IsNullOrWhiteSpace(source))
        {
            _logger.LogDebug(
                "No manifest URL configured for channel '{channel}'; dispatcher will return no target.",
                channel);
            return null;
        }

        string? body = null;
        try
        {
            if (Uri.TryCreate(source, UriKind.Absolute, out var uri) &&
                (uri.Scheme == Uri.UriSchemeHttp || uri.Scheme == Uri.UriSchemeHttps))
            {
                using var client = _httpClientFactory.CreateClient(nameof(PublisherManifestProvider));
                using var response = await client.GetAsync(uri, cancellationToken);
                if (response.StatusCode == HttpStatusCode.OK)
                {
                    body = await response.Content.ReadAsStringAsync(cancellationToken);
                }
                else
                {
                    _logger.LogWarning(
                        "Manifest fetch for channel '{channel}' returned HTTP {status}.",
                        channel, (int)response.StatusCode);
                }
            }
            else if (Path.IsPathRooted(source) && File.Exists(source))
            {
                body = await File.ReadAllTextAsync(source, cancellationToken);
            }
            else
            {
                _logger.LogWarning(
                    "Manifest source for channel '{channel}' is neither an http(s) URL nor an existing absolute path: '{source}'.",
                    channel, source);
            }
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex,
                "Manifest fetch for channel '{channel}' from '{source}' failed.",
                channel, source);
        }

        if (body is null)
        {
            return null;
        }

        var parsed = PublisherManifestParser.Parse(body);
        if (!parsed.IsSuccess)
        {
            _logger.LogWarning(
                "Manifest for channel '{channel}' failed validation: {error} ({detail}).",
                channel, parsed.Error, parsed.ErrorDetail);
            return null;
        }

        // Refuse a manifest whose channel field disagrees with the
        // channel we asked for — that would be a misconfiguration that
        // could silently route preview builds to stable devices.
        if (!string.Equals(parsed.Manifest!.Channel, channel, StringComparison.Ordinal))
        {
            _logger.LogWarning(
                "Manifest at '{source}' declares channel '{actual}' but was loaded as channel '{requested}'; refusing.",
                source, parsed.Manifest.Channel, channel);
            return null;
        }

        _cache[channel] = new CacheEntry(
            parsed.Manifest,
            _systemTime.Now.Add(_options.ManifestCacheLifetime));

        return parsed.Manifest;
    }

    private sealed record CacheEntry(PublisherManifest Manifest, DateTimeOffset ExpiresAt);
}
