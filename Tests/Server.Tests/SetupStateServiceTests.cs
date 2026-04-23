using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services;
using Remotely.Shared.Entities;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class SetupStateServiceTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private SetupStateService _service = null!;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        // ClearData is enough — the wizard tests don't need the seeded
        // orgs/users/devices that TestData.Init() creates; tests that
        // exercise the existing-deployment heuristic seed their own rows.
        _testData.ClearData();
        await Task.CompletedTask;

        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _service = new SetupStateService(_dbFactory, NullLogger<SetupStateService>.Instance);
    }

    [TestMethod]
    public async Task IsSetupCompleted_GreenfieldDb_ReturnsFalse()
    {
        Assert.IsFalse(await _service.IsSetupCompletedAsync());
    }

    [TestMethod]
    public async Task MarkSetupCompleted_WritesMarker_AndIsObservable()
    {
        await _service.MarkSetupCompletedAsync();

        Assert.IsTrue(await _service.IsSetupCompletedAsync());

        using var db = _dbFactory.GetContext();
        var record = await db.KeyValueRecords
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.Key == SetupStateService.SetupCompletedKey);
        Assert.IsNotNull(record);
        Assert.IsFalse(string.IsNullOrWhiteSpace(record!.Value));
        StringAssert.Contains(record.Value, "CompletedAtUtc");
    }

    [TestMethod]
    public async Task MarkSetupCompleted_IsIdempotent_DoesNotOverwriteOriginalStamp()
    {
        await _service.MarkSetupCompletedAsync();

        string? firstValue;
        using (var db = _dbFactory.GetContext())
        {
            firstValue = (await db.KeyValueRecords
                .AsNoTracking()
                .FirstAsync(x => x.Key == SetupStateService.SetupCompletedKey)).Value;
        }

        await Task.Delay(15); // ensure a different DateTimeOffset.UtcNow if the impl re-stamped.
        await _service.MarkSetupCompletedAsync();

        using (var db = _dbFactory.GetContext())
        {
            var secondValue = (await db.KeyValueRecords
                .AsNoTracking()
                .FirstAsync(x => x.Key == SetupStateService.SetupCompletedKey)).Value;
            Assert.AreEqual(firstValue, secondValue,
                "The completion marker must be idempotent; second MarkSetupCompletedAsync " +
                "call should not overwrite the original stamp.");
        }
    }

    [TestMethod]
    public async Task EnsureMarkerForExistingDeployment_GreenfieldDb_LeavesMarkerUnset()
    {
        await _service.EnsureMarkerForExistingDeploymentAsync();
        Assert.IsFalse(await _service.IsSetupCompletedAsync(),
            "Greenfield databases must remain unmarked so the wizard is shown.");
    }

    [TestMethod]
    public async Task EnsureMarkerForExistingDeployment_WithExistingOrg_WritesMarker()
    {
        using (var db = _dbFactory.GetContext())
        {
            db.Organizations.Add(new Organization { OrganizationName = "Pre-existing" });
            await db.SaveChangesAsync();
        }

        await _service.EnsureMarkerForExistingDeploymentAsync();
        Assert.IsTrue(await _service.IsSetupCompletedAsync(),
            "An existing deployment (org present) must be auto-marked so it " +
            "is not hijacked into the setup wizard.");
    }

    [TestMethod]
    public async Task EnsureMarkerForExistingDeployment_WithExistingDevice_WritesMarker()
    {
        using (var db = _dbFactory.GetContext())
        {
            var org = new Organization { OrganizationName = "Pre-existing" };
            db.Organizations.Add(org);
            await db.SaveChangesAsync();

            db.Devices.Add(new Device
            {
                ID = "preexisting-device",
                DeviceName = "preexisting",
                OrganizationID = org.ID,
            });
            await db.SaveChangesAsync();
        }

        await _service.EnsureMarkerForExistingDeploymentAsync();
        Assert.IsTrue(await _service.IsSetupCompletedAsync());
    }

    [TestMethod]
    public async Task EnsureMarkerForExistingDeployment_AlreadyMarked_IsNoOp()
    {
        await _service.MarkSetupCompletedAsync();

        // Should not throw and should leave the marker intact.
        await _service.EnsureMarkerForExistingDeploymentAsync();

        Assert.IsTrue(await _service.IsSetupCompletedAsync());
    }
}
