// Source: CMRemote, clean-room implementation

using Microsoft.AspNetCore.Identity;
using Microsoft.Extensions.DependencyInjection;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using System;
using System.Threading.Tasks;

namespace Remotely.Server.Tests.Infrastructure;

/// <summary>
/// Shared scaffold used by the Module 3 (<c>Server.Services</c> clean-room
/// split) per-service test classes. Consolidates the
/// <see cref="IoCActivator"/> + <see cref="TestData"/> pattern used today by
/// <c>SetupStateServiceTests</c>, <c>AdminBootstrapServiceTests</c>,
/// <c>AuditLogServiceTests</c>, etc., so each new per-service slice (S1…S9)
/// only needs a one-line <c>[TestInitialize]</c>.
/// </summary>
/// <remarks>
/// <para>
/// The fixture deliberately reuses the assembly-wide
/// <see cref="IoCActivator.ServiceProvider"/> rather than spinning up a new
/// one per test. <c>TestingDbContext</c> binds its in-memory store to a
/// fixed database name, so tests already share a single store and reset it
/// between runs via <see cref="TestData.ClearData"/>. Adding a parallel
/// service provider here would only fragment that store and break the
/// existing convention.
/// </para>
/// <para>
/// See <c>docs/server-services/README.md</c> for the slice plan and the
/// per-service spec template that backs every subsequent slice.
/// </para>
/// </remarks>
public sealed class ServiceTestFixture
{
    private ServiceTestFixture(TestData? data)
    {
        Services = IoCActivator.ServiceProvider;
        DbFactory = Services.GetRequiredService<IAppDbFactory>();
        Data = data;
    }

    /// <summary>The assembly-wide <see cref="IServiceProvider"/>.</summary>
    public IServiceProvider Services { get; }

    /// <summary>The shared <see cref="IAppDbFactory"/>.</summary>
    public IAppDbFactory DbFactory { get; }

    /// <summary>
    /// The seeded <see cref="TestData"/> when the fixture was built via
    /// <see cref="CreateSeededAsync"/>; <c>null</c> when built via
    /// <see cref="CreateEmptyAsync"/>.
    /// </summary>
    public TestData? Data { get; }

    /// <summary>
    /// Wipes the in-memory database and returns a fixture without seeding
    /// the standard two-org / four-user / four-device fixture. Use for
    /// services whose tests prefer to insert their own rows.
    /// </summary>
    public static Task<ServiceTestFixture> CreateEmptyAsync()
    {
        var data = new TestData();
        data.ClearData();
        return Task.FromResult(new ServiceTestFixture(data: null));
    }

    /// <summary>
    /// Wipes the in-memory database and seeds the canonical
    /// <see cref="TestData"/> (Org1 + Org2, two admins / two users / two
    /// devices each, two device groups each). Use for services whose tests
    /// need realistic cross-org coverage.
    /// </summary>
    public static async Task<ServiceTestFixture> CreateSeededAsync()
    {
        var data = new TestData();
        await data.Init();
        return new ServiceTestFixture(data);
    }

    /// <summary>
    /// Resolves a registered service from the shared provider. Equivalent
    /// to <c>fixture.Services.GetRequiredService&lt;T&gt;()</c>; provided as
    /// sugar so per-service tests stay terse.
    /// </summary>
    public T Get<T>() where T : notnull => Services.GetRequiredService<T>();

    /// <summary>
    /// Resolves the ASP.NET Identity <see cref="UserManager{TUser}"/> for
    /// <see cref="RemotelyUser"/>. Most user-directory tests need this
    /// alongside the db factory.
    /// </summary>
    public UserManager<RemotelyUser> GetUserManager()
        => Services.GetRequiredService<UserManager<RemotelyUser>>();
}
