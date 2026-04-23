using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.Setup;
using Remotely.Shared.Entities;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class SetupWizardProgressServiceTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private SetupWizardProgressService _service = null!;

    [TestInitialize]
    public void Init()
    {
        _testData = new TestData();
        _testData.ClearData();
        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _service = new SetupWizardProgressService(
            _dbFactory,
            NullLogger<SetupWizardProgressService>.Instance);
    }

    [TestMethod]
    public async Task GetCurrentStep_FreshDb_ReturnsWelcome()
    {
        Assert.AreEqual(SetupWizardStep.Welcome, await _service.GetCurrentStepAsync());
    }

    [TestMethod]
    public async Task SetCurrentStep_PersistsAndRoundTrips()
    {
        await _service.SetCurrentStepAsync(SetupWizardStep.Database);
        Assert.AreEqual(SetupWizardStep.Database, await _service.GetCurrentStepAsync());
    }

    [TestMethod]
    public async Task SetCurrentStep_RefusesToMoveBackwards()
    {
        await _service.SetCurrentStepAsync(SetupWizardStep.Import);
        await _service.SetCurrentStepAsync(SetupWizardStep.Preflight);
        Assert.AreEqual(SetupWizardStep.Import, await _service.GetCurrentStepAsync(),
            "The wizard never un-completes a step; the second SetCurrentStepAsync " +
            "call must be a no-op when it would move backwards.");
    }

    [TestMethod]
    public async Task SetCurrentStep_SameStepTwice_IsIdempotent()
    {
        await _service.SetCurrentStepAsync(SetupWizardStep.Database);
        await _service.SetCurrentStepAsync(SetupWizardStep.Database);
        Assert.AreEqual(SetupWizardStep.Database, await _service.GetCurrentStepAsync());
    }

    [TestMethod]
    public async Task SetCurrentStep_OutOfRange_Throws()
    {
        await Assert.ThrowsExceptionAsync<System.ArgumentOutOfRangeException>(() =>
            _service.SetCurrentStepAsync((SetupWizardStep)999));
    }

    [TestMethod]
    public async Task GetCurrentStep_MalformedMarker_ReturnsWelcome()
    {
        // Seed the row by hand with a value that is not a valid
        // WizardProgressMarker JSON document so we exercise the
        // logger-warned recovery path.
        using (var db = _dbFactory.GetContext())
        {
            db.KeyValueRecords.Add(new KeyValueRecord
            {
                Key = SetupWizardProgressService.WizardProgressKey,
                Value = "this-is-not-json",
            });
            await db.SaveChangesAsync();
        }

        Assert.AreEqual(SetupWizardStep.Welcome, await _service.GetCurrentStepAsync());
    }

    [TestMethod]
    public async Task GetCurrentStep_UnknownEnumValue_ReturnsWelcome()
    {
        // A future build that wrote a step value the running build
        // does not know about must not crash the wizard — the
        // service falls back to Welcome.
        using (var db = _dbFactory.GetContext())
        {
            db.KeyValueRecords.Add(new KeyValueRecord
            {
                Key = SetupWizardProgressService.WizardProgressKey,
                Value = """{"Step":999,"UpdatedAtUtc":"2026-04-23T00:00:00+00:00"}""",
            });
            await db.SaveChangesAsync();
        }

        Assert.AreEqual(SetupWizardStep.Welcome, await _service.GetCurrentStepAsync());
    }
}
