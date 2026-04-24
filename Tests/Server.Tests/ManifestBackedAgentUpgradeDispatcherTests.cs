using Microsoft.AspNetCore.SignalR;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Moq;
using Remotely.Server.Data;
using Remotely.Server.Hubs;
using Remotely.Server.Services;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
using Remotely.Shared.Interfaces;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;
using MsOptions = Microsoft.Extensions.Options.Options;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests for <see cref="ManifestBackedAgentUpgradeDispatcher"/> and its
/// <see cref="ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting"/>
/// helper. Pins the routing rules, the multi-match refusal, the
/// already-on-target short-circuit, the RequireSignature gate, and the
/// download-URL resolver.
/// </summary>
[TestClass]
public class ManifestBackedAgentUpgradeDispatcherTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private FakeManifestProvider _provider = null!;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();
        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _provider = new FakeManifestProvider();
    }

    private ManifestBackedAgentUpgradeDispatcher NewDispatcher(
        Dictionary<string, string>? manifestUrls = null,
        bool requireSignature = false,
        string defaultChannel = "stable",
        IAgentHubSessionCache? sessionCache = null,
        FakeAgentHub? agentHub = null,
        TimeSpan? versionWatchInterval = null)
    {
        var opts = MsOptions.Create(new AgentUpgradeManifestOptions
        {
            ManifestUrls = manifestUrls ?? new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["stable"] = "https://cdn.example.com/cmremote/stable/publisher-manifest.json",
            },
            RequireSignature = requireSignature,
            DefaultChannel = defaultChannel,
            VersionWatchInterval = versionWatchInterval ?? TimeSpan.FromMilliseconds(20),
        });
        return new ManifestBackedAgentUpgradeDispatcher(
            _provider,
            _dbFactory,
            sessionCache ?? new AgentHubSessionCache(),
            (agentHub ?? new FakeAgentHub()).HubContext,
            opts,
            NullLogger<ManifestBackedAgentUpgradeDispatcher>.Instance);
    }

    private async Task SetDevicePlatformAsync(string deviceId, string platform, Architecture arch, string? agentVersion = null)
    {
        using var db = _dbFactory.GetContext();
        var device = await db.Devices.FirstAsync(d => d.ID == deviceId);
        device.Platform = platform;
        device.OSArchitecture = arch;
        device.AgentVersion = agentVersion;
        await db.SaveChangesAsync();
    }

    private static AgentUpgradeStatus StatusFor(string deviceId) =>
        new()
        {
            Id = Guid.NewGuid(),
            DeviceId = deviceId,
            OrganizationID = "org",
            State = AgentUpgradeState.Pending,
            CreatedAt = DateTimeOffset.UtcNow,
            EligibleAt = DateTimeOffset.UtcNow,
        };

    private static PublisherManifestBuild Build(
        string target, string format,
        string version = "2.0.0",
        string file = "cmremote-agent.bin",
        string? signature = null) =>
        new()
        {
            AgentVersion = version,
            Target = target,
            Format = format,
            File = file,
            Size = 1234,
            Sha256 = new string('a', 64),
            Signature = signature,
            SignedBy = signature is null ? null : "ca@crashmedia.ca",
        };

    private static PublisherManifest Manifest(string channel = "stable", params PublisherManifestBuild[] builds) =>
        new()
        {
            SchemaVersion = 1,
            Publisher = "CrashMedia IT",
            GeneratedAt = DateTimeOffset.UtcNow,
            Channel = channel,
            Version = builds.FirstOrDefault()?.AgentVersion ?? "2.0.0",
            Builds = builds,
        };

    // ---- AgentTargetRouting (pure) ----

    [TestMethod]
    public void Routing_WindowsX64_ReturnsMsi()
    {
        var device = new Device { Platform = "Windows 11 Pro", OSArchitecture = Architecture.X64 };
        var routing = ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device);
        Assert.IsNotNull(routing);
        Assert.AreEqual("x86_64-pc-windows-msvc", routing!.Value.Target);
        Assert.AreEqual("msi", routing.Value.Format);
    }

    [TestMethod]
    public void Routing_WindowsArm64_ReturnsNull()
    {
        // Windows-on-ARM is not in the R8 wave; the routing must refuse
        // rather than guess.
        var device = new Device { Platform = "Windows 11 Pro", OSArchitecture = Architecture.Arm64 };
        Assert.IsNull(ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device));
    }

    [TestMethod]
    public void Routing_DarwinX64_ReturnsUniversalPkg()
    {
        var device = new Device { Platform = "Darwin 23.4", OSArchitecture = Architecture.X64 };
        var routing = ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device);
        Assert.AreEqual("universal2-apple-darwin", routing!.Value.Target);
        Assert.AreEqual("pkg", routing.Value.Format);
    }

    [TestMethod]
    public void Routing_MacOsArm64_ReturnsUniversalPkg()
    {
        var device = new Device { Platform = "macOS 14.4", OSArchitecture = Architecture.Arm64 };
        var routing = ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device);
        Assert.AreEqual("universal2-apple-darwin", routing!.Value.Target);
        Assert.AreEqual("pkg", routing.Value.Format);
    }

    [TestMethod]
    public void Routing_LinuxUbuntu_ReturnsDeb()
    {
        var device = new Device { Platform = "Linux/Ubuntu 22.04", OSArchitecture = Architecture.X64 };
        var routing = ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device);
        Assert.AreEqual("x86_64-unknown-linux-gnu", routing!.Value.Target);
        Assert.AreEqual("deb", routing.Value.Format);
    }

    [TestMethod]
    public void Routing_LinuxFedora_ReturnsRpm()
    {
        var device = new Device { Platform = "Linux/Fedora 39", OSArchitecture = Architecture.Arm64 };
        var routing = ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device);
        Assert.AreEqual("aarch64-unknown-linux-gnu", routing!.Value.Target);
        Assert.AreEqual("rpm", routing.Value.Format);
    }

    [TestMethod]
    public void Routing_LinuxRocky_ReturnsRpm()
    {
        var device = new Device { Platform = "Linux/Rocky 9", OSArchitecture = Architecture.X64 };
        var routing = ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device);
        Assert.AreEqual("rpm", routing!.Value.Format);
    }

    [TestMethod]
    public void Routing_UnknownPlatform_ReturnsNull()
    {
        var device = new Device { Platform = "FreeBSD 14", OSArchitecture = Architecture.X64 };
        Assert.IsNull(ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device));
    }

    [TestMethod]
    public void Routing_UnknownArchitecture_ReturnsNull()
    {
        var device = new Device { Platform = "Linux/Ubuntu 22.04", OSArchitecture = Architecture.X86 };
        Assert.IsNull(ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device));
    }

    [TestMethod]
    public void Routing_NullPlatform_ReturnsNull()
    {
        var device = new Device { Platform = null, OSArchitecture = Architecture.X64 };
        Assert.IsNull(ManifestBackedAgentUpgradeDispatcher.AgentTargetRouting.Resolve(device));
    }

    // ---- ResolveTargetAsync ----

    [TestMethod]
    public async Task ResolveTarget_HappyPath_ReturnsAbsoluteDownloadUri()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        _provider.SetManifest("stable", Manifest("stable",
            Build("x86_64-unknown-linux-gnu", "deb", file: "cmremote-agent_2.0.0_amd64.deb")));

        var dispatcher = NewDispatcher();
        var target = await dispatcher.ResolveTargetAsync(StatusFor(_testData.Org1Device1.ID), CancellationToken.None);

        Assert.IsNotNull(target);
        Assert.AreEqual("2.0.0", target!.Version);
        // The manifest URL was https://cdn.example.com/cmremote/stable/publisher-manifest.json
        // → the resolved download URL must drop the JSON name and append the file.
        Assert.AreEqual("https://cdn.example.com/cmremote/stable/cmremote-agent_2.0.0_amd64.deb",
            target.DownloadUri.ToString());
    }

    [TestMethod]
    public async Task ResolveTarget_AlreadyOnVersion_ReturnsNull()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64,
            agentVersion: "2.0.0");
        _provider.SetManifest("stable", Manifest("stable",
            Build("x86_64-unknown-linux-gnu", "deb", version: "2.0.0")));

        var target = await NewDispatcher().ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNull(target);
    }

    [TestMethod]
    public async Task ResolveTarget_MultipleMatches_RefusesAndReturnsNull()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        _provider.SetManifest("stable", Manifest("stable",
            Build("x86_64-unknown-linux-gnu", "deb", file: "a.deb"),
            Build("x86_64-unknown-linux-gnu", "deb", file: "b.deb")));

        var target = await NewDispatcher().ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNull(target,
            "Two entries match (target,format) → the dispatcher must refuse rather than guess.");
    }

    [TestMethod]
    public async Task ResolveTarget_NoMatch_ReturnsNull()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        // Manifest only carries Windows entry — Linux device finds nothing.
        _provider.SetManifest("stable", Manifest("stable",
            Build("x86_64-pc-windows-msvc", "msi")));

        var target = await NewDispatcher().ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNull(target);
    }

    [TestMethod]
    public async Task ResolveTarget_NoManifestForChannel_ReturnsNull()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        // Provider has no manifest for stable.
        var target = await NewDispatcher().ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNull(target);
    }

    [TestMethod]
    public async Task ResolveTarget_RequireSignatureSkipsUnsignedEntries()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        _provider.SetManifest("stable", Manifest("stable",
            Build("x86_64-unknown-linux-gnu", "deb", signature: null)));

        var target = await NewDispatcher(requireSignature: true).ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNull(target);
    }

    [TestMethod]
    public async Task ResolveTarget_RequireSignatureAcceptsSignedEntries()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        _provider.SetManifest("stable", Manifest("stable",
            Build("x86_64-unknown-linux-gnu", "deb",
                signature: "MEUCIQDxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxQIgxxxxxxx")));

        var target = await NewDispatcher(requireSignature: true).ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNotNull(target);
    }

    [TestMethod]
    public async Task ResolveTarget_UnknownDevice_ReturnsNull()
    {
        // Status with a DeviceId that has no row in the table — the
        // dispatcher must leave it pending rather than throw.
        var status = StatusFor("device-that-does-not-exist");
        var target = await NewDispatcher().ResolveTargetAsync(status, CancellationToken.None);
        Assert.IsNull(target);
    }

    [TestMethod]
    public async Task ResolveTarget_UsesConfiguredDefaultChannel()
    {
        await SetDevicePlatformAsync(_testData.Org1Device1.ID, "Linux/Ubuntu 22.04", Architecture.X64);
        // Provider only has a "preview" manifest. Dispatcher's default
        // channel is "preview" → it should pick that up.
        _provider.SetManifest("preview", Manifest("preview",
            Build("x86_64-unknown-linux-gnu", "deb", version: "3.0.0-rc.1")));

        var dispatcher = NewDispatcher(
            manifestUrls: new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["preview"] = "https://cdn.example.com/cmremote/preview/publisher-manifest.json",
            },
            defaultChannel: "preview");
        var target = await dispatcher.ResolveTargetAsync(
            StatusFor(_testData.Org1Device1.ID), CancellationToken.None);
        Assert.IsNotNull(target);
        Assert.AreEqual("3.0.0-rc.1", target!.Version);
    }

    // ---- DispatchAsync ----

    [TestMethod]
    public async Task Dispatch_DeviceOffline_ReturnsRecoverableFailure()
    {
        // Empty session cache → device is offline. Must fail-fast (the
        // orchestrator's on-connect path will requeue when the device
        // reconnects).
        var sessionCache = new AgentHubSessionCache();
        var agentHub = new FakeAgentHub();
        var dispatcher = NewDispatcher(sessionCache: sessionCache, agentHub: agentHub);
        var target = new AgentUpgradeTarget("2.0.0", new string('a', 64),
            new Uri("https://cdn.example.com/cmremote/stable/cmremote-agent.deb"));

        var result = await dispatcher.DispatchAsync(
            StatusFor(_testData.Org1Device1.ID), target, CancellationToken.None);

        Assert.IsFalse(result.Succeeded);
        StringAssert.Contains(result.Error!, "offline", StringComparison.OrdinalIgnoreCase);
        Assert.AreEqual(0, agentHub.InstallAgentUpdateCalls.Count,
            "Hub method must NOT be invoked for an offline device.");
    }

    [TestMethod]
    public async Task Dispatch_PushesHubMethodAndSucceedsOnVersionBump()
    {
        var device = new Device
        {
            ID = _testData.Org1Device1.ID,
            AgentVersion = "1.9.0",
            Platform = "Linux/Ubuntu 22.04",
            OSArchitecture = Architecture.X64,
        };
        var sessionCache = new AgentHubSessionCache();
        sessionCache.AddOrUpdateByConnectionId("conn-1", device);
        var agentHub = new FakeAgentHub();
        var dispatcher = NewDispatcher(sessionCache: sessionCache, agentHub: agentHub);
        var target = new AgentUpgradeTarget("2.0.0", new string('a', 64),
            new Uri("https://cdn.example.com/cmremote/stable/cmremote-agent.deb"));

        // Simulate the agent restarting + heartbeating with the new
        // version after a tick or two.
        _ = Task.Run(async () =>
        {
            await Task.Delay(50);
            sessionCache.AddOrUpdateByConnectionId("conn-1", new Device
            {
                ID = device.ID,
                AgentVersion = "2.0.0",
                Platform = device.Platform,
                OSArchitecture = device.OSArchitecture,
            });
        });

        var result = await dispatcher.DispatchAsync(
            StatusFor(_testData.Org1Device1.ID), target, CancellationToken.None);

        Assert.IsTrue(result.Succeeded, $"Expected success but got: {result.Error}");
        Assert.AreEqual(1, agentHub.InstallAgentUpdateCalls.Count);
        var (connId, url, ver, sha) = agentHub.InstallAgentUpdateCalls[0];
        Assert.AreEqual("conn-1", connId);
        Assert.AreEqual(target.DownloadUri.ToString(), url);
        Assert.AreEqual(target.Version, ver);
        Assert.AreEqual(target.Sha256, sha);
    }

    [TestMethod]
    public async Task Dispatch_NoVersionBumpBeforeTimeout_ThrowsOperationCancelled()
    {
        var device = new Device
        {
            ID = _testData.Org1Device1.ID,
            AgentVersion = "1.9.0",
            Platform = "Linux/Ubuntu 22.04",
            OSArchitecture = Architecture.X64,
        };
        var sessionCache = new AgentHubSessionCache();
        sessionCache.AddOrUpdateByConnectionId("conn-1", device);
        var agentHub = new FakeAgentHub();
        var dispatcher = NewDispatcher(
            sessionCache: sessionCache, agentHub: agentHub,
            versionWatchInterval: TimeSpan.FromMilliseconds(10));
        var target = new AgentUpgradeTarget("2.0.0", new string('a', 64),
            new Uri("https://cdn.example.com/cmremote/stable/cmremote-agent.deb"));

        using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(150));
        // The orchestrator translates the OperationCanceledException
        // (TaskCanceledException is the concrete subtype Task.Delay
        // raises) into a "Dispatch timed out" failure, so we just need
        // to assert the dispatch loop honours the token.
        await Assert.ThrowsExceptionAsync<TaskCanceledException>(async () =>
            await dispatcher.DispatchAsync(
                StatusFor(_testData.Org1Device1.ID), target, cts.Token));
        Assert.AreEqual(1, agentHub.InstallAgentUpdateCalls.Count,
            "Hub method must be invoked exactly once even when the watch loop times out.");
    }

    [TestMethod]
    public async Task Dispatch_RefusesNonHttpsUri()
    {
        var device = new Device
        {
            ID = _testData.Org1Device1.ID,
            AgentVersion = "1.9.0",
            Platform = "Linux/Ubuntu 22.04",
            OSArchitecture = Architecture.X64,
        };
        var sessionCache = new AgentHubSessionCache();
        sessionCache.AddOrUpdateByConnectionId("conn-1", device);
        var agentHub = new FakeAgentHub();
        var dispatcher = NewDispatcher(sessionCache: sessionCache, agentHub: agentHub);
        // http (not https) — must be refused before any hub call fires.
        var target = new AgentUpgradeTarget("2.0.0", new string('a', 64),
            new Uri("http://cdn.example.com/cmremote/stable/cmremote-agent.deb"));

        var result = await dispatcher.DispatchAsync(
            StatusFor(_testData.Org1Device1.ID), target, CancellationToken.None);

        Assert.IsFalse(result.Succeeded);
        StringAssert.Contains(result.Error!, "scheme", StringComparison.OrdinalIgnoreCase);
        Assert.AreEqual(0, agentHub.InstallAgentUpdateCalls.Count);
    }

    [TestMethod]
    public async Task Dispatch_HubCallThrows_ReturnsFailureWithoutBlocking()
    {
        var device = new Device
        {
            ID = _testData.Org1Device1.ID,
            AgentVersion = "1.9.0",
            Platform = "Linux/Ubuntu 22.04",
            OSArchitecture = Architecture.X64,
        };
        var sessionCache = new AgentHubSessionCache();
        sessionCache.AddOrUpdateByConnectionId("conn-1", device);
        var agentHub = new FakeAgentHub
        {
            ThrowOnInstallAgentUpdate = new InvalidOperationException("transport closed"),
        };
        var dispatcher = NewDispatcher(sessionCache: sessionCache, agentHub: agentHub);
        var target = new AgentUpgradeTarget("2.0.0", new string('a', 64),
            new Uri("https://cdn.example.com/cmremote/stable/cmremote-agent.deb"));

        var result = await dispatcher.DispatchAsync(
            StatusFor(_testData.Org1Device1.ID), target, CancellationToken.None);

        Assert.IsFalse(result.Succeeded);
        StringAssert.Contains(result.Error!, "transport closed", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// Hand-rolled <see cref="IPublisherManifestProvider"/> stub. Avoids
    /// pulling in Moq + the matching-engine just to return a fixed
    /// per-channel manifest map.
    /// </summary>
    private sealed class FakeManifestProvider : IPublisherManifestProvider
    {
        private readonly Dictionary<string, PublisherManifest> _byChannel =
            new(StringComparer.OrdinalIgnoreCase);

        public void SetManifest(string channel, PublisherManifest manifest) =>
            _byChannel[channel] = manifest;

        public Task<PublisherManifest?> GetAsync(string channel, CancellationToken cancellationToken) =>
            Task.FromResult(_byChannel.TryGetValue(channel, out var m) ? m : null);
    }

    /// <summary>
    /// Tracking double for <see cref="IHubContext{THub, TClient}"/> over
    /// <see cref="AgentHub"/> + <see cref="IAgentHubClient"/>. Records
    /// every <see cref="IAgentHubClient.InstallAgentUpdate"/> call (along
    /// with which connection ID it was routed to) so the dispatch tests
    /// can assert exactly what hit the wire. Throws
    /// <see cref="ThrowOnInstallAgentUpdate"/> when set, so the
    /// transport-failure path can be exercised.
    /// </summary>
    internal sealed class FakeAgentHub
    {
        public List<(string ConnectionId, string DownloadUrl, string Version, string Sha256)> InstallAgentUpdateCalls { get; } = new();
        public Exception? ThrowOnInstallAgentUpdate { get; set; }

        public IHubContext<AgentHub, IAgentHubClient> HubContext { get; }

        public FakeAgentHub()
        {
            var clients = new Mock<IHubClients<IAgentHubClient>>(MockBehavior.Strict);
            clients
                .Setup(c => c.Client(It.IsAny<string>()))
                .Returns<string>(connId =>
                {
                    var client = new Mock<IAgentHubClient>(MockBehavior.Loose);
                    client
                        .Setup(c => c.InstallAgentUpdate(
                            It.IsAny<string>(), It.IsAny<string>(), It.IsAny<string>()))
                        .Returns<string, string, string>((url, ver, sha) =>
                        {
                            InstallAgentUpdateCalls.Add((connId, url, ver, sha));
                            if (ThrowOnInstallAgentUpdate is not null)
                            {
                                throw ThrowOnInstallAgentUpdate;
                            }
                            return Task.CompletedTask;
                        });
                    return client.Object;
                });

            var ctx = new Mock<IHubContext<AgentHub, IAgentHubClient>>(MockBehavior.Strict);
            ctx.Setup(c => c.Clients).Returns(clients.Object);
            HubContext = ctx.Object;
        }
    }
}
