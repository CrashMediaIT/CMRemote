using Microsoft.Extensions.Logging;
using Microsoft.Win32;
using Remotely.Agent.Interfaces;
using Remotely.Shared.Enums;
using Remotely.Shared.Models;
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Services.Windows;

/// <summary>
/// Windows implementation of <see cref="IInstalledApplicationsProvider"/>.
///
/// <para>Enumeration sources:</para>
/// <list type="bullet">
///   <item><c>HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall</c></item>
///   <item><c>HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall</c></item>
///   <item><c>HKU\&lt;sid&gt;\Software\Microsoft\Windows\CurrentVersion\Uninstall</c> for each loaded user hive</item>
///   <item><c>Get-AppxPackage -AllUsers</c> via <c>powershell.exe</c></item>
/// </list>
///
/// <para>Uninstall strategy (in order of preference):</para>
/// <list type="number">
///   <item>AppX → <c>Remove-AppxPackage -AllUsers -Package &lt;PackageFullName&gt;</c></item>
///   <item>MSI (<c>WindowsInstaller=1</c>) → <c>msiexec.exe /x {ProductCode} /qn /norestart</c></item>
///   <item>Win32 with <c>QuietUninstallString</c> set → execute it</item>
///   <item>Otherwise → return not-silent failure (UI must surface a confirmation;
///         non-silent uninstalls are not auto-executed in Phase 1)</item>
/// </list>
/// </summary>
[SupportedOSPlatform("windows")]
public class WindowsInstalledApplicationsProvider : IInstalledApplicationsProvider
{
    private const string UninstallSubKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    private const string Wow64UninstallSubKeyPath = @"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

    // GUID matcher used both to detect MSI ProductCode subkeys and to
    // extract a ProductCode from a UninstallString. Anchored at parse
    // time to avoid false matches inside paths.
    private static readonly Regex _productCodeRegex = new(
        @"\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}",
        RegexOptions.Compiled);

    private const int MaxOutputCharacters = 16 * 1024;

    // 15 minutes accommodates legitimately slow MSI uninstalls (Office,
    // Adobe Creative Cloud, Visual Studio components) on spinning disks
    // while still bounding rogue installers. AppX removals and most
    // QuietUninstallString invocations finish in seconds.
    private static readonly TimeSpan UninstallTimeout = TimeSpan.FromMinutes(15);

    private readonly ILogger<WindowsInstalledApplicationsProvider> _logger;

    public WindowsInstalledApplicationsProvider(ILogger<WindowsInstalledApplicationsProvider> logger)
    {
        _logger = logger;
    }

    public bool IsSupported => true;

    public async Task<(bool Success, string? ErrorMessage, IReadOnlyList<InstalledApplication> Applications)> GetInstalledApplicationsAsync(CancellationToken cancellationToken)
    {
        try
        {
            var apps = new List<InstalledApplication>(256);
            var seenKeys = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

            void AddRange(IEnumerable<InstalledApplication> items)
            {
                foreach (var item in items)
                {
                    // Dedupe by (Source, ApplicationKey). Win32 apps can
                    // appear in both 32-bit and 64-bit views with the
                    // same subkey name; we prefer the first hit.
                    var dedupeKey = $"{item.Source}|{item.ApplicationKey}";
                    if (seenKeys.Add(dedupeKey))
                    {
                        apps.Add(item);
                    }
                }
            }

            AddRange(EnumerateRegistryHive(Registry.LocalMachine, UninstallSubKeyPath));
            AddRange(EnumerateRegistryHive(Registry.LocalMachine, Wow64UninstallSubKeyPath));

            // Each loaded user hive (skipping the well-known service
            // SIDs which never have application installs).
            foreach (var sid in EnumerateUserSids())
            {
                cancellationToken.ThrowIfCancellationRequested();
                using var userHive = Registry.Users.OpenSubKey(sid);
                if (userHive is null)
                {
                    continue;
                }
                AddRange(EnumerateRegistryHiveFromKey(userHive, UninstallSubKeyPath));
            }

            cancellationToken.ThrowIfCancellationRequested();

            var (appxOk, appxApps, appxErr) = await EnumerateAppxAsync(cancellationToken).ConfigureAwait(false);
            if (appxOk)
            {
                AddRange(appxApps);
            }
            else if (!string.IsNullOrEmpty(appxErr))
            {
                _logger.LogWarning("AppX enumeration failed: {error}", appxErr);
            }

            apps.Sort((a, b) => string.Compare(a.Name, b.Name, StringComparison.OrdinalIgnoreCase));
            return (true, null, apps);
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to enumerate installed applications.");
            return (false, ex.Message, Array.Empty<InstalledApplication>());
        }
    }

    public async Task<(bool Success, int ExitCode, string? Stdout, string? Stderr, string? ErrorMessage)> UninstallApplicationAsync(string applicationKey, CancellationToken cancellationToken)
    {
        if (string.IsNullOrWhiteSpace(applicationKey))
        {
            return (false, -1, null, null, "Application key is required.");
        }

        try
        {
            // Re-enumerate locally so we always operate on a freshly
            // resolved uninstall command — never trust an executable
            // string from the network.
            var (enumOk, enumErr, apps) = await GetInstalledApplicationsAsync(cancellationToken).ConfigureAwait(false);
            if (!enumOk)
            {
                return (false, -1, null, null, $"Failed to enumerate applications: {enumErr}");
            }

            var target = apps.FirstOrDefault(a =>
                string.Equals(a.ApplicationKey, applicationKey, StringComparison.OrdinalIgnoreCase));

            if (target is null)
            {
                return (false, -1, null, null, "Application not found in current inventory.");
            }

            _logger.LogInformation(
                "Uninstall requested. Source={source} Name={name} Version={version} Publisher={publisher} Key={key}",
                target.Source, target.Name, target.Version, target.Publisher, target.ApplicationKey);

            return target.Source switch
            {
                InstalledApplicationSource.Appx => await RunProcessAsync(
                    "powershell.exe",
                    [
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy", "Bypass",
                        "-Command",
                        $"Remove-AppxPackage -AllUsers -Package '{EscapeForSingleQuotedPS(target.ApplicationKey)}'"
                    ],
                    cancellationToken).ConfigureAwait(false),

                InstalledApplicationSource.Msi => await UninstallMsiAsync(target, cancellationToken).ConfigureAwait(false),

                InstalledApplicationSource.Win32 => await UninstallWin32Async(target, cancellationToken).ConfigureAwait(false),

                _ => (false, -1, null, null, "Unsupported application source."),
            };
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to uninstall application {key}.", applicationKey);
            return (false, -1, null, null, ex.Message);
        }
    }

    // --- Registry enumeration -------------------------------------------------

    private IEnumerable<InstalledApplication> EnumerateRegistryHive(RegistryKey hive, string subKeyPath)
    {
        using var root = hive.OpenSubKey(subKeyPath);
        if (root is null)
        {
            yield break;
        }
        foreach (var app in EnumerateUninstallEntries(root))
        {
            yield return app;
        }
    }

    private IEnumerable<InstalledApplication> EnumerateRegistryHiveFromKey(RegistryKey rootHive, string subKeyPath)
    {
        using var root = rootHive.OpenSubKey(subKeyPath);
        if (root is null)
        {
            yield break;
        }
        foreach (var app in EnumerateUninstallEntries(root))
        {
            yield return app;
        }
    }

    private IEnumerable<InstalledApplication> EnumerateUninstallEntries(RegistryKey root)
    {
        foreach (var subKeyName in root.GetSubKeyNames())
        {
            InstalledApplication? app = null;
            try
            {
                using var sub = root.OpenSubKey(subKeyName);
                if (sub is null)
                {
                    continue;
                }
                app = ReadUninstallEntry(sub, subKeyName);
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex, "Failed to read uninstall registry entry {subKey}.", subKeyName);
            }

            if (app is not null)
            {
                yield return app;
            }
        }
    }

    private static InstalledApplication? ReadUninstallEntry(RegistryKey key, string subKeyName)
    {
        var displayName = key.GetValue("DisplayName") as string;
        if (string.IsNullOrWhiteSpace(displayName))
        {
            // Per Microsoft's docs on Add/Remove Programs, entries
            // without a DisplayName are not user-visible apps.
            return null;
        }

        // Filter out OS components and updates.
        var systemComponent = ReadInt(key, "SystemComponent") == 1;
        var releaseType = key.GetValue("ReleaseType") as string;
        var parentKey = key.GetValue("ParentKeyName") as string;
        var parentDisplay = key.GetValue("ParentDisplayName") as string;

        var isSystemComponent = systemComponent ||
            string.Equals(releaseType, "Update", StringComparison.OrdinalIgnoreCase) ||
            string.Equals(releaseType, "Hotfix", StringComparison.OrdinalIgnoreCase) ||
            string.Equals(releaseType, "Security Update", StringComparison.OrdinalIgnoreCase) ||
            !string.IsNullOrEmpty(parentKey) ||
            !string.IsNullOrEmpty(parentDisplay);

        var uninstallString = key.GetValue("UninstallString") as string;
        var quietUninstallString = key.GetValue("QuietUninstallString") as string;
        var windowsInstaller = ReadInt(key, "WindowsInstaller") == 1;

        // Source attribution: MSI when WindowsInstaller=1 OR the subkey
        // name is a GUID ProductCode (the latter is the canonical signal
        // for Windows Installer products).
        var isMsi = windowsInstaller || _productCodeRegex.IsMatch(subKeyName);
        var source = isMsi ? InstalledApplicationSource.Msi : InstalledApplicationSource.Win32;

        bool canUninstallSilently;
        if (isMsi && _productCodeRegex.IsMatch(subKeyName))
        {
            canUninstallSilently = true;
        }
        else if (!string.IsNullOrWhiteSpace(quietUninstallString))
        {
            canUninstallSilently = true;
        }
        else
        {
            canUninstallSilently = false;
        }

        var sizeKb = ReadInt(key, "EstimatedSize");

        return new InstalledApplication
        {
            ApplicationKey = subKeyName,
            Source = source,
            Name = displayName!,
            Version = key.GetValue("DisplayVersion") as string,
            Publisher = key.GetValue("Publisher") as string,
            InstallDate = NormalizeRegistryInstallDate(key.GetValue("InstallDate") as string),
            SizeBytes = sizeKb is > 0 ? sizeKb * 1024L : null,
            InstallLocation = key.GetValue("InstallLocation") as string,
            IsSystemComponent = isSystemComponent,
            CanUninstallSilently = canUninstallSilently,
        };
    }

    private static int? ReadInt(RegistryKey key, string name)
    {
        var value = key.GetValue(name);
        return value switch
        {
            int i => i,
            long l => (int)l,
            string s when int.TryParse(s, out var parsed) => parsed,
            _ => null,
        };
    }

    private static string? NormalizeRegistryInstallDate(string? raw)
    {
        // Registry InstallDate is yyyyMMdd (no separators).
        if (string.IsNullOrWhiteSpace(raw) || raw.Length != 8)
        {
            return raw;
        }
        if (DateTime.TryParseExact(raw, "yyyyMMdd", System.Globalization.CultureInfo.InvariantCulture,
                System.Globalization.DateTimeStyles.None, out var parsed))
        {
            return parsed.ToString("yyyy-MM-dd");
        }
        return raw;
    }

    private IEnumerable<string> EnumerateUserSids()
    {
        // Skip well-known service / built-in SIDs. Real user hives use
        // S-1-5-21-* (and a "_Classes" suffix variant we also skip).
        foreach (var sid in Registry.Users.GetSubKeyNames())
        {
            if (!sid.StartsWith("S-1-5-21", StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            if (sid.EndsWith("_Classes", StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            yield return sid;
        }
    }

    // --- AppX enumeration -----------------------------------------------------

    private async Task<(bool Success, IReadOnlyList<InstalledApplication> Apps, string? Error)> EnumerateAppxAsync(CancellationToken cancellationToken)
    {
        // Get-AppxPackage requires Windows PowerShell (powershell.exe). We
        // intentionally do not load it via Microsoft.PowerShell.SDK to
        // avoid pinning the in-process pwsh runspace to a specific
        // Windows feature.
        const string command =
            "Get-AppxPackage -AllUsers " +
            "| Where-Object { -not $_.IsFramework -and $_.SignatureKind -ne 'None' } " +
            "| Select-Object Name,PackageFullName,Publisher,Version,InstallLocation,SignatureKind,IsFramework " +
            "| ConvertTo-Json -Depth 3 -Compress";

        var (success, exitCode, stdout, stderr, error) = await RunProcessAsync(
            "powershell.exe",
            ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command],
            cancellationToken).ConfigureAwait(false);

        if (!success || exitCode != 0)
        {
            return (false, Array.Empty<InstalledApplication>(), error ?? stderr ?? $"Get-AppxPackage exited with {exitCode}");
        }

        if (string.IsNullOrWhiteSpace(stdout))
        {
            return (true, Array.Empty<InstalledApplication>(), null);
        }

        return ParseAppxJson(stdout!);
    }

    /// <summary>
    /// Parses the JSON emitted by <c>Get-AppxPackage | ConvertTo-Json</c>.
    /// PowerShell emits a single object when there is one result, an
    /// array otherwise — both shapes are handled. Public for unit tests.
    /// </summary>
    public static (bool Success, IReadOnlyList<InstalledApplication> Apps, string? Error) ParseAppxJson(string json)
    {
        try
        {
            using var doc = JsonDocument.Parse(json);
            var apps = new List<InstalledApplication>();

            void ConsumeElement(JsonElement el)
            {
                if (el.ValueKind != JsonValueKind.Object)
                {
                    return;
                }

                var packageFullName = TryGetString(el, "PackageFullName");
                if (string.IsNullOrWhiteSpace(packageFullName))
                {
                    return;
                }

                var isFramework = el.TryGetProperty("IsFramework", out var ifEl)
                    && ifEl.ValueKind == JsonValueKind.True;
                if (isFramework)
                {
                    return;
                }

                var name = TryGetString(el, "Name") ?? packageFullName;

                apps.Add(new InstalledApplication
                {
                    ApplicationKey = packageFullName!,
                    Source = InstalledApplicationSource.Appx,
                    Name = name,
                    Version = TryGetString(el, "Version"),
                    Publisher = TryGetString(el, "Publisher"),
                    InstallLocation = TryGetString(el, "InstallLocation"),
                    IsSystemComponent =
                        string.Equals(TryGetString(el, "SignatureKind"), "System", StringComparison.OrdinalIgnoreCase),
                    CanUninstallSilently = true,
                });
            }

            if (doc.RootElement.ValueKind == JsonValueKind.Array)
            {
                foreach (var item in doc.RootElement.EnumerateArray())
                {
                    ConsumeElement(item);
                }
            }
            else
            {
                ConsumeElement(doc.RootElement);
            }

            return (true, apps, null);
        }
        catch (JsonException ex)
        {
            return (false, Array.Empty<InstalledApplication>(), $"Invalid AppX JSON: {ex.Message}");
        }
    }

    private static string? TryGetString(JsonElement el, string name)
    {
        if (!el.TryGetProperty(name, out var prop))
        {
            return null;
        }
        return prop.ValueKind switch
        {
            JsonValueKind.String => prop.GetString(),
            JsonValueKind.Null => null,
            _ => prop.ToString(),
        };
    }

    // --- Uninstall helpers ----------------------------------------------------

    private async Task<(bool Success, int ExitCode, string? Stdout, string? Stderr, string? ErrorMessage)> UninstallMsiAsync(InstalledApplication target, CancellationToken cancellationToken)
    {
        var match = _productCodeRegex.Match(target.ApplicationKey);
        if (!match.Success)
        {
            return (false, -1, null, null, "MSI product code not found in application key.");
        }

        var productCode = match.Value;
        return await RunProcessAsync(
            "msiexec.exe",
            ["/x", productCode, "/qn", "/norestart"],
            cancellationToken).ConfigureAwait(false);
    }

    private async Task<(bool Success, int ExitCode, string? Stdout, string? Stderr, string? ErrorMessage)> UninstallWin32Async(InstalledApplication target, CancellationToken cancellationToken)
    {
        if (!target.CanUninstallSilently)
        {
            return (false, -1, null, null,
                "Application does not advertise a silent uninstall command. " +
                "Phase 1 only supports silent uninstalls; please uninstall manually.");
        }

        // CanUninstallSilently=true here means QuietUninstallString was
        // set. Re-read it from the registry rather than caching it on the
        // wire/object — defense-in-depth.
        var quietCommand = ReadQuietUninstallString(target.ApplicationKey);
        if (string.IsNullOrWhiteSpace(quietCommand))
        {
            return (false, -1, null, null, "QuietUninstallString missing at uninstall time.");
        }

        var (file, args) = SplitCommandLine(quietCommand!);
        if (string.IsNullOrWhiteSpace(file))
        {
            return (false, -1, null, null, "Could not parse QuietUninstallString.");
        }

        return await RunProcessAsync(file!, args, cancellationToken).ConfigureAwait(false);
    }

    private static string? ReadQuietUninstallString(string subKeyName)
    {
        string? Read(RegistryKey hive, string path)
        {
            using var root = hive.OpenSubKey(path);
            using var sub = root?.OpenSubKey(subKeyName);
            return sub?.GetValue("QuietUninstallString") as string;
        }

        return Read(Registry.LocalMachine, UninstallSubKeyPath)
            ?? Read(Registry.LocalMachine, Wow64UninstallSubKeyPath);
    }

    /// <summary>
    /// Splits a Windows command line into an executable path and an
    /// argument list. Handles both <c>"C:\path with space\u.exe" /S</c>
    /// and unquoted forms. Public for unit tests.
    /// </summary>
    public static (string? File, IReadOnlyList<string> Args) SplitCommandLine(string commandLine)
    {
        if (string.IsNullOrWhiteSpace(commandLine))
        {
            return (null, Array.Empty<string>());
        }

        var trimmed = commandLine.Trim();
        string file;
        string remainder;

        if (trimmed.StartsWith('"'))
        {
            var closing = trimmed.IndexOf('"', 1);
            if (closing < 0)
            {
                return (null, Array.Empty<string>());
            }
            file = trimmed.Substring(1, closing - 1);
            remainder = trimmed.Length > closing + 1 ? trimmed[(closing + 1)..].TrimStart() : string.Empty;
        }
        else
        {
            var firstSpace = trimmed.IndexOf(' ');
            if (firstSpace < 0)
            {
                file = trimmed;
                remainder = string.Empty;
            }
            else
            {
                file = trimmed[..firstSpace];
                remainder = trimmed[(firstSpace + 1)..].TrimStart();
            }
        }

        return (file, ParseArgs(remainder));
    }

    private static List<string> ParseArgs(string remainder)
    {
        var args = new List<string>();
        if (string.IsNullOrEmpty(remainder))
        {
            return args;
        }

        var sb = new StringBuilder();
        var inQuotes = false;
        for (var i = 0; i < remainder.Length; i++)
        {
            var c = remainder[i];
            if (c == '"')
            {
                inQuotes = !inQuotes;
                continue;
            }
            if (c == ' ' && !inQuotes)
            {
                if (sb.Length > 0)
                {
                    args.Add(sb.ToString());
                    sb.Clear();
                }
                continue;
            }
            sb.Append(c);
        }
        if (sb.Length > 0)
        {
            args.Add(sb.ToString());
        }
        return args;
    }

    private static string EscapeForSingleQuotedPS(string s) => s.Replace("'", "''");

    // --- Process runner -------------------------------------------------------

    /// <summary>
    /// Runs a process with the given file and arguments using
    /// <see cref="ProcessStartInfo.ArgumentList"/> (no shell), captures
    /// stdout/stderr (truncated), and enforces a hard timeout with
    /// process termination on cancellation.
    /// </summary>
    private async Task<(bool Success, int ExitCode, string? Stdout, string? Stderr, string? ErrorMessage)> RunProcessAsync(
        string fileName,
        IReadOnlyList<string> arguments,
        CancellationToken cancellationToken)
    {
        var psi = new ProcessStartInfo
        {
            FileName = fileName,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
        };
        foreach (var arg in arguments)
        {
            psi.ArgumentList.Add(arg);
        }

        using var process = new Process { StartInfo = psi };
        var stdoutBuilder = new StringBuilder();
        var stderrBuilder = new StringBuilder();

        process.OutputDataReceived += (_, e) =>
        {
            if (e.Data is null)
            {
                return;
            }
            if (stdoutBuilder.Length < MaxOutputCharacters)
            {
                stdoutBuilder.AppendLine(e.Data);
            }
        };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is null)
            {
                return;
            }
            if (stderrBuilder.Length < MaxOutputCharacters)
            {
                stderrBuilder.AppendLine(e.Data);
            }
        };

        try
        {
            if (!process.Start())
            {
                return (false, -1, null, null, "Process failed to start.");
            }
        }
        catch (Exception ex)
        {
            return (false, -1, null, null, ex.Message);
        }

        process.BeginOutputReadLine();
        process.BeginErrorReadLine();

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(UninstallTimeout);

        try
        {
            await process.WaitForExitAsync(timeoutCts.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            try
            {
                if (!process.HasExited)
                {
                    process.Kill(entireProcessTree: true);
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Failed to terminate process {file} after timeout.", fileName);
            }
            return (false, -1, Truncate(stdoutBuilder.ToString()), Truncate(stderrBuilder.ToString()),
                cancellationToken.IsCancellationRequested ? "Cancelled." : "Timed out.");
        }

        return (process.ExitCode == 0, process.ExitCode,
            Truncate(stdoutBuilder.ToString()), Truncate(stderrBuilder.ToString()), null);
    }

    private static string? Truncate(string s)
    {
        if (string.IsNullOrEmpty(s))
        {
            return null;
        }
        return s.Length > MaxOutputCharacters ? s[..MaxOutputCharacters] : s;
    }
}
