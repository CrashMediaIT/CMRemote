using Remotely.Server.Hubs;
using Remotely.Server.Models;
using Remotely.Server.Services;
using Remotely.Server.Services.AgentUpgrade;
using Bitbound.SimpleMessenger;
using Microsoft.AspNetCore.SignalR;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Moq;
using Remotely.Shared.Dtos;
using Remotely.Shared.Entities;
using Remotely.Shared.Extensions;
using Remotely.Shared.Interfaces;
using Remotely.Shared.Services;
using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class AgentHubTests
{
    private TestData _testData = null!;
    private IDataService _dataService = null!;

    [TestMethod]
    [DoNotParallelize]
    public async Task DeviceCameOnline_BannedByName()
    {
        var circuitManager = new Mock<ICircuitManager>();
        var circuitConnection = new Mock<ICircuitConnection>();
        circuitManager.Setup(x => x.Connections).Returns(new[] { circuitConnection.Object });
        circuitConnection.Setup(x => x.User).Returns(_testData.Org1Admin1);
        var viewerHub = new Mock<IHubContext<ViewerHub>>();
        var expiringTokenService = new Mock<IExpiringTokenService>();
        var serviceSessionCache = new Mock<IAgentHubSessionCache>();
        var remoteControlSessions = new Mock<IRemoteControlSessionCache>();
        var messenger = new Mock<IMessenger>();
        var logger = new Mock<ILogger<AgentHub>>();

        var settings = await _dataService.GetSettings();
        settings.BannedDevices = [_testData.Org1Device1.DeviceName!];
        await _dataService.SaveSettings(settings);

        var hub = new AgentHub(
            _dataService,
            serviceSessionCache.Object,
            viewerHub.Object,
            circuitManager.Object,
            expiringTokenService.Object,
            remoteControlSessions.Object,
            messenger.Object,
            new Mock<IInstalledApplicationsService>().Object,
            new Mock<IPackageInstallJobService>().Object,
            new Mock<IAgentUpgradeService>().Object,
            new SystemTime(),
            logger.Object);

        var hubClients = new Mock<IHubCallerClients<IAgentHubClient>>();
        var caller = new Mock<IAgentHubClient>();
        hubClients.Setup(x => x.Caller).Returns(caller.Object);
        hub.Clients = hubClients.Object;

        var result = await hub.DeviceCameOnline(_testData.Org1Device1.ToDto());
        Assert.IsFalse(result);
        hubClients.Verify(x => x.Caller, Times.Once);
        caller.Verify(x => x.UninstallAgent(), Times.Once);
    }

    // TODO: Checking of device ban should be pulled out into
    // a separate service that's better testable.
    [TestMethod]
    [DoNotParallelize]
    public async Task DeviceCameOnline_BannedById()
    {
        var circuitManager = new Mock<ICircuitManager>();
        var circuitConnection = new Mock<ICircuitConnection>();
        circuitManager.Setup(x => x.Connections).Returns(new[] { circuitConnection.Object });
        circuitConnection.Setup(x => x.User).Returns(_testData.Org1Admin1);
        var viewerHub = new Mock<IHubContext<ViewerHub>>();
        var expiringTokenService = new Mock<IExpiringTokenService>();
        var serviceSessionCache = new Mock<IAgentHubSessionCache>();
        var remoteControlSessions = new Mock<IRemoteControlSessionCache>();
        var messenger = new Mock<IMessenger>();
        var logger = new Mock<ILogger<AgentHub>>();


        var settings = await _dataService.GetSettings();
        settings.BannedDevices = [$"{_testData.Org1Device1.ID}"];
        await _dataService.SaveSettings(settings);

        var hub = new AgentHub(
            _dataService,
            serviceSessionCache.Object,
            viewerHub.Object,
            circuitManager.Object,
            expiringTokenService.Object,
            remoteControlSessions.Object,
            messenger.Object,
            new Mock<IInstalledApplicationsService>().Object,
            new Mock<IPackageInstallJobService>().Object,
            new Mock<IAgentUpgradeService>().Object,
            new SystemTime(),
            logger.Object);

        var hubClients = new Mock<IHubCallerClients<IAgentHubClient>>();
        var caller = new Mock<IAgentHubClient>();
        hubClients.Setup(x => x.Caller).Returns(caller.Object);
        hub.Clients = hubClients.Object;

        var result = await hub.DeviceCameOnline(_testData.Org1Device1.ToDto());
        Assert.IsFalse(result);
        hubClients.Verify(x => x.Caller, Times.Once);
        caller.Verify(x => x.UninstallAgent(), Times.Once);
    }

    // -----------------------------------------------------------------
    // Slice R7.n.7 — agent → server signalling (SendSdpAnswer /
    // SendIceCandidate). Each test wires a minimal hub with a mocked
    // viewer-hub client proxy, an in-memory remote-control session
    // cache, and the calling agent's `Device` populated in
    // `Context.Items`, then asserts the forwarding contract.
    // -----------------------------------------------------------------

    [TestMethod]
    public async Task SendSdpAnswer_ForwardsToViewerWhenSessionAndViewerMatch()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = device.ID, OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, viewerClients) = BuildSignallingHub(device, sessionId, session);

        var dto = new AgentSdpAnswerDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n",
        };

        await hub.SendSdpAnswer(dto);

        viewerClients.Verify(c => c.Client(viewerId), Times.Once);
        viewerProxy.Verify(
            p => p.SendCoreAsync(
                "ReceiveSdpAnswer",
                It.Is<object?[]>(args => args.Length == 1 && ReferenceEquals(args[0], dto)),
                It.IsAny<CancellationToken>()),
            Times.Once);
    }

    [TestMethod]
    public async Task SendSdpAnswer_DropsWhenSessionNotInCache()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";

        // Build hub but DO NOT register the session.
        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId: null, session: null);

        await hub.SendSdpAnswer(new AgentSdpAnswerDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Sdp = "v=0\r\n",
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendSdpAnswer_DropsWhenSessionBelongsToAnotherDevice()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        // Session is for a different device — must not be forwarded.
        var session = new RemoteControlSession { DeviceId = "other-device", OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId, session);

        await hub.SendSdpAnswer(new AgentSdpAnswerDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Sdp = "v=0\r\n",
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendSdpAnswer_DropsWhenViewerNotInSessionViewerList()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = device.ID, OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add("some-other-viewer");

        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId, session);

        await hub.SendSdpAnswer(new AgentSdpAnswerDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Sdp = "v=0\r\n",
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendSdpAnswer_DropsWhenSdpExceedsCap()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = device.ID, OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId, session);

        await hub.SendSdpAnswer(new AgentSdpAnswerDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Sdp = new string('x', AgentSignallingLimits.MaxSdpBytes + 1),
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendSdpAnswer_DropsWhenRoutingFieldsEmpty()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId: null, session: null);

        await hub.SendSdpAnswer(new AgentSdpAnswerDto
        {
            ViewerConnectionId = "",
            SessionId = "",
            Sdp = "v=0\r\n",
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendSdpAnswer_DoesNotThrowOnNullDto()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId: null, session: null);

        await hub.SendSdpAnswer(null!);

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendIceCandidate_ForwardsToViewerWithOptionalFields()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = device.ID, OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, viewerClients) = BuildSignallingHub(device, sessionId, session);

        var dto = new AgentIceCandidateDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Candidate = "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host",
            SdpMid = "0",
            SdpMlineIndex = 0,
        };

        await hub.SendIceCandidate(dto);

        viewerClients.Verify(c => c.Client(viewerId), Times.Once);
        viewerProxy.Verify(
            p => p.SendCoreAsync(
                "ReceiveIceCandidate",
                It.Is<object?[]>(args => args.Length == 1 && ReferenceEquals(args[0], dto)),
                It.IsAny<CancellationToken>()),
            Times.Once);
    }

    [TestMethod]
    public async Task SendIceCandidate_ForwardsEndOfCandidatesMarker()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = device.ID, OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId, session);

        // Empty candidate + null mid/index — RFC 8838 marker. Must
        // forward verbatim, not be rejected by the length cap.
        var dto = new AgentIceCandidateDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Candidate = string.Empty,
            SdpMid = null,
            SdpMlineIndex = null,
        };

        await hub.SendIceCandidate(dto);

        viewerProxy.Verify(
            p => p.SendCoreAsync(
                "ReceiveIceCandidate",
                It.Is<object?[]>(args => args.Length == 1 && ReferenceEquals(args[0], dto)),
                It.IsAny<CancellationToken>()),
            Times.Once);
    }

    [TestMethod]
    public async Task SendIceCandidate_DropsWhenCandidateExceedsCap()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = device.ID, OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId, session);

        await hub.SendIceCandidate(new AgentIceCandidateDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Candidate = new string('x', AgentSignallingLimits.MaxSignallingStringLen + 1),
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    [TestMethod]
    public async Task SendIceCandidate_DropsWhenSessionBelongsToAnotherDevice()
    {
        var device = new Device { ID = "device-1", OrganizationID = "org-1" };
        var session = new RemoteControlSession { DeviceId = "other-device", OrganizationId = device.OrganizationID };
        const string viewerId = "viewer-conn-7";
        const string sessionId = "11111111-2222-3333-4444-555555555555";
        session.ViewerList.Add(viewerId);

        var (hub, viewerProxy, _) = BuildSignallingHub(device, sessionId, session);

        await hub.SendIceCandidate(new AgentIceCandidateDto
        {
            ViewerConnectionId = viewerId,
            SessionId = sessionId,
            Candidate = "candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host",
        });

        viewerProxy.Verify(
            p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()),
            Times.Never);
    }

    /// <summary>
    /// Shared scaffolding for the SendSdpAnswer / SendIceCandidate
    /// tests: builds an <see cref="AgentHub"/> with a mocked viewer
    /// hub context that exposes a verifiable client proxy, and (when
    /// a non-null <paramref name="sessionId"/> is provided)
    /// pre-registers <paramref name="session"/> in the
    /// <see cref="IRemoteControlSessionCache"/> mock.
    /// </summary>
    private static (AgentHub Hub, Mock<ISingleClientProxy> ViewerProxy, Mock<IHubClients> ViewerClients) BuildSignallingHub(
        Device device,
        string? sessionId,
        RemoteControlSession? session)
    {
        var circuitManager = new Mock<ICircuitManager>();
        var dataService = new Mock<IDataService>();
        var viewerProxy = new Mock<ISingleClientProxy>();
        viewerProxy
            .Setup(p => p.SendCoreAsync(It.IsAny<string>(), It.IsAny<object?[]>(), It.IsAny<CancellationToken>()))
            .Returns(Task.CompletedTask);
        var viewerClients = new Mock<IHubClients>();
        viewerClients.Setup(c => c.Client(It.IsAny<string>())).Returns(viewerProxy.Object);
        var viewerHub = new Mock<IHubContext<ViewerHub>>();
        viewerHub.Setup(h => h.Clients).Returns(viewerClients.Object);

        var expiringTokenService = new Mock<IExpiringTokenService>();
        var serviceSessionCache = new Mock<IAgentHubSessionCache>();

        var remoteControlSessions = new Mock<IRemoteControlSessionCache>();
        if (!string.IsNullOrEmpty(sessionId) && session is not null)
        {
            remoteControlSessions
                .Setup(s => s.TryGetValue(sessionId, out session!))
                .Returns(true);
        }
        // Default fall-through for any other session id is `false`
        // (Moq's default behaviour for unmatched setups).

        var messenger = new Mock<IMessenger>();
        var logger = new Mock<ILogger<AgentHub>>();

        var hub = new AgentHub(
            dataService.Object,
            serviceSessionCache.Object,
            viewerHub.Object,
            circuitManager.Object,
            expiringTokenService.Object,
            remoteControlSessions.Object,
            messenger.Object,
            new Mock<IInstalledApplicationsService>().Object,
            new Mock<IPackageInstallJobService>().Object,
            new Mock<IAgentUpgradeService>().Object,
            new SystemTime(),
            logger.Object);

        // Populate the Device in HubCallerContext.Items so the hub
        // methods see an authenticated agent. The hub reads `Device`
        // via the private accessor, which checks Context.Items.
        var items = new Dictionary<object, object?> { ["Device"] = device };
        var context = new Mock<HubCallerContext>();
        context.Setup(c => c.Items).Returns(items);
        context.Setup(c => c.ConnectionId).Returns(Guid.NewGuid().ToString());
        hub.Context = context.Object;

        return (hub, viewerProxy, viewerClients);
    }

    [TestCleanup]
    public void TestCleanup()
    {
        _testData.ClearData();
    }

    [TestInitialize]
    public async Task TestInit()
    {
        _testData = new TestData();
        await _testData.Init();
        _dataService = IoCActivator.ServiceProvider.GetRequiredService<IDataService>();
    }
}
