using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.AgentUpgrade;
using Remotely.Shared.Entities;
using Remotely.Shared.Enums;
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
        string defaultChannel = "stable")
    {
        var opts = MsOptions.Create(new AgentUpgradeManifestOptions
        {
            ManifestUrls = manifestUrls ?? new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
            {
                ["stable"] = "https://cdn.example.com/cmremote/stable/publisher-manifest.json",
            },
            RequireSignature = requireSignature,
            DefaultChannel = defaultChannel,
        });
        return new ManifestBackedAgentUpgradeDispatcher(
            _provider, _dbFactory, opts,
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

    // ---- DispatchAsync (deferred handler is explicit failure) ----

    [TestMethod]
    public async Task Dispatch_BeforeR6Handler_ReturnsExplicitFailure()
    {
        var dispatcher = NewDispatcher();
        var target = new AgentUpgradeTarget("2.0.0", new string('a', 64),
            new Uri("https://cdn.example.com/cmremote/stable/cmremote-agent.deb"));
        var result = await dispatcher.DispatchAsync(
            StatusFor(_testData.Org1Device1.ID), target, CancellationToken.None);
        Assert.IsFalse(result.Succeeded);
        Assert.IsNotNull(result.Error);
        // The failure must be explicit so the orchestrator records it
        // as a real Failed transition rather than a silent success.
        StringAssert.Contains(result.Error!, "R6", StringComparison.OrdinalIgnoreCase);
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
}
