using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Services;
using System;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Http;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using MsOptions = Microsoft.Extensions.Options.Options;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests for <see cref="PublisherManifestProvider"/>. Exercises the
/// filesystem source (the dev-loop path) and the cache TTL; the HTTP
/// path is exercised through <see cref="StubHttpMessageHandler"/> so we
/// do not need a real network for the channel-mismatch refusal.
/// </summary>
[TestClass]
public class PublisherManifestProviderTests
{
    private string _tempDir = null!;
    private SystemTime _systemTime = null!;

    [TestInitialize]
    public void Init()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "cmremote-pm-tests-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_tempDir);
        _systemTime = new SystemTime();
        _systemTime.Set(new DateTimeOffset(2026, 4, 24, 0, 0, 0, TimeSpan.Zero));
    }

    [TestCleanup]
    public void Cleanup()
    {
        try { Directory.Delete(_tempDir, recursive: true); }
        catch { /* best-effort */ }
    }

    private string WriteManifest(string fileName, string channel, string version = "2.0.0", int schemaVersion = 1)
    {
        var path = Path.Combine(_tempDir, fileName);
        var json = JsonSerializer.Serialize(new
        {
            schemaVersion,
            publisher = "CrashMedia IT",
            generatedAt = "2026-04-24T00:00:00Z",
            channel,
            version,
            builds = new[]
            {
                new
                {
                    agentVersion = version,
                    target = "x86_64-unknown-linux-gnu",
                    format = "deb",
                    file = "cmremote-agent.deb",
                    size = 12345L,
                    sha256 = new string('a', 64),
                    signature = "cmremote-agent.deb.cosign.bundle",
                    signedBy = "https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/v2.0.0",
                },
            },
        });
        File.WriteAllText(path, json);
        return path;
    }

    private PublisherManifestProvider NewProvider(
        Dictionary<string, string> manifestUrls,
        TimeSpan? cacheLifetime = null,
        IHttpClientFactory? httpClientFactory = null)
    {
        var opts = MsOptions.Create(new AgentUpgradeManifestOptions
        {
            ManifestUrls = manifestUrls,
            ManifestCacheLifetime = cacheLifetime ?? TimeSpan.FromMinutes(5),
        });
        return new PublisherManifestProvider(
            httpClientFactory ?? new SingleClientFactory(new StubHttpMessageHandler((_, _) =>
            {
                throw new InvalidOperationException("HTTP path was not expected to fire in this test.");
            })),
            opts,
            _systemTime,
            NullLogger<PublisherManifestProvider>.Instance);
    }

    // ---- Filesystem source ----

    [TestMethod]
    public async Task Get_FilesystemSource_HappyPath()
    {
        var path = WriteManifest("publisher-manifest.json", "stable");
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["stable"] = path,
        });

        var manifest = await provider.GetAsync("stable", CancellationToken.None);
        Assert.IsNotNull(manifest);
        Assert.AreEqual("stable", manifest!.Channel);
        Assert.AreEqual(1, manifest.Builds.Count);
    }

    [TestMethod]
    public async Task Get_NoUrlForChannel_ReturnsNull()
    {
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase));
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    [TestMethod]
    public async Task Get_BlankChannel_ReturnsNull()
    {
        var path = WriteManifest("publisher-manifest.json", "stable");
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["stable"] = path,
        });
        Assert.IsNull(await provider.GetAsync("", CancellationToken.None));
    }

    [TestMethod]
    public async Task Get_MissingFile_ReturnsNull()
    {
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["stable"] = Path.Combine(_tempDir, "not-there.json"),
        });
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    [TestMethod]
    public async Task Get_InvalidManifest_ReturnsNull()
    {
        var path = Path.Combine(_tempDir, "publisher-manifest.json");
        File.WriteAllText(path, "{ this is not valid json }");
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["stable"] = path,
        });
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    [TestMethod]
    public async Task Get_UnsupportedSchemaVersion_ReturnsNull()
    {
        var path = WriteManifest("publisher-manifest.json", "stable", schemaVersion: 99);
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["stable"] = path,
        });
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    [TestMethod]
    public async Task Get_ChannelMismatch_RefusesAndReturnsNull()
    {
        // Manifest declares "preview" but is loaded as "stable" — must
        // refuse so a misconfigured deployment cannot route preview
        // builds to stable devices.
        var path = WriteManifest("publisher-manifest.json", "preview");
        var provider = NewProvider(new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["stable"] = path,
        });
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    // ---- Cache behaviour ----

    [TestMethod]
    public async Task Get_CachesWithinLifetime()
    {
        var path = WriteManifest("publisher-manifest.json", "stable", version: "2.0.0");
        var provider = NewProvider(
            new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase) { ["stable"] = path },
            cacheLifetime: TimeSpan.FromMinutes(5));

        var first = await provider.GetAsync("stable", CancellationToken.None);
        Assert.IsNotNull(first);
        Assert.AreEqual("2.0.0", first!.Version);

        // Mutate the file on disk. Within the cache lifetime the
        // provider must serve the cached parse, not re-read.
        WriteManifest("publisher-manifest.json", "stable", version: "9.9.9");
        var second = await provider.GetAsync("stable", CancellationToken.None);
        Assert.AreEqual("2.0.0", second!.Version, "Cache must serve the original parse within the TTL.");
    }

    [TestMethod]
    public async Task Get_RefreshesAfterLifetimeExpires()
    {
        var path = WriteManifest("publisher-manifest.json", "stable", version: "2.0.0");
        var provider = NewProvider(
            new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase) { ["stable"] = path },
            cacheLifetime: TimeSpan.FromMinutes(5));

        var first = await provider.GetAsync("stable", CancellationToken.None);
        Assert.AreEqual("2.0.0", first!.Version);

        WriteManifest("publisher-manifest.json", "stable", version: "9.9.9");
        // Advance past the TTL. The next fetch must re-read the file.
        _systemTime.Offset(TimeSpan.FromMinutes(10));
        var refreshed = await provider.GetAsync("stable", CancellationToken.None);
        Assert.AreEqual("9.9.9", refreshed!.Version);
    }

    // ---- HTTP source ----

    [TestMethod]
    public async Task Get_HttpSource_HappyPath()
    {
        var json = JsonSerializer.Serialize(new
        {
            schemaVersion = 1,
            publisher = "CrashMedia IT",
            generatedAt = "2026-04-24T00:00:00Z",
            channel = "stable",
            version = "2.0.0",
            builds = new[]
            {
                new
                {
                    agentVersion = "2.0.0",
                    target = "x86_64-unknown-linux-gnu",
                    format = "deb",
                    file = "cmremote-agent.deb",
                    size = 12345L,
                    sha256 = new string('a', 64),
                    signature = "cmremote-agent.deb.cosign.bundle",
                    signedBy = "https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/v2.0.0",
                },
            },
        });
        var handler = new StubHttpMessageHandler((_, _) =>
            new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(json),
            });
        var provider = NewProvider(
            new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["stable"] = "https://cdn.example.com/cmremote/stable/publisher-manifest.json",
            },
            httpClientFactory: new SingleClientFactory(handler));

        var manifest = await provider.GetAsync("stable", CancellationToken.None);
        Assert.IsNotNull(manifest);
        Assert.AreEqual("2.0.0", manifest!.Version);
    }

    [TestMethod]
    public async Task Get_HttpSource_Non200_ReturnsNull()
    {
        var handler = new StubHttpMessageHandler((_, _) =>
            new HttpResponseMessage(HttpStatusCode.NotFound));
        var provider = NewProvider(
            new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["stable"] = "https://cdn.example.com/cmremote/stable/publisher-manifest.json",
            },
            httpClientFactory: new SingleClientFactory(handler));
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    [TestMethod]
    public async Task Get_HttpSource_NetworkException_ReturnsNullDoesNotThrow()
    {
        var handler = new StubHttpMessageHandler((_, _) =>
            throw new HttpRequestException("connection refused"));
        var provider = NewProvider(
            new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["stable"] = "https://cdn.example.com/cmremote/stable/publisher-manifest.json",
            },
            httpClientFactory: new SingleClientFactory(handler));
        Assert.IsNull(await provider.GetAsync("stable", CancellationToken.None));
    }

    /// <summary>
    /// Hand-rolled <see cref="HttpMessageHandler"/> that delegates to a
    /// caller-supplied function. Used in place of Moq + protected setup
    /// so the test reads top-down.
    /// </summary>
    private sealed class StubHttpMessageHandler : HttpMessageHandler
    {
        private readonly Func<HttpRequestMessage, CancellationToken, HttpResponseMessage> _send;

        public StubHttpMessageHandler(Func<HttpRequestMessage, CancellationToken, HttpResponseMessage> send)
        {
            _send = send;
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            Task.FromResult(_send(request, cancellationToken));
    }

    private sealed class SingleClientFactory : IHttpClientFactory
    {
        private readonly HttpMessageHandler _handler;
        public SingleClientFactory(HttpMessageHandler handler) { _handler = handler; }
        public HttpClient CreateClient(string name) => new(_handler, disposeHandler: false);
    }
}
