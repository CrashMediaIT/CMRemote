using Remotely.Server.Extensions;
using Bitbound.SimpleMessenger;
using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.Components.Authorization;
using Microsoft.AspNetCore.Components.Server.Circuits;
using Microsoft.AspNetCore.HttpOverrides;
using Microsoft.AspNetCore.Identity;
using Microsoft.AspNetCore.Identity.UI.Services;
using Microsoft.AspNetCore.RateLimiting;
using Microsoft.AspNetCore.StaticFiles;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.FileProviders;
using Remotely.Server.Auth;
using Remotely.Server.Components.Account;
using Remotely.Server.Data;
using Remotely.Server.Hubs;
using Remotely.Server.Middleware;
using Remotely.Server.Models;
using Remotely.Server.Options;
using Remotely.Server.Services;
using Remotely.Server.Services.Stores;
using Remotely.Shared.Entities;
using Remotely.Shared.Services;
using Serilog;
using System.Net;
using RatePolicyNames = Remotely.Server.RateLimiting.PolicyNames;
using Remotely.Server.Filters;
using Remotely.Server.Services.UserDirectory;
using Remotely.Server.Services.Organizations;
using Remotely.Server.Services.Devices;

var builder = WebApplication.CreateBuilder(args);
var configuration = builder.Configuration;
var services = builder.Services;

configuration.AddEnvironmentVariables("Remotely_");

services.Configure<ApplicationOptions>(
    configuration.GetSection(ApplicationOptions.SectionKey));

var appOptions = configuration
    .GetSection(ApplicationOptions.SectionKey)
    .Get<ApplicationOptions>();

services
    .AddRazorComponents()
    .AddInteractiveServerComponents();

services.AddRazorPages();

services.AddCascadingAuthenticationState();
services.AddScoped<IdentityUserAccessor>();
services.AddScoped<IdentityRedirectManager>();
services.AddScoped<AuthenticationStateProvider, IdentityRevalidatingAuthenticationStateProvider>();

var dbProvider = appOptions?.DbProvider?.ToLower();

switch (dbProvider)
{
    case "sqlite":
        services.AddDbContext<AppDb, SqliteDbContext>(
            contextLifetime: ServiceLifetime.Transient,
            optionsLifetime: ServiceLifetime.Transient);
        break;

    case "sqlserver":
        services.AddDbContext<AppDb, SqlServerDbContext>(
            contextLifetime: ServiceLifetime.Transient,
            optionsLifetime: ServiceLifetime.Transient);
        break;

    case "postgresql":
        services.AddDbContext<AppDb, PostgreSqlDbContext>(
            contextLifetime: ServiceLifetime.Transient,
            optionsLifetime: ServiceLifetime.Transient);
        break;

    default:
        throw new InvalidOperationException(
            $"Invalid DBProvider: {dbProvider}.  Ensure a valid value " +
            $"is set in appsettings.json or environment variables.");
}

using AppDb appDb = dbProvider switch
{
    "sqlite" => new SqliteDbContext(builder.Configuration, builder.Environment),
    "sqlserver" => new SqlServerDbContext(builder.Configuration, builder.Environment),
    "postgresql" => new PostgreSqlDbContext(builder.Configuration, builder.Environment),
    _ => throw new InvalidOperationException($"Invalid DBProvider: {dbProvider}")
};

await appDb.Database.MigrateAsync();
var settings = await appDb.GetAppSettings();

ConfigureSerilog(builder, settings);

builder.Logging.AddConfiguration(builder.Configuration.GetSection("Logging"));

if (OperatingSystem.IsWindows() && settings.EnableWindowsEventLog)
{
    builder.Logging.AddEventLog();
}

builder.Services.AddAuthentication(options =>
{
    options.DefaultScheme = IdentityConstants.ApplicationScheme;
    options.DefaultSignInScheme = IdentityConstants.ExternalScheme;
})
    .AddIdentityCookies();

services.AddIdentityCore<RemotelyUser>(options =>
{
    options.Stores.MaxLengthForKeys = 128;
    options.Password.RequireNonAlphanumeric = false;
})
    .AddEntityFrameworkStores<AppDb>()
    .AddSignInManager()
    .AddDefaultTokenProviders();

services.AddScoped<IAuthorizationHandler, TwoFactorRequiredHandler>();
services.AddScoped<IAuthorizationHandler, OrganizationAdminRequirementHandler>();
services.AddScoped<IAuthorizationHandler, ServerAdminRequirementHandler>();
services.AddScoped<IAuthorizationHandler, PackageManagerRequirementHandler>();
services.AddSingleton<IEmailSender<RemotelyUser>, IdentityNoOpEmailSender>();

services.AddAuthorization(options =>
{
    options.AddPolicy(PolicyNames.TwoFactorRequired, builder =>
    {
        builder.Requirements.Add(new TwoFactorRequiredRequirement());
    });

    options.AddPolicy(PolicyNames.OrganizationAdminRequired, builder =>
    {
        builder.Requirements.Add(new OrganizationAdminRequirement());
    });

    options.AddPolicy(PolicyNames.ServerAdminRequired, builder =>
    {
        builder.Requirements.Add(new ServerAdminRequirement());
    });

    options.AddPolicy(PolicyNames.PackageManagerRequired, builder =>
    {
        builder.Requirements.Add(new PackageManagerRequirement());
    });
});

services.AddDatabaseDeveloperPageExceptionFilter();

if (settings.UseHttpLogging)
{
    services.AddHttpLogging(options =>
    {
        options.RequestHeaders.Add("X-Forwarded-For");
        options.RequestHeaders.Add("X-Forwarded-Proto");
        options.RequestHeaders.Add("X-Forwarded-Host");
        options.RequestHeaders.Add("X-Original-For");
        options.RequestHeaders.Add("X-Original-Proto");
        options.RequestHeaders.Add("X-Original-Host");
        options.RequestHeaders.Add("Host");
    });
}

services.AddCors(options =>
{
    if (settings.TrustedCorsOrigins is { Count: > 0 } trustedOrigins)
    {
        options.AddPolicy("TrustedOriginPolicy", builder => builder
            .WithOrigins(trustedOrigins.ToArray())
            .AllowAnyHeader()
            .AllowAnyMethod()
            .AllowCredentials()
        );
    }
});

services.Configure<ForwardedHeadersOptions>(options =>
{
    options.ForwardedHeaders = ForwardedHeaders.All;
    options.ForwardLimit = null;

    // Default Docker host. We want to allow forwarded headers from this address.
    if (IPAddress.TryParse(appOptions?.DockerGatewayIp, out var dockerGatewayIp))
    {
        options.KnownProxies.Add(dockerGatewayIp);
    }

    if (settings.KnownProxies is { Count: > 0 } knownProxies)
    {
        foreach (var proxy in knownProxies)
        {
            if (IPAddress.TryParse(proxy, out var ip))
            {
                options.KnownProxies.Add(ip);
            }
        }
    }
});

services.AddSignalR(options =>
{
    options.EnableDetailedErrors = builder.Environment.IsDevelopment();
    options.MaximumReceiveMessageSize = 100_000;
})
    .AddJsonProtocol(options =>
    {
        options.PayloadSerializerOptions.PropertyNameCaseInsensitive = true;
    })
    .AddMessagePackProtocol();

services.AddRateLimiter(options =>
{
    options.AddConcurrencyLimiter(RatePolicyNames.AgentUpdateDownloads, clOptions =>
    {
        clOptions.QueueLimit = int.MaxValue;

        clOptions.PermitLimit =
            settings.MaxConcurrentUpdates <= 0 ?
                10 :
                settings.MaxConcurrentUpdates;
    });
});
services.AddHttpClient();
services.AddLogging();
services.AddScoped<IEmailSender, EmailSender>();
if (builder.Environment.IsDevelopment())
{
    services.AddScoped<IEmailSenderEx, EmailSenderFake>();
}
else
{
    services.AddScoped<IEmailSender<RemotelyUser>, EmailSenderEx>();
    services.AddScoped<IEmailSenderEx, EmailSenderEx>();
}
services.AddSingleton<IAppDbFactory, AppDbFactory>();
services.AddTransient<IDataService, DataService>();
services.AddTransient<IUserDirectoryService, UserDirectoryService>();
services.AddTransient<IOrganizationService, OrganizationService>();
services.AddTransient<IDeviceQueryService, DeviceQueryService>();
services.AddScoped<ApiAuthorizationFilter>();
services.AddScoped<LocalOnlyFilter>();
services.AddScoped<ExpiringTokenFilter>();
services.AddScoped<SignedMsiTokenFilter>();
services.AddHostedService<DataCleanupService>();
services.AddHostedService<ScriptScheduler>();
services.AddSingleton<IUpgradeService, UpgradeService>();
services.AddScoped<IToastService, ToastService>();
services.AddScoped<IModalService, ModalService>();
services.AddScoped<IJsInterop, JsInterop>();
services.AddScoped<ICircuitConnection, CircuitConnection>();
services.AddScoped<ILoaderService, LoaderService>();
services.AddScoped(x => (CircuitHandler)x.GetRequiredService<ICircuitConnection>());
services.AddSingleton<ICircuitManager, CircuitManager>();
services.AddScoped<IAuthService, AuthService>();
services.AddScoped<ISelectedCardsStore, SelectedCardsStore>();
services.AddScoped<IExpiringTokenService, ExpiringTokenService>();
services.AddScoped<ISignedMsiUrlService, SignedMsiUrlService>();
services.AddScoped<Remotely.Server.Services.AuditLog.IAuditLogService,
    Remotely.Server.Services.AuditLog.AuditLogService>();
services.AddScoped<IScriptScheduleDispatcher, ScriptScheduleDispatcher>();
services.AddSingleton<IOtpProvider, OtpProvider>();
services.AddSingleton<IEmbeddedServerDataProvider, EmbeddedServerDataProvider>();
services.AddSingleton<ILogsManager, LogsManager>();
services.AddScoped<IThemeProvider, ThemeProvider>();
services.AddScoped<IChatSessionStore, ChatSessionStore>();
services.AddScoped<ITerminalStore, TerminalStore>();
services.AddScoped<ViewerAuthorizationFilter>();
services.AddSingleton(WeakReferenceMessenger.Default);
services.AddSingleton<ISessionRecordingSink, SessionRecordingSink>();
services.AddSingleton<IDesktopStreamCache, DesktopStreamCache>();
services.AddSingleton<IRemoteControlSessionCache, RemoteControlSessionCache>();
services.AddSingleton<ISystemTime, SystemTime>();
services.AddSingleton<IAgentHubSessionCache, AgentHubSessionCache>();
services.AddScoped<IInstalledApplicationsService, InstalledApplicationsService>();
services.AddScoped<IPackageService, PackageService>();
services.AddScoped<IPackageInstallJobService, PackageInstallJobService>();
services.Configure<PackageInstallJobRateLimitOptions>(
    builder.Configuration.GetSection(PackageInstallJobRateLimitOptions.SectionName));
services.AddSingleton<IPackageInstallJobRateLimiter, PackageInstallJobRateLimiter>();
services.AddScoped<IUploadedMsiService, UploadedMsiService>();
// Background agent-upgrade pipeline (ROADMAP.md "M3 — Background
// agent-upgrade pipeline"). Service holds the state machine; the
// orchestrator is the IHostedService that sweeps eligible rows and
// dispatches upgrades through IAgentUpgradeDispatcher. The dispatcher
// is the manifest-backed implementation when a publisher manifest URL
// is configured (slice R8); otherwise the legacy no-op default is used
// so existing deployments remain stable.
services.AddScoped<Remotely.Server.Services.AgentUpgrade.IAgentUpgradeService,
    Remotely.Server.Services.AgentUpgrade.AgentUpgradeService>();
services.Configure<Remotely.Server.Services.AgentUpgrade.AgentUpgradeManifestOptions>(
    builder.Configuration.GetSection(
        Remotely.Server.Services.AgentUpgrade.AgentUpgradeManifestOptions.SectionName));
services.AddSingleton<Remotely.Server.Services.AgentUpgrade.IPublisherManifestProvider,
    Remotely.Server.Services.AgentUpgrade.PublisherManifestProvider>();

// Pick the manifest-backed dispatcher when at least one channel has a
// configured manifest URL; fall back to the no-op otherwise. The check
// is one-shot at startup so a runtime configuration change requires a
// restart (matches every other AgentUpgrade tunable).
var manifestSection = builder.Configuration.GetSection(
    Remotely.Server.Services.AgentUpgrade.AgentUpgradeManifestOptions.SectionName +
    ":ManifestUrls");
var hasAnyManifestUrl = manifestSection.GetChildren()
    .Any(c => !string.IsNullOrWhiteSpace(c.Value));
if (hasAnyManifestUrl)
{
    services.AddScoped<Remotely.Server.Services.AgentUpgrade.IAgentUpgradeDispatcher,
        Remotely.Server.Services.AgentUpgrade.ManifestBackedAgentUpgradeDispatcher>();
}
else
{
    services.AddScoped<Remotely.Server.Services.AgentUpgrade.IAgentUpgradeDispatcher,
        Remotely.Server.Services.AgentUpgrade.NoopAgentUpgradeDispatcher>();
}
services.Configure<Remotely.Server.Services.AgentUpgrade.AgentUpgradeOrchestratorOptions>(
    builder.Configuration.GetSection(
        Remotely.Server.Services.AgentUpgrade.AgentUpgradeOrchestratorOptions.SectionName));
services.AddHostedService<Remotely.Server.Services.AgentUpgrade.AgentUpgradeOrchestrator>();
// First-boot setup wizard skeleton (ROADMAP.md "M1 — First-boot setup wizard").
services.AddScoped<ISetupStateService, SetupStateService>();
// First-boot setup wizard — M1.1 (preflight) / M1.2 (DB connection) /
// M1.3 (legacy import) / wizard progress (ROADMAP.md "M1 — First-boot
// setup wizard").
services.AddScoped<Remotely.Server.Services.Setup.ISetupWizardProgressService,
    Remotely.Server.Services.Setup.SetupWizardProgressService>();
services.AddScoped<Remotely.Server.Services.Setup.IConnectionStringWriter,
    Remotely.Server.Services.Setup.ConnectionStringWriter>();
services.AddScoped<Remotely.Server.Services.Setup.IPreflightService,
    Remotely.Server.Services.Setup.PreflightService>();
services.AddScoped<Remotely.Server.Services.Setup.IDatabaseConnectionTester,
    Remotely.Server.Services.Setup.PostgresConnectionTester>();
services.AddScoped<Remotely.Server.Services.Setup.ISetupImportService,
    Remotely.Server.Services.Setup.SetupImportService>();
services.AddScoped<Remotely.Server.Services.Setup.IAdminBootstrapService,
    Remotely.Server.Services.Setup.AdminBootstrapService>();
services.AddHostedService<RemoteControlSessionCleaner>();
services.AddHostedService<RemoteControlSessionReconnector>();

builder.Services.AddEndpointsApiExplorer();
builder.Services.AddSwaggerGen();

var app = builder.Build();

app.UseForwardedHeaders();

app.UseRateLimiter();

if (settings.UseHttpLogging)
{
    app.UseHttpLogging();
}

if (app.Environment.IsDevelopment())
{
    app.UseDeveloperExceptionPage();
    app.UseMigrationsEndPoint();
}
else
{
    app.UseExceptionHandler("/Error");
    if (settings.UseHsts)
    {
        app.UseHsts();
    }
    if (settings.RedirectToHttps)
    {
        app.UseHttpsRedirection();
    }
}

// Track S / S7 — Runtime security posture. Sets the CMRemote security
// headers baseline (Content-Security-Policy, X-Content-Type-Options,
// Referrer-Policy, Permissions-Policy, X-Frame-Options, COOP/CORP) on
// every response. Registered before UseRouting so framework-short-circuit
// responses (404s, redirects, the setup-redirect middleware's 503) all
// pick up the headers.
app.UseMiddleware<SecurityHeadersMiddleware>();

app.UseSwagger();
app.UseSwaggerUI();

ConfigureStaticFiles();

app.UseRouting();

// Redirect uncompleted first-boot setups to /setup. Must come after
// UseRouting (so PathString segment matches work as expected) but before
// auth so the wizard is reachable without a signed-in user.
app.UseMiddleware<SetupRedirectMiddleware>();

app.UseAuthentication();
app.UseAuthorization();
app.UseCors("TrustedOriginPolicy");

app.UseAntiforgery();


app.MapRazorPages();
app.MapHub<DesktopHub>("/hubs/desktop");
app.MapHub<ViewerHub>("/hubs/viewer");
app.MapHub<AgentHub>("/hubs/service");
app.MapControllers();

app.MapRazorComponents<App>()
    .AddInteractiveServerRenderMode();

app.MapAdditionalIdentityEndpoints();

using (var scope = app.Services.CreateScope())
{
    var dataService = scope.ServiceProvider.GetRequiredService<IDataService>();

    await dataService.SetAllDevicesNotOnline();
    await dataService.CleanupOldRecords();

    // First-boot setup wizard (ROADMAP.md "M1 — First-boot setup wizard"):
    // if the database already contains operator-visible state, write the
    // CMRemote.Setup.Completed marker so existing deployments are not
    // hijacked into the wizard on upgrade.
    var setupState = scope.ServiceProvider.GetRequiredService<ISetupStateService>();
    await setupState.EnsureMarkerForExistingDeploymentAsync();
}

await app.RunAsync();

void ConfigureStaticFiles()
{
    var provider = new FileExtensionContentTypeProvider();
    // Add new mappings
    provider.Mappings[".ps1"] = "application/octet-stream";
    provider.Mappings[".exe"] = "application/octet-stream";
    provider.Mappings[".dll"] = "application/octet-stream";
    provider.Mappings[".appimage"] = "application/octet-stream";
    provider.Mappings[".zip"] = "application/octet-stream";
    provider.Mappings[".config"] = "application/octet-stream";
    app.UseStaticFiles();
    var contentPath = Path.Combine(app.Environment.WebRootPath, "Content");

    if (Directory.Exists(contentPath))
    {
        app.UseStaticFiles(new StaticFileOptions()
        {
            FileProvider = new PhysicalFileProvider(Path.Combine(app.Environment.WebRootPath, "Content")),
            ServeUnknownFileTypes = true,
            RequestPath = new PathString("/Content"),
            ContentTypeProvider = provider,
            DefaultContentType = "application/octet-stream"
        });
    }

    // Needed for Let's Encrypt.
    if (Directory.Exists(Path.Combine(app.Environment.ContentRootPath, ".well-known")))
    {
        app.UseStaticFiles(new StaticFileOptions
        {
            FileProvider = new PhysicalFileProvider(Path.Combine(app.Environment.ContentRootPath, @".well-known")),
            RequestPath = new PathString("/.well-known"),
            ServeUnknownFileTypes = true
        });
    }
}

void ConfigureSerilog(WebApplicationBuilder webAppBuilder, SettingsModel settings)
{
    try
    {
        var dataRetentionDays = settings.DataRetentionInDays;
        if (dataRetentionDays <= 0)
        {
            dataRetentionDays = 7;
        }

        var logPath = LogsManager.DefaultLogsDirectory;

        void ApplySharedLoggerConfig(LoggerConfiguration loggerConfiguration)
        {
            loggerConfiguration
                .Enrich.FromLogContext()
                .Enrich.WithThreadId()
                .WriteTo.Console(outputTemplate: "[{Timestamp:HH:mm:ss} {Level:u3}] {Message:lj} {Properties}{NewLine}{Exception}")
                .WriteTo.File($"{logPath}/Remotely_Server.log",
                    rollingInterval: RollingInterval.Day,
                    retainedFileTimeLimit: TimeSpan.FromDays(dataRetentionDays),
                    outputTemplate: "{Timestamp:yyyy-MM-dd HH:mm:ss.fff zzz} [{Level:u3}] {Message:lj} {Properties}{NewLine}{Exception}",
                    shared: true);
        }

        // https://github.com/serilog/serilog-aspnetcore#two-stage-initialization
        var loggerConfig = new LoggerConfiguration();
        ApplySharedLoggerConfig(loggerConfig);
        Log.Logger = loggerConfig.CreateBootstrapLogger();

        builder.Host.UseSerilog((context, services, configuration) =>
        {
            configuration
                .ReadFrom.Configuration(context.Configuration)
                .ReadFrom.Services(services);

            ApplySharedLoggerConfig(configuration);
        });
    }
    catch (Exception ex)
    {
        Console.WriteLine($"Failed to configure Serilog file logging.  Error: {ex.Message}");
    }
}