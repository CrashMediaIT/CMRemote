using Microsoft.AspNetCore.SignalR;
using Microsoft.Extensions.Options;
using Remotely.Server.Data;
using Remotely.Server.Hubs;
using Remotely.Server.Services;
using Remotely.Shared.Entities;
using Remotely.Shared.Interfaces;

namespace Remotely.Server.Services.AgentUpgrade;

/// <summary>
/// Manifest-backed <see cref="IAgentUpgradeDispatcher"/>. Replaces the
/// <see cref="NoopAgentUpgradeDispatcher"/> default once the deployment
/// has a publisher manifest (slice R8) it can point at.
///
/// <para>Resolution algorithm — see <c>docs/publisher-manifest.md</c>
/// § "Routing":</para>
/// <list type="number">
///   <item>Look up the device row to derive its
///         <c>(target, format, currentVersion)</c> tuple.</item>
///   <item>Fetch the manifest for the device's channel via
///         <see cref="IPublisherManifestProvider"/>.</item>
///   <item>Pick the unique entry whose <c>target</c> + <c>format</c>
///         match. None or multiple → return null with a warning log.</item>
///   <item>If the entry's version equals the device's current version,
///         return null (already on target).</item>
///   <item>Optionally enforce <see cref="AgentUpgradeManifestOptions.RequireSignature"/>:
///         skip entries without a cosign bundle field.</item>
/// </list>
///
/// <para>Dispatch verifies the SHA-256 announced by the manifest against
/// the bytes the agent receives (the agent re-checks on its side too —
/// belt and braces). The dispatch path itself is intentionally simple:
/// the agent receives the resolved download URL via the existing M3
/// <c>InstallAgentUpdate</c> hub method (added with this slice) and
/// drives the download / install / restart sequence locally.</para>
/// </summary>
public class ManifestBackedAgentUpgradeDispatcher : IAgentUpgradeDispatcher
{
    private readonly IPublisherManifestProvider _manifestProvider;
    private readonly IAppDbFactory _dbFactory;
    private readonly IAgentHubSessionCache _sessionCache;
    private readonly IHubContext<AgentHub, IAgentHubClient> _agentHub;
    private readonly AgentUpgradeManifestOptions _options;
    private readonly ILogger<ManifestBackedAgentUpgradeDispatcher> _logger;

    public ManifestBackedAgentUpgradeDispatcher(
        IPublisherManifestProvider manifestProvider,
        IAppDbFactory dbFactory,
        IAgentHubSessionCache sessionCache,
        IHubContext<AgentHub, IAgentHubClient> agentHub,
        IOptions<AgentUpgradeManifestOptions> options,
        ILogger<ManifestBackedAgentUpgradeDispatcher> logger)
    {
        _manifestProvider = manifestProvider;
        _dbFactory = dbFactory;
        _sessionCache = sessionCache;
        _agentHub = agentHub;
        _options = options.Value;
        _logger = logger;
    }

    public async Task<AgentUpgradeTarget?> ResolveTargetAsync(
        AgentUpgradeStatus status,
        CancellationToken cancellationToken)
    {
        if (status is null || string.IsNullOrWhiteSpace(status.DeviceId))
        {
            return null;
        }

        // Snapshot the device's platform / architecture / current
        // version so we can route. A device that's never been seen
        // (no row) is left in Pending — the on-connect path in
        // AgentHub will enrol it on first heartbeat.
        Device? device;
        using (var db = _dbFactory.GetContext())
        {
            device = await db.Devices.FindAsync(new object[] { status.DeviceId }, cancellationToken);
        }

        if (device is null)
        {
            _logger.LogDebug(
                "ResolveTarget: no device row for DeviceId={deviceId}; leaving pending.",
                status.DeviceId);
            return null;
        }

        var routing = AgentTargetRouting.Resolve(device);
        if (routing is null)
        {
            _logger.LogDebug(
                "ResolveTarget: cannot derive (target,format) from Platform='{platform}' OSArchitecture={arch}; leaving pending.",
                device.Platform, device.OSArchitecture);
            return null;
        }

        var channel = string.IsNullOrWhiteSpace(_options.DefaultChannel)
            ? "stable"
            : _options.DefaultChannel;

        var manifest = await _manifestProvider.GetAsync(channel, cancellationToken);
        if (manifest is null)
        {
            return null;
        }

        // Match the unique (target, format) entry. The parser already
        // rejected unsafe file names, mismatched agent versions, and
        // bad SHA-256s, so nothing here needs to re-check those.
        var matches = manifest.Builds
            .Where(b => string.Equals(b.Target, routing.Value.Target, StringComparison.Ordinal) &&
                        string.Equals(b.Format, routing.Value.Format, StringComparison.Ordinal))
            .ToList();

        if (matches.Count == 0)
        {
            _logger.LogDebug(
                "ResolveTarget: no manifest entry matches Target={target} Format={format}; leaving pending.",
                routing.Value.Target, routing.Value.Format);
            return null;
        }

        if (matches.Count > 1)
        {
            _logger.LogWarning(
                "ResolveTarget: manifest for channel '{channel}' has {count} entries matching Target={target} Format={format}; refusing to guess.",
                channel, matches.Count, routing.Value.Target, routing.Value.Format);
            return null;
        }

        var build = matches[0];

        if (_options.RequireSignature &&
            (string.IsNullOrEmpty(build.Signature) || string.IsNullOrEmpty(build.SignedBy)))
        {
            _logger.LogWarning(
                "ResolveTarget: manifest entry for Target={target} Format={format} has no signature, but RequireSignature=true.",
                routing.Value.Target, routing.Value.Format);
            return null;
        }

        // Already on target version → nothing to do.
        if (!string.IsNullOrWhiteSpace(device.AgentVersion) &&
            string.Equals(device.AgentVersion, build.AgentVersion, StringComparison.Ordinal))
        {
            return null;
        }

        // Resolve the relative `file` against the manifest URL so the
        // agent gets an absolute download URL.
        var downloadUri = ResolveDownloadUri(channel, build.File);
        if (downloadUri is null)
        {
            return null;
        }

        return new AgentUpgradeTarget(build.AgentVersion, build.Sha256, downloadUri);
    }

    public async Task<AgentUpgradeDispatchResult> DispatchAsync(
        AgentUpgradeStatus status,
        AgentUpgradeTarget target,
        CancellationToken cancellationToken)
    {
        if (status is null || string.IsNullOrWhiteSpace(status.DeviceId))
        {
            return AgentUpgradeDispatchResult.Fail("Status has no DeviceId.");
        }

        // Defence in depth — the resolver should have caught these but
        // we re-check so a misbehaving caller cannot push an unverified
        // URL into the agent.
        if (target is null ||
            string.IsNullOrWhiteSpace(target.Version) ||
            string.IsNullOrWhiteSpace(target.Sha256) ||
            target.DownloadUri is null)
        {
            return AgentUpgradeDispatchResult.Fail("Resolved target is incomplete.");
        }

        // Refuse anything that didn't come back as an absolute https URL —
        // the agent must not trust an http URL for a binary it's about to
        // install with elevated privileges.
        if (!target.DownloadUri.IsAbsoluteUri ||
            (target.DownloadUri.Scheme != Uri.UriSchemeHttps &&
             target.DownloadUri.Scheme != Uri.UriSchemeFile))
        {
            return AgentUpgradeDispatchResult.Fail(
                $"Refusing to dispatch upgrade — download URI scheme '{target.DownloadUri.Scheme}' is not allowed.");
        }

        if (!_sessionCache.TryGetConnectionId(status.DeviceId, out var connectionId))
        {
            // Device offline — the orchestrator's on-connect path will
            // requeue the row when the device next handshakes, so this
            // is a recoverable failure.
            return AgentUpgradeDispatchResult.Fail("Device is offline; will retry when it reconnects.");
        }

        try
        {
            await _agentHub.Clients.Client(connectionId).InstallAgentUpdate(
                target.DownloadUri.ToString(),
                target.Version,
                target.Sha256);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex,
                "Pushing InstallAgentUpdate to DeviceId={deviceId} (ConnectionId={connectionId}) failed.",
                status.DeviceId, connectionId);
            return AgentUpgradeDispatchResult.Fail($"InstallAgentUpdate hub call failed: {ex.Message}");
        }

        // Wait for the device's heartbeat to report the new AgentVersion.
        // The orchestrator gives us its DispatchTimeout via the
        // CancellationToken, so we just poll until the cache flips or
        // the token cancels.
        var pollInterval = _options.VersionWatchInterval > TimeSpan.Zero
            ? _options.VersionWatchInterval
            : TimeSpan.FromSeconds(5);

        try
        {
            while (true)
            {
                cancellationToken.ThrowIfCancellationRequested();

                if (_sessionCache.TryGetByDeviceId(status.DeviceId, out var device) &&
                    !string.IsNullOrWhiteSpace(device.AgentVersion) &&
                    string.Equals(device.AgentVersion, target.Version, StringComparison.Ordinal))
                {
                    _logger.LogInformation(
                        "Agent upgrade succeeded for DeviceId={deviceId} → version {version}.",
                        status.DeviceId, target.Version);
                    return AgentUpgradeDispatchResult.Ok();
                }

                await Task.Delay(pollInterval, cancellationToken);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Translated by the orchestrator into either the
            // dispatch-timeout failure or the host-shutdown rethrow.
            throw;
        }
    }

    private Uri? ResolveDownloadUri(string channel, string file)
    {
        if (!_options.ManifestUrls.TryGetValue(channel, out var source) ||
            string.IsNullOrWhiteSpace(source))
        {
            return null;
        }

        // For an http(s) manifest URL, replace the last path segment
        // with the file name. For a filesystem manifest path, return a
        // file:// URL pointing next to it; this lets the Linux-only dev
        // smoke test work without a CDN.
        if (Uri.TryCreate(source, UriKind.Absolute, out var manifestUri))
        {
            var leftPart = manifestUri.GetLeftPart(UriPartial.Path);
            var lastSlash = leftPart.LastIndexOf('/');
            if (lastSlash <= 0)
            {
                return null;
            }
            var basePart = leftPart[..(lastSlash + 1)];
            if (Uri.TryCreate(basePart + file, UriKind.Absolute, out var downloadUri))
            {
                return downloadUri;
            }
        }

        return null;
    }

    /// <summary>
    /// Maps a <see cref="Device"/>'s reported platform / architecture
    /// to the Rust-style target triple + package format used by the
    /// publisher manifest. Public so tests can pin the rules.
    /// </summary>
    public static class AgentTargetRouting
    {
        // Linux-distro families. These match the substrings the Rust
        // agent emits for `Platform` (e.g. "Linux/Ubuntu 22.04",
        // "Linux/Fedora 39"). Unknown distros fall back to .deb because
        // the M4 dashboard reports it and an operator can flip the
        // device's effective channel manually.
        private static readonly string[] _rpmDistros =
        {
            "fedora", "rhel", "redhat", "centos", "rocky", "alma",
            "opensuse", "suse",
        };

        public static (string Target, string Format)? Resolve(Device device)
        {
            if (device is null || string.IsNullOrWhiteSpace(device.Platform))
            {
                return null;
            }

            var platform = device.Platform.ToLowerInvariant();
            var arch = device.OSArchitecture switch
            {
                System.Runtime.InteropServices.Architecture.X64 => "x86_64",
                System.Runtime.InteropServices.Architecture.Arm64 => "aarch64",
                _ => null,
            };
            if (arch is null)
            {
                return null;
            }

            if (platform.Contains("windows", StringComparison.Ordinal))
            {
                // The agent ships a single MSI; arm64 Windows is on the
                // R8 follow-up. Refuse arm64 explicitly so a misrouted
                // dispatch is observable in the dashboard.
                if (arch != "x86_64")
                {
                    return null;
                }
                return ("x86_64-pc-windows-msvc", "msi");
            }

            if (platform.Contains("darwin", StringComparison.Ordinal) ||
                platform.Contains("macos", StringComparison.Ordinal) ||
                platform.Contains("osx", StringComparison.Ordinal))
            {
                return ("universal2-apple-darwin", "pkg");
            }

            if (platform.Contains("linux", StringComparison.Ordinal))
            {
                var format = "deb";
                foreach (var distro in _rpmDistros)
                {
                    if (platform.Contains(distro, StringComparison.Ordinal))
                    {
                        format = "rpm";
                        break;
                    }
                }
                return ($"{arch}-unknown-linux-gnu", format);
            }

            return null;
        }
    }
}
