using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Remotely.Agent.Interfaces;
using Remotely.Agent.Services;
using Remotely.Shared.Utilities;
using Remotely.Shared.Services;
using System;
using System.IO;
using System.Threading.Tasks;
using System.Runtime.Versioning;
using Remotely.Agent.Services.Linux;
using Remotely.Agent.Services.MacOS;
using Remotely.Agent.Services.Windows;
using Microsoft.Extensions.Hosting;
using System.Linq;
using Microsoft.Win32;
using System.Reflection;

namespace Remotely.Agent;

public class Program
{
    public static async Task Main(string[] args)
    {
        try
        {
            var host = Host
                .CreateDefaultBuilder(args)
                .UseWindowsService()
                .UseSystemd()
                .ConfigureServices(RegisterServices)
                .Build();

            await host.StartAsync();

            await Init(host.Services);

            await host.WaitForShutdownAsync();
        }
        catch (Exception ex)
        {
            var version = AppVersionHelper.GetAppVersion();
            var componentName = Assembly.GetExecutingAssembly().GetName().Name;
            var logger = new FileLogger($"{componentName}", version, "Main");
            logger.LogError(ex, "Error during agent startup.");
            throw;
        }
    }

    private static async Task Init(IServiceProvider services)
    {
        var logger = services.GetRequiredService<ILogger<IHost>>();

        AppDomain.CurrentDomain.UnhandledException += (sender, ex) =>
        {
            if (ex.ExceptionObject is Exception exception)
            {
                logger.LogError(exception, "Unhandled exception in AppDomain.");
            }
            else
            {
                logger.LogError("Unhandled exception in AppDomain.");
            }
        };

        SetWorkingDirectory();

        if (OperatingSystem.IsWindows())
        {
            SetSas(services, logger);
        }

        // TODO: Move this to a BackgroundService.
        await services.GetRequiredService<IUpdater>().BeginChecking();
        await services.GetRequiredService<IAgentHubConnection>().Connect();
    }

    private static void RegisterServices(IServiceCollection services)
    {
        services.AddHttpClient();
        services.AddLogging(builder =>
        {
            builder.AddConsole().AddDebug();
            var version = AppVersionHelper.GetAppVersion();
            var componentName = Assembly.GetExecutingAssembly().GetName().Name;
            builder.AddProvider(new FileLoggerProvider($"{componentName}", version));
        });

        services.AddSingleton<IAgentHubConnection, AgentHubConnection>();
        services.AddSingleton<ICpuUtilizationSampler, CpuUtilizationSampler>();
        services.AddSingleton<IWakeOnLanService, WakeOnLanService>();
        services.AddHostedService(services => services.GetRequiredService<ICpuUtilizationSampler>());
        services.AddSingleton<IChatClientService, ChatClientService>();
        services.AddTransient<IPsCoreShell, PsCoreShell>();
        services.AddTransient<IExternalScriptingShell, ExternalScriptingShell>();
        services.AddSingleton<IConfigService, ConfigService>();
        services.AddSingleton<IUninstaller, Uninstaller>();
        services.AddSingleton<IScriptExecutor, ScriptExecutor>();
        services.AddSingleton<IProcessInvoker, ProcessInvoker>();
        services.AddSingleton<IUpdateDownloader, UpdateDownloader>();
        services.AddSingleton<IFileLogsManager, FileLogsManager>();
        services.AddSingleton<IScriptingShellFactory, ScriptingShellFactory>();

        if (OperatingSystem.IsWindows())
        {
            services.AddSingleton<IAppLauncher, AppLauncherWin>();
            services.AddSingleton<IUpdater, UpdaterWin>();
            services.AddSingleton<IDeviceInformationService, DeviceInfoGeneratorWin>();
            services.AddSingleton<IElevationDetector, ElevationDetectorWin>();
            services.AddSingleton<IInstalledApplicationsProvider, WindowsInstalledApplicationsProvider>();
            // Per-provider implementations register as IPackageProvider so
            // the composite picks them up via IEnumerable<IPackageProvider>.
            // The composite itself is registered under ICompositePackageProvider
            // and re-exported as the *single* IPackageProvider the hub resolves
            // (last AddSingleton wins for a single-resolve), so the hub keeps
            // its single dependency and adding new providers is a one-line
            // change here.
            services.AddSingleton<IPackageProvider, ChocolateyPackageProvider>();
            services.AddSingleton<IPackageProvider, MsiPackageInstaller>();
            services.AddSingleton<ICompositePackageProvider, CompositePackageProvider>();
            services.AddSingleton<IPackageProvider>(sp => sp.GetRequiredService<ICompositePackageProvider>());
        }
        else if (OperatingSystem.IsLinux())
        {
            services.AddSingleton<IAppLauncher, AppLauncherLinux>();
            services.AddSingleton<IUpdater, UpdaterLinux>();
            services.AddSingleton<IDeviceInformationService, DeviceInfoGeneratorLinux>();
            services.AddSingleton<IElevationDetector, ElevationDetectorLinux>();
            services.AddSingleton<IInstalledApplicationsProvider, NotSupportedInstalledApplicationsProvider>();
            services.AddSingleton<IPackageProvider, NotSupportedPackageProvider>();
        }
        else if (OperatingSystem.IsMacOS())
        {
            services.AddSingleton<IAppLauncher, AppLauncherMac>();
            services.AddSingleton<IUpdater, UpdaterMac>();
            services.AddSingleton<IDeviceInformationService, DeviceInfoGeneratorMac>();
            services.AddSingleton<IElevationDetector, ElevationDetectorMac>();
            services.AddSingleton<IInstalledApplicationsProvider, NotSupportedInstalledApplicationsProvider>();
            services.AddSingleton<IPackageProvider, NotSupportedPackageProvider>();
        }
        else
        {
            throw new NotSupportedException("Operating system not supported.");
        }
    }

    [SupportedOSPlatform("windows")]
    private static void SetSas(IServiceProvider services, ILogger<IHost> logger)
    {
        try
        {
            var elevationDetector = services.GetRequiredService<IElevationDetector>();
            if (elevationDetector.IsElevated())
            {
                // Set Secure Attention Sequence policy to allow app to simulate Ctrl + Alt + Del.
                var subkey = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System", true);
                subkey?.SetValue("SoftwareSASGeneration", "3", RegistryValueKind.DWord);
            }
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "Error while setting Secure Attention Sequence in the registry.");
        }
    }

    private static void SetWorkingDirectory()
    {
        var exePath = Environment.ProcessPath ?? Environment.GetCommandLineArgs().First();
        var exeDir = Path.GetDirectoryName(exePath) ?? AppDomain.CurrentDomain.BaseDirectory;
        Directory.SetCurrentDirectory(exeDir);
    }
}
