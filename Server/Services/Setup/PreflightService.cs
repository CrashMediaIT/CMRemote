using System.Runtime.InteropServices;

namespace Remotely.Server.Services.Setup;

/// <inheritdoc cref="IPreflightService" />
public class PreflightService : IPreflightService
{
    private readonly IConnectionStringWriter _connectionStringWriter;
    private readonly IConfiguration _configuration;
    private readonly ILogger<PreflightService> _logger;

    public PreflightService(
        IConnectionStringWriter connectionStringWriter,
        IConfiguration configuration,
        ILogger<PreflightService> logger)
    {
        _connectionStringWriter = connectionStringWriter;
        _configuration = configuration;
        _logger = logger;
    }

    /// <inheritdoc />
    public Task<PreflightReport> RunChecksAsync(
        CancellationToken cancellationToken = default)
    {
        var checks = new List<PreflightCheckResult>
        {
            CheckWritableDataDir(),
            CheckTlsConfigured(),
            CheckBindPorts(),
        };

        return Task.FromResult(new PreflightReport { Checks = checks });
    }

    private PreflightCheckResult CheckWritableDataDir()
    {
        // The wizard's M1.2 step writes appsettings.Production.json
        // through IConnectionStringWriter, so the canonical "writable
        // data dir" check is "can the writer touch its target file?".
        // We do this by attempting an atomic create-and-delete in the
        // target directory, which never modifies the real settings
        // file.
        var targetPath = _connectionStringWriter.TargetSettingsPath;
        var directory = Path.GetDirectoryName(targetPath);
        if (string.IsNullOrEmpty(directory))
        {
            return new PreflightCheckResult(
                "Writable data directory",
                PreflightStatus.Failed,
                $"Could not derive directory from settings path '{targetPath}'.");
        }

        try
        {
            Directory.CreateDirectory(directory);
            var probe = Path.Combine(directory, $".cm-preflight-{Guid.NewGuid():N}");
            File.WriteAllText(probe, string.Empty);
            File.Delete(probe);
        }
        catch (Exception ex) when (
            ex is UnauthorizedAccessException
            or IOException
            or NotSupportedException)
        {
            _logger.LogWarning(ex,
                "Preflight: data directory {Directory} is not writable.", directory);
            return new PreflightCheckResult(
                "Writable data directory",
                PreflightStatus.Failed,
                $"'{directory}' is not writable: {ex.Message}");
        }

        return new PreflightCheckResult(
            "Writable data directory",
            PreflightStatus.Passed,
            directory);
    }

    private PreflightCheckResult CheckTlsConfigured()
    {
        // ASPNETCORE_URLS / Kestrel:Endpoints both accept https://…
        // bindings. We treat HTTPS as advisory rather than mandatory
        // because the upstream Docker image is routinely deployed
        // behind an external reverse proxy that terminates TLS.
        var urls = _configuration["ASPNETCORE_URLS"]
                   ?? _configuration["urls"]
                   ?? string.Empty;
        var endpointsHasHttps = false;
        var endpoints = _configuration.GetSection("Kestrel:Endpoints").GetChildren();
        foreach (var endpoint in endpoints)
        {
            var url = endpoint["Url"];
            if (!string.IsNullOrEmpty(url) &&
                url.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
            {
                endpointsHasHttps = true;
                break;
            }
        }

        if (urls.Contains("https://", StringComparison.OrdinalIgnoreCase) ||
            endpointsHasHttps)
        {
            return new PreflightCheckResult(
                "TLS endpoint configured",
                PreflightStatus.Passed,
                "At least one HTTPS binding is configured.");
        }

        return new PreflightCheckResult(
            "TLS endpoint configured",
            PreflightStatus.Warning,
            "No HTTPS binding found in ASPNETCORE_URLS or Kestrel:Endpoints. " +
            "This is allowed when CMRemote runs behind a TLS-terminating reverse proxy.");
    }

    private PreflightCheckResult CheckBindPorts()
    {
        // The wizard runs *inside* the running web server, so any
        // configured URL has already bound successfully. We surface
        // the bound URLs as the check's detail so the operator can
        // confirm the wizard is reachable on the address they expect.
        var urls = _configuration["ASPNETCORE_URLS"]
                   ?? _configuration["urls"]
                   ?? string.Empty;
        if (string.IsNullOrWhiteSpace(urls))
        {
            return new PreflightCheckResult(
                "Bind ports reachable",
                PreflightStatus.Warning,
                "Could not read ASPNETCORE_URLS; the wizard cannot confirm the bound address.");
        }

        return new PreflightCheckResult(
            "Bind ports reachable",
            PreflightStatus.Passed,
            $"Server is bound on: {urls}");
    }

    /// <summary>
    /// Diagnostic; not part of the public service surface, but useful
    /// for the wizard to pin "what platform am I on?" copy.
    /// </summary>
    internal static string PlatformDescription() =>
        RuntimeInformation.OSDescription;
}
