using System.Collections.Generic;
using System.IO;

namespace Remotely.Shared.PackageManager;

/// <summary>
/// Pure helpers that interpret the textual output and exit codes of
/// the <c>choco</c> CLI. Lives in <c>Shared</c> so the Server test
/// project can exercise it directly without spinning up an agent.
///
/// <para>Parsing rules are deliberately tolerant — Chocolatey's output
/// has shifted across versions (the v1 trailing summary line vs. the
/// v2 plain-list format with <c>--limit-output</c> / <c>--no-color</c>).
/// Both shapes are accepted.</para>
/// </summary>
public static class ChocolateyOutputParser
{
    /// <summary>
    /// Exit codes Chocolatey treats as "the package operation succeeded
    /// even though Windows reported a non-zero code". 1641 (reboot
    /// initiated) and 3010 (reboot required) come from <c>msiexec</c>
    /// and are explicitly success-meaning-please-reboot.
    /// </summary>
    public static readonly IReadOnlyCollection<int> SuccessfulExitCodes =
        new HashSet<int> { 0, 1605, 1614, 1641, 3010 };

    public static bool IsSuccessExitCode(int exitCode) => SuccessfulExitCodes.Contains(exitCode);

    /// <summary>
    /// Parses the output of <c>choco list --limit-output --no-color</c>.
    /// Returns one entry per <c>id|version</c> line. Tolerates blank
    /// lines and the legacy "N packages installed." summary footer that
    /// older Chocolatey versions emit even with <c>--limit-output</c>.
    /// </summary>
    public static IReadOnlyList<ChocolateyPackage> ParseListOutput(string? output)
    {
        var results = new List<ChocolateyPackage>();
        if (string.IsNullOrWhiteSpace(output))
        {
            return results;
        }

        using var reader = new StringReader(output);
        string? line;
        while ((line = reader.ReadLine()) is not null)
        {
            var trimmed = line.Trim();
            if (trimmed.Length == 0)
            {
                continue;
            }

            // Skip the v1 banner and the trailing summary lines so a
            // caller can pipe raw output without pre-stripping.
            if (trimmed.StartsWith("Chocolatey ", System.StringComparison.OrdinalIgnoreCase) ||
                trimmed.EndsWith(" packages installed.", System.StringComparison.OrdinalIgnoreCase) ||
                trimmed.EndsWith(" packages found.", System.StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            var pipe = trimmed.IndexOf('|');
            if (pipe <= 0 || pipe == trimmed.Length - 1)
            {
                // Some non-limit-output rows may be human-readable
                // ("googlechrome 120.0.6099.130") — accept those too.
                var space = trimmed.IndexOf(' ');
                if (space > 0)
                {
                    var id = trimmed.Substring(0, space).Trim();
                    var version = trimmed.Substring(space + 1).Trim();
                    if (IsLikelyVersion(version))
                    {
                        results.Add(new ChocolateyPackage(id, version));
                    }
                }
                continue;
            }

            var packageId = trimmed.Substring(0, pipe).Trim();
            var packageVersion = trimmed.Substring(pipe + 1).Trim();
            if (packageId.Length > 0 && packageVersion.Length > 0)
            {
                results.Add(new ChocolateyPackage(packageId, packageVersion));
            }
        }

        return results;
    }

    private static bool IsLikelyVersion(string s)
    {
        if (string.IsNullOrEmpty(s))
        {
            return false;
        }
        // Accept any string starting with a digit — covers SemVer,
        // four-part Windows versions, and the occasional date-version.
        return char.IsDigit(s[0]);
    }
}

public readonly record struct ChocolateyPackage(string Id, string Version);
