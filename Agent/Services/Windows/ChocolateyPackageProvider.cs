using Microsoft.Extensions.Logging;
using Remotely.Agent.Interfaces;
using Remotely.Shared.Dtos;
using Remotely.Shared.Enums;
using Remotely.Shared.PackageManager;
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.Versioning;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Agent.Services.Windows;

/// <summary>
/// Chocolatey-backed implementation of <see cref="IPackageProvider"/>.
/// Resolves the operator-supplied <c>PackageIdentifier</c> to a
/// <c>choco install --yes --no-progress --limit-output --no-color</c>
/// invocation (or the matching uninstall). The wire never carries an
/// executable string — the agent picks <c>choco.exe</c> from PATH and
/// passes only the package id and operator-vetted flags.
///
/// <para>We deliberately do NOT pass extra args through a shell.
/// Operator install arguments are split on whitespace into discrete
/// argv slots, which combined with the server-side reject-list for
/// shell metacharacters keeps "evil" arguments from escaping.</para>
/// </summary>
[SupportedOSPlatform("windows")]
public sealed class ChocolateyPackageProvider : IPackageProvider
{
    // 30 minutes is generous — large packages (Office, VS Build Tools)
    // can comfortably exceed 10 minutes on a fresh install but should
    // never legitimately exceed 30. Hitting the cap kills the process
    // and reports a timeout.
    private static readonly TimeSpan ExecutionTimeout = TimeSpan.FromMinutes(30);

    private const int MaxOutputCharacters = 16 * 1024;

    private readonly ILogger<ChocolateyPackageProvider> _logger;

    public ChocolateyPackageProvider(ILogger<ChocolateyPackageProvider> logger)
    {
        _logger = logger;
    }

    public bool CanHandle(PackageInstallRequestDto request)
    {
        if (request is null || request.Provider != PackageProvider.Chocolatey)
        {
            return false;
        }
        return TryResolveChocoPath(out _);
    }

    public async Task<PackageInstallResultDto> ExecuteAsync(PackageInstallRequestDto request, CancellationToken cancellationToken)
    {
        var stopwatch = Stopwatch.StartNew();
        var result = new PackageInstallResultDto
        {
            JobId = request?.JobId ?? string.Empty,
            ExitCode = -1,
            Success = false,
        };

        try
        {
            if (request is null)
            {
                result.ErrorMessage = "Request is required.";
                return result;
            }
            if (request.Provider != PackageProvider.Chocolatey)
            {
                result.ErrorMessage = "Provider mismatch.";
                return result;
            }
            if (string.IsNullOrWhiteSpace(request.PackageIdentifier))
            {
                result.ErrorMessage = "Package identifier is required.";
                return result;
            }
            if (!IsSafePackageId(request.PackageIdentifier))
            {
                result.ErrorMessage = "Package identifier contains disallowed characters.";
                return result;
            }
            if (!TryResolveChocoPath(out var chocoPath))
            {
                result.ErrorMessage = "Chocolatey (choco.exe) is not installed on this device.";
                return result;
            }

            var args = new System.Collections.Generic.List<string>(16)
            {
                request.Action == PackageInstallAction.Uninstall ? "uninstall" : "install",
                request.PackageIdentifier,
                "--yes",
                "--no-progress",
                "--limit-output",
                "--no-color",
            };

            if (!string.IsNullOrWhiteSpace(request.Version) && IsSafeVersion(request.Version!))
            {
                args.Add("--version");
                args.Add(request.Version!);
            }

            // Operator-supplied flags are pre-validated server-side for
            // shell metacharacters. We additionally split on whitespace
            // so they land in discrete argv slots — eliminates the
            // "single string passed to a shell" attack surface.
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
                "Chocolatey {action} starting. JobId={jobId} PackageId={packageId} Version={version}",
                request.Action, request.JobId, request.PackageIdentifier, request.Version);

            var (exitCode, stdout, stderr, error) = await RunProcessAsync(
                chocoPath, args, cancellationToken).ConfigureAwait(false);

            stopwatch.Stop();

            result.ExitCode = exitCode;
            result.DurationMs = stopwatch.ElapsedMilliseconds;
            result.StdoutTail = stdout;
            result.StderrTail = stderr;
            result.ErrorMessage = error;
            result.Success = error is null && ChocolateyOutputParser.IsSuccessExitCode(exitCode);

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
            _logger.LogError(ex, "Chocolatey {action} failed unexpectedly. JobId={jobId}",
                request?.Action, request?.JobId);
            result.DurationMs = stopwatch.ElapsedMilliseconds;
            result.ErrorMessage = ex.Message;
            return result;
        }
    }

    // --- helpers --------------------------------------------------------

    internal static bool IsSafePackageId(string id)
    {
        if (string.IsNullOrWhiteSpace(id) || id.Length > 100)
        {
            return false;
        }
        for (var i = 0; i < id.Length; i++)
        {
            var c = id[i];
            var ok = (c >= 'a' && c <= 'z') ||
                     (c >= 'A' && c <= 'Z') ||
                     (c >= '0' && c <= '9') ||
                     c == '.' || c == '-' || c == '_';
            if (!ok)
            {
                return false;
            }
        }
        return true;
    }

    internal static bool IsSafeVersion(string version)
    {
        if (string.IsNullOrEmpty(version) || version.Length > 64)
        {
            return false;
        }
        for (var i = 0; i < version.Length; i++)
        {
            var c = version[i];
            var ok = (c >= '0' && c <= '9') ||
                     (c >= 'a' && c <= 'z') ||
                     (c >= 'A' && c <= 'Z') ||
                     c == '.' || c == '-' || c == '+';
            if (!ok)
            {
                return false;
            }
        }
        return true;
    }

    internal static bool TryResolveChocoPath(out string path)
    {
        // Chocolatey's installer drops choco.exe under %ChocolateyInstall%
        // and adds it to PATH; honor both. We avoid using `where` so the
        // probe doesn't itself spawn a shell.
        var chocolateyInstall = Environment.GetEnvironmentVariable("ChocolateyInstall");
        if (!string.IsNullOrWhiteSpace(chocolateyInstall))
        {
            var candidate = Path.Combine(chocolateyInstall!, "bin", "choco.exe");
            if (File.Exists(candidate))
            {
                path = candidate;
                return true;
            }
        }

        var pathVar = Environment.GetEnvironmentVariable("PATH");
        if (!string.IsNullOrEmpty(pathVar))
        {
            foreach (var dir in pathVar.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
            {
                try
                {
                    var candidate = Path.Combine(dir.Trim(), "choco.exe");
                    if (File.Exists(candidate))
                    {
                        path = candidate;
                        return true;
                    }
                }
                catch
                {
                    // Tolerate malformed PATH entries.
                }
            }
        }

        path = string.Empty;
        return false;
    }

    private async Task<(int ExitCode, string? Stdout, string? Stderr, string? Error)> RunProcessAsync(
        string fileName,
        System.Collections.Generic.IReadOnlyList<string> arguments,
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
                return (-1, null, null, "Process failed to start.");
            }
        }
        catch (Exception ex)
        {
            return (-1, null, null, ex.Message);
        }

        process.BeginOutputReadLine();
        process.BeginErrorReadLine();

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(ExecutionTimeout);

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
                _logger.LogWarning(ex, "Failed to terminate choco.exe after timeout.");
            }
            return (-1, Truncate(stdoutBuilder.ToString()), Truncate(stderrBuilder.ToString()),
                cancellationToken.IsCancellationRequested ? "Cancelled." : "Timed out.");
        }

        return (process.ExitCode,
            Truncate(stdoutBuilder.ToString()),
            Truncate(stderrBuilder.ToString()),
            null);
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
