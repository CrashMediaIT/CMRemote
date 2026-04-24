using System.Linq;
using System.Threading.Tasks;
using Microsoft.AspNetCore.Identity;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.Setup;
using Remotely.Shared.Entities;

namespace Remotely.Server.Tests;

[TestClass]
public class AdminBootstrapServiceTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private UserManager<RemotelyUser> _userManager = null!;
    private AdminBootstrapService _service = null!;

    [TestInitialize]
    public void Init()
    {
        _testData = new TestData();
        _testData.ClearData();
        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _userManager = IoCActivator.ServiceProvider
            .GetRequiredService<UserManager<RemotelyUser>>();
        _service = new AdminBootstrapService(
            _dbFactory,
            _userManager,
            NullLogger<AdminBootstrapService>.Instance);
    }

    [TestMethod]
    public async Task IsRequired_GreenfieldDb_ReturnsTrue()
    {
        Assert.IsTrue(await _service.IsRequiredAsync());
    }

    [TestMethod]
    public async Task IsRequired_OrgPresent_ReturnsFalse()
    {
        using (var db = _dbFactory.GetContext())
        {
            db.Organizations.Add(new Organization { OrganizationName = "Imported" });
            await db.SaveChangesAsync();
        }

        Assert.IsFalse(await _service.IsRequiredAsync(),
            "An imported org means the wizard must not insist on creating another.");
    }

    [TestMethod]
    public async Task CreateInitialAdmin_HappyPath_PersistsOrgAndAdminUser()
    {
        var result = await _service.CreateInitialAdminAsync(
            organizationName: "Acme IT",
            email: "Admin@Example.COM",
            password: "C0rrect-Horse-Battery-Staple!");

        Assert.IsTrue(result.IsSuccess,
            $"Expected success but got: {string.Join("; ", result.Errors)}");
        Assert.IsNotNull(result.CreatedUserId);

        using var db = _dbFactory.GetContext();
        var org = await db.Organizations.AsNoTracking().FirstAsync();
        Assert.AreEqual("Acme IT", org.OrganizationName);
        Assert.IsTrue(org.IsDefaultOrganization,
            "The very first organisation must be flagged as default.");

        var user = await db.Users.AsNoTracking().FirstAsync();
        Assert.AreEqual("admin@example.com", user.UserName,
            "Email must be normalised to lower-case to match the rest of the app's convention.");
        Assert.AreEqual("admin@example.com", user.Email);
        Assert.IsTrue(user.IsAdministrator,
            "First-admin must have IsAdministrator=true so they can manage their org.");
        Assert.IsTrue(user.IsServerAdmin,
            "First-admin must have IsServerAdmin=true so they can manage the deployment.");
        Assert.IsTrue(user.EmailConfirmed,
            "Wizard-created admin should land with EmailConfirmed=true so they can sign in immediately.");
        Assert.AreEqual(org.ID, user.OrganizationID);
        Assert.IsFalse(string.IsNullOrEmpty(user.PasswordHash),
            "Password must be hashed by Identity, not stored in cleartext.");
        Assert.IsFalse(string.IsNullOrEmpty(user.SecurityStamp),
            "Identity must stamp a SecurityStamp on creation.");
    }

    [TestMethod]
    public async Task CreateInitialAdmin_WeakPassword_ReportsIdentityErrorsAndRollsBackOrg()
    {
        // The default Identity password policy requires 6+ chars + a
        // digit + a non-alphanumeric + lowercase; "abc" violates all of
        // those, so the IPasswordValidator should reject the call and
        // the wrapper must roll back the org row to keep the database
        // in the same shape the operator started in.
        var result = await _service.CreateInitialAdminAsync(
            organizationName: "Acme IT",
            email: "admin@example.com",
            password: "abc");

        Assert.IsFalse(result.IsSuccess);
        Assert.IsTrue(result.Errors.Count > 0);

        using var db = _dbFactory.GetContext();
        Assert.AreEqual(0, await db.Organizations.AsNoTracking().CountAsync(),
            "A failed admin-bootstrap attempt must not leave a phantom org row behind.");
        Assert.AreEqual(0, await db.Users.AsNoTracking().CountAsync());
    }

    [TestMethod]
    public async Task CreateInitialAdmin_OnceUserExists_RefusesSecondCall()
    {
        var first = await _service.CreateInitialAdminAsync(
            organizationName: "Acme IT",
            email: "first@example.com",
            password: "C0rrect-Horse-Battery-Staple!");
        Assert.IsTrue(first.IsSuccess);

        var second = await _service.CreateInitialAdminAsync(
            organizationName: "Other Co",
            email: "second@example.com",
            password: "C0rrect-Horse-Battery-Staple!");

        Assert.IsFalse(second.IsSuccess,
            "Once an org/user pair exists, the bootstrap step is no longer required " +
            "and a second call must refuse rather than silently create a parallel admin.");

        using var db = _dbFactory.GetContext();
        Assert.AreEqual(1, await db.Organizations.AsNoTracking().CountAsync());
        Assert.AreEqual(1, await db.Users.AsNoTracking().CountAsync());
    }

    [TestMethod]
    public async Task CreateInitialAdmin_BlankInputs_ReturnsValidationErrors()
    {
        var emptyName = await _service.CreateInitialAdminAsync(
            string.Empty, "admin@example.com", "C0rrect-Horse-Battery-Staple!");
        Assert.IsFalse(emptyName.IsSuccess);
        Assert.IsTrue(emptyName.Errors.Any(e => e.Contains("Organisation name")));

        var emptyEmail = await _service.CreateInitialAdminAsync(
            "Acme IT", string.Empty, "C0rrect-Horse-Battery-Staple!");
        Assert.IsFalse(emptyEmail.IsSuccess);
        Assert.IsTrue(emptyEmail.Errors.Any(e => e.Contains("Email")));

        var emptyPassword = await _service.CreateInitialAdminAsync(
            "Acme IT", "admin@example.com", string.Empty);
        Assert.IsFalse(emptyPassword.IsSuccess);
        Assert.IsTrue(emptyPassword.Errors.Any(e => e.Contains("Password")));

        // None of the validation failures may have written rows.
        using var db = _dbFactory.GetContext();
        Assert.AreEqual(0, await db.Organizations.AsNoTracking().CountAsync());
        Assert.AreEqual(0, await db.Users.AsNoTracking().CountAsync());
    }

    [TestMethod]
    public async Task CreateInitialAdmin_OrganizationNameTruncatedToCap()
    {
        // Storage cap is 25 chars on Organization.OrganizationName —
        // service should pre-truncate so the operator gets a clean
        // success rather than a EF-thrown DbUpdateException.
        var longName = new string('a', 100);
        var result = await _service.CreateInitialAdminAsync(
            longName,
            "admin@example.com",
            "C0rrect-Horse-Battery-Staple!");

        Assert.IsTrue(result.IsSuccess,
            $"Long names should be truncated, not rejected. Errors: {string.Join("; ", result.Errors)}");

        using var db = _dbFactory.GetContext();
        var org = await db.Organizations.AsNoTracking().FirstAsync();
        Assert.AreEqual(25, org.OrganizationName.Length);
    }
}
