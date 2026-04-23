using Microsoft.Extensions.Logging;
using Remotely.Agent.Interfaces;
using Remotely.Shared;
using Remotely.Shared.Dtos;
using Remotely.Shared.Enums;
using Remotely.Shared.PackageManager;
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Net.Http;
using System.Runtime.Versioning;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Services.Windows;

/// <summary>
/// Implements <see cref="IPackageProvider"/> for
/// <see cref="PackageProvider.UploadedMsi"/>. Workflow:
/// <list type="number">
///   <item>Pull <see cref="ConnectionInfo.Host"/> + the
///         <c>MsiSharedFileId</c> / <c>MsiAuthToken</c> off the wire
///         (server-minted, short-lived).</item>
///   <item>Stream the bytes to a temp file under
///         <c>%ProgramData%\Remotely\PackageManager\Cache</c>.</item>
///   <item>Re-hash with SHA-256 and check the OLE2 magic bytes — refuse
///         on either mismatch (bytes are deleted before returning).</item>
///   <item>Run <c>msiexec /i &lt;file&gt; /qn /norestart /L*v &lt;log&gt;</c>
///         with operator-supplied flags appended as discrete argv
///         slots (no shell).</item>
///   <item>On failure, attach the tail of the verbose log to the
///         result so the operator can see why msiexec rejected the
///         install.</item>
/// </list>
/// The provider never accepts an executable string from the wire — the
/// <c>msiexec.exe</c> path is resolved locally from <c>%SystemRoot%</c>.
/// </summary>
[SupportedOSPlatform("windows")]
public sealed class MsiPackageInstaller : IPackageProvider
{
    private static readonly TimeSpan DownloadTimeout = TimeSpan.FromMinutes(15);
    private static readonly TimeSpan InstallTimeout = TimeSpan.FromMinutes(60);
    private const int MaxLogTailBytes = 16 * 1024;

    private readonly IConfigService _configService;
    private readonly IHttpClientFactory _httpFactory;
    private readonly ILogger<MsiPackageInstaller> _logger;

    public MsiPackageInstaller(
        IConfigService configService,
        IHttpClientFactory httpFactory,
        ILogger<MsiPackageInstaller> logger)
    {
        _configService = configService;
        _httpFactory = httpFactory;
        _logger = logger;
    }

    public bool CanHandle(PackageInstallRequestDto request)
    {
        if (request is null || request.Provider != PackageProvider.UploadedMsi)
        {
            return false;
        }
        // Need msiexec.exe (always present on Windows) and the wire
        // payload must include the server-minted download metadata.
        return TryResolveMsiExec(out _) &&
               !string.IsNullOrWhiteSpace(request.MsiSharedFileId) &&
               !string.IsNullOrWhiteSpace(request.MsiAuthToken) &&
               !string.IsNullOrWhiteSpace(request.MsiSha256);
    }

    public async Task<PackageInstallResultDto> ExecuteAsync(
        PackageInstallRequestDto request,
        CancellationToken cancellationToken)
    {
        var stopwatch = Stopwatch.StartNew();
        var result = new PackageInstallResultDto
        {
            JobId = request?.JobId ?? string.Empty,
            ExitCode = -1,
            Success = false,
        };

        if (request is null)
        {
            result.ErrorMessage = "Request is required.";
            return result;
        }
        if (request.Provider != PackageProvider.UploadedMsi)
        {
            result.ErrorMessage = "Provider mismatch.";
            return result;
        }
        if (string.IsNullOrWhiteSpace(request.MsiSharedFileId) ||
            string.IsNullOrWhiteSpace(request.MsiAuthToken) ||
            string.IsNullOrWhiteSpace(request.MsiSha256))
        {
            result.ErrorMessage = "MSI download metadata is missing.";
            return result;
        }
        if (!TryResolveMsiExec(out var msiExec))
        {
            result.ErrorMessage = "msiexec.exe could not be located.";
            return result;
        }

        var cacheDir = EnsureCacheDir();
        var safeName = MsiFileValidator.SanitiseFileName(request.MsiFileName);
        var localFile = Path.Combine(cacheDir, $"{Guid.NewGuid():N}_{safeName}");
        var logFile = Path.Combine(cacheDir, $"{Guid.NewGuid():N}.msi.log");

        try
        {
            // Step 1: download.
            try
            {
                await DownloadAsync(request, localFile, cancellationToken).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "MSI download failed. JobId={jobId}", request.JobId);
                result.ErrorMessage = $"Failed to download MSI: {ex.Message}";
                return result;
            }

            // Step 2: validate magic bytes + SHA-256.
            if (!MsiFileValidator.HasOle2Magic(localFile))
            {
                result.ErrorMessage = "Downloaded file is not a valid MSI (magic-byte check failed).";
                return result;
            }

            string actualSha;
            try
            {
                using var fs = new FileStream(localFile, FileMode.Open, FileAccess.Read, FileShare.Read);
                actualSha = MsiFileValidator.ComputeSha256Hex(fs);
            }
            catch (Exception ex)
            {
                result.ErrorMessage = $"Failed to hash downloaded MSI: {ex.Message}";
                return result;
            }
            if (!string.Equals(actualSha, request.MsiSha256, StringComparison.OrdinalIgnoreCase))
            {
                _logger.LogWarning(
                    "SHA-256 mismatch on downloaded MSI. JobId={jobId} Expected={expected} Actual={actual}",
                    request.JobId, request.MsiSha256, actualSha);
                result.ErrorMessage = "SHA-256 mismatch — refusing to install.";
                return result;
            }

            // Step 3: build argv. msiexec is unusual in that it always
            // wants /i <file> in that order. We deliberately pass each
            // token as a discrete argv slot (no shell) so operator
            // arguments can't escape.
            var args = new List<string>(16)
            {
                request.Action == PackageInstallAction.Uninstall ? "/x" : "/i",
                localFile,
                "/qn",
                "/norestart",
                "/L*v",
                logFile,
            };
            if (!string.IsNullOrWhiteSpace(request.InstallArguments))
            {
                foreach (var part in request.InstallArguments!.Split(
                             new[] { ' ', '\t' },
                             StringSplitOptions.RemoveEmptyEntries))
                {
                    args.Add(part);
                }
            }

            _logger.LogInformation(
                "msiexec {action} starting. JobId={jobId} File={file}",
                request.Action, request.JobId, localFile);

            var (exitCode, error) = await RunProcessAsync(
                msiExec, args, InstallTimeout, cancellationToken).ConfigureAwait(false);

            stopwatch.Stop();
            result.ExitCode = exitCode;
            result.DurationMs = stopwatch.ElapsedMilliseconds;
            result.ErrorMessage = error;
            // 0 = success, 3010 = success but reboot required, 1641 = success and reboot was initiated.
            // These match Microsoft's documented "successful" exit codes for msiexec.
            result.Success = error is null && (exitCode == 0 || exitCode == 3010 || exitCode == 1641);

            // Always attach the tail of the verbose log so the operator
            // sees the same diagnostic msiexec wrote — but only on
            // failure to keep the success path's payload small.
            if (!result.Success)
            {
                result.StdoutTail = TryReadLogTail(logFile);
            }

            return result;
        }
        catch (OperationCanceledException)
        {
            stopwatch.Stop();
            result.DurationMs = stopwatch.ElapsedMilliseconds;
            result.ErrorMessage = "Cancelled.";
            return result;
        }
        catch (Exception ex)
        {
            stopwatch.Stop();
            _logger.LogError(ex, "MSI install failed unexpectedly. JobId={jobId}", request.JobId);
            result.DurationMs = stopwatch.ElapsedMilliseconds;
            result.ErrorMessage = ex.Message;
            return result;
        }
        finally
        {
            // Best-effort cleanup. msiexec writes the log even on
            // success, so tear it down here so we don't leak GiB of
            // verbose logs over time.
            TryDelete(localFile);
            TryDelete(logFile);
        }
    }

    // --- helpers --------------------------------------------------------

    private async Task DownloadAsync(
        PackageInstallRequestDto request,
        string destinationPath,
        CancellationToken cancellationToken)
    {
        var host = _configService.GetConnectionInfo()?.Host?.TrimEnd('/');
        if (string.IsNullOrEmpty(host))
        {
            throw new InvalidOperationException("Server host is not configured.");
        }
        var url = $"{host}/API/FileSharing/{request.MsiSharedFileId}";

        using var http = _httpFactory.CreateClient();
        http.Timeout = DownloadTimeout;
        http.DefaultRequestHeaders.Add(AppConstants.ExpiringTokenHeaderName, request.MsiAuthToken);

        using var response = await http.GetAsync(
            url, HttpCompletionOption.ResponseHeadersRead, cancellationToken).ConfigureAwait(false);
        response.EnsureSuccessStatusCode();

        await using var src = await response.Content.ReadAsStreamAsync(cancellationToken).ConfigureAwait(false);
        await using var fs = new FileStream(
            destinationPath, FileMode.Create, FileAccess.Write, FileShare.None, 81920, useAsync: true);
        await src.CopyToAsync(fs, cancellationToken).ConfigureAwait(false);
    }

    internal static bool TryResolveMsiExec(out string path)
    {
        var systemRoot = Environment.GetEnvironmentVariable("SystemRoot");
        if (!string.IsNullOrEmpty(systemRoot))
        {
            var candidate = Path.Combine(systemRoot, "System32", "msiexec.exe");
            if (File.Exists(candidate))
            {
                path = candidate;
                return true;
            }
        }
        path = string.Empty;
        return false;
    }

    private static string EnsureCacheDir()
    {
        var programData = Environment.GetEnvironmentVariable("ProgramData") ?? Path.GetTempPath();
        var dir = Path.Combine(programData, "Remotely", "PackageManager", "Cache");
        Directory.CreateDirectory(dir);
        return dir;
    }

    private static string? TryReadLogTail(string path)
    {
        try
        {
            if (!File.Exists(path))
            {
                return null;
            }
            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
            var length = fs.Length;
            if (length <= MaxLogTailBytes)
            {
                using var sr = new StreamReader(fs, Encoding.UTF8, detectEncodingFromByteOrderMarks: true);
                return sr.ReadToEnd();
            }
            fs.Seek(length - MaxLogTailBytes, SeekOrigin.Begin);
            using var srTail = new StreamReader(fs, Encoding.UTF8, detectEncodingFromByteOrderMarks: true);
            return srTail.ReadToEnd();
        }
        catch
        {
            return null;
        }
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch
        {
            // Best-effort.
        }
    }

    private async Task<(int ExitCode, string? Error)> RunProcessAsync(
        string fileName,
        IReadOnlyList<string> arguments,
        TimeSpan timeout,
        CancellationToken cancellationToken)
    {
        var psi = new ProcessStartInfo
        {
            FileName = fileName,
            UseShellExecute = false,
            CreateNoWindow = true,
            // msiexec writes its own log via /L*v; we don't redirect
            // stdout/stderr because msiexec doesn't write meaningful
            // diagnostic data to either in /qn mode.
            RedirectStandardOutput = false,
            RedirectStandardError = false,
        };
        foreach (var arg in arguments)
        {
            psi.ArgumentList.Add(arg);
        }

        using var process = new Process { StartInfo = psi };
        try
        {
            if (!process.Start())
            {
                return (-1, "msiexec failed to start.");
            }
        }
        catch (Exception ex)
        {
            return (-1, ex.Message);
        }

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(timeout);

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
                _logger.LogWarning(ex, "Failed to terminate msiexec after timeout.");
            }
            return (-1, cancellationToken.IsCancellationRequested ? "Cancelled." : "Timed out.");
        }

        return (process.ExitCode, null);
    }
}
