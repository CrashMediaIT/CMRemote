using System.Globalization;
using Microsoft.Extensions.Logging;
using Remotely.Migration.Legacy;
using Remotely.Migration.Legacy.Converters;
using Remotely.Migration.Legacy.Readers;
using Remotely.Migration.Legacy.Writers;

namespace Remotely.Migration.Cli;

/// <summary>
/// Headless CLI wrapper around <see cref="MigrationRunner"/> — the
/// "scripted import" surface for M2.
///
/// <para>
/// Usage: <c>cmremote migrate --from &lt;sourceConn&gt; --to
/// &lt;targetConn&gt; [--dry-run] [--batch-size N]</c>. Composes the
/// full M2 converter / reader / writer triple set
/// (<see cref="OrganizationRowConverter"/> +
/// <see cref="DeviceRowConverter"/> +
/// <see cref="AspNetUserRowConverter"/>, plus their matching readers
/// and Postgres writers) and lets the runner stream every entity end
/// to end.
/// </para>
///
/// <para>
/// Exit codes: <c>0</c> on a clean run (no <c>FatalErrors</c> and no
/// <c>RowsFailed</c> across any entity), <c>1</c> on per-row failures
/// (the report still ran but recorded errors), <c>2</c> on a fatal
/// error (the report bailed before/while detecting), <c>64</c> on a
/// usage error (BSD <c>EX_USAGE</c>). The wizard's import step
/// (M1.3) consumes the same <see cref="MigrationReport"/> shape, so
/// the CLI and the UI stay in sync without a parallel codepath.
/// </para>
/// </summary>
public static class Program
{
    internal const int ExitOk = 0;
    internal const int ExitRowFailures = 1;
    internal const int ExitFatal = 2;
    internal const int ExitUsage = 64;

    public static async Task<int> Main(string[] args)
    {
        if (args.Length == 0
            || args[0] is "-h" or "--help" or "help")
        {
            PrintUsage(Console.Out);
            return ExitOk;
        }

        if (!string.Equals(args[0], "migrate", StringComparison.Ordinal))
        {
            Console.Error.WriteLine($"Unknown subcommand '{args[0]}'.");
            PrintUsage(Console.Error);
            return ExitUsage;
        }

        if (!TryParseArgs(args.AsSpan(1).ToArray(), out var parsed, out var parseError))
        {
            Console.Error.WriteLine(parseError);
            PrintUsage(Console.Error);
            return ExitUsage;
        }

        using var loggerFactory = LoggerFactory.Create(builder =>
            builder.AddSimpleConsole(o =>
                {
                    o.SingleLine = true;
                    o.TimestampFormat = "HH:mm:ss ";
                })
                .SetMinimumLevel(LogLevel.Information));
        var logger = loggerFactory.CreateLogger("cmremote-migrate");

        var runner = BuildRunner(loggerFactory);

        // Honour Ctrl+C so an operator can stop a multi-hour import
        // mid-stream without a stack-trace; the runner will surface
        // the cancellation as a fatal-error entry on the report.
        using var cts = new CancellationTokenSource();
        Console.CancelKeyPress += (_, e) =>
        {
            e.Cancel = true;
            logger.LogWarning("Cancellation requested; finishing current row…");
            cts.Cancel();
        };

        var options = new MigrationOptions
        {
            SourceConnectionString = parsed.From!,
            TargetConnectionString = parsed.To!,
            DryRun = parsed.DryRun,
            BatchSize = parsed.BatchSize,
        };

        logger.LogInformation(
            "Starting migration (dry-run={DryRun}, batch={BatchSize}).",
            options.DryRun, options.BatchSize);

        var report = await runner.RunAsync(options, cts.Token).ConfigureAwait(false);

        PrintReport(report, Console.Out);

        return ComputeExitCode(report);
    }

    /// <summary>
    /// Composes the runner used by both <see cref="Main"/> and the
    /// CLI test suite, so the CLI tests exercise the same reader /
    /// converter / writer triple set the operator gets at runtime.
    /// </summary>
    internal static MigrationRunner BuildRunner(ILoggerFactory loggerFactory)
        => new(
            inspector: new LegacySchemaInspector(),
            converters: new object[]
            {
                new OrganizationRowConverter(),
                new DeviceRowConverter(),
                new AspNetUserRowConverter(),
            },
            readers: new object[]
            {
                new LegacyOrganizationReader(),
                new LegacyDeviceReader(),
                new LegacyAspNetUserReader(),
            },
            writers: new object[]
            {
                new LegacyOrganizationWriter(),
                new LegacyDeviceWriter(),
                new LegacyUserWriter(),
            },
            logger: loggerFactory.CreateLogger<MigrationRunner>());

    /// <summary>
    /// Maps a finished <see cref="MigrationReport"/> to a process
    /// exit code. Pulled out of <see cref="Main"/> so the CLI tests
    /// can pin the contract without re-spawning the process.
    /// </summary>
    internal static int ComputeExitCode(MigrationReport report)
    {
        if (report.FatalErrors.Count > 0)
        {
            return ExitFatal;
        }

        var rowFailures = 0;
        foreach (var entity in report.Entities)
        {
            rowFailures += entity.RowsFailed;
        }
        return rowFailures > 0 ? ExitRowFailures : ExitOk;
    }

    internal static bool TryParseArgs(
        string[] args,
        out ParsedArgs parsed,
        out string? error)
    {
        parsed = new ParsedArgs { BatchSize = 500 };
        error = null;

        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            switch (arg)
            {
                case "--from":
                case "-f":
                    if (i + 1 >= args.Length)
                    {
                        error = $"'{arg}' requires a value.";
                        return false;
                    }
                    parsed.From = args[++i];
                    break;
                case "--to":
                case "-t":
                    if (i + 1 >= args.Length)
                    {
                        error = $"'{arg}' requires a value.";
                        return false;
                    }
                    parsed.To = args[++i];
                    break;
                case "--dry-run":
                    parsed.DryRun = true;
                    break;
                case "--batch-size":
                    if (i + 1 >= args.Length)
                    {
                        error = "'--batch-size' requires a value.";
                        return false;
                    }
                    if (!int.TryParse(args[++i], NumberStyles.Integer,
                            CultureInfo.InvariantCulture, out var bs)
                        || bs <= 0)
                    {
                        error = "'--batch-size' must be a positive integer.";
                        return false;
                    }
                    parsed.BatchSize = bs;
                    break;
                default:
                    error = $"Unrecognised argument '{arg}'.";
                    return false;
            }
        }

        if (string.IsNullOrWhiteSpace(parsed.From))
        {
            error = "Missing required '--from <sourceConnectionString>'.";
            return false;
        }
        if (string.IsNullOrWhiteSpace(parsed.To))
        {
            error = "Missing required '--to <targetConnectionString>'.";
            return false;
        }
        return true;
    }

    internal static void PrintReport(MigrationReport report, TextWriter writer)
    {
        writer.WriteLine();
        writer.WriteLine("=== Migration Report ===");
        writer.WriteLine($"Detected schema: {report.DetectedSchemaVersion}");
        writer.WriteLine(
            FormattableString.Invariant($"Started:   {report.StartedAtUtc:O}"));
        writer.WriteLine(
            FormattableString.Invariant($"Completed: {report.CompletedAtUtc:O}"));
        writer.WriteLine();
        if (report.Entities.Count == 0)
        {
            writer.WriteLine("(no entities processed)");
        }
        else
        {
            writer.WriteLine(
                $"{"Entity",-15} {"Read",6} {"Conv",6} {"Skip",6} {"Fail",6} {"Wrote",6}  Errors");
            foreach (var entity in report.Entities)
            {
                writer.WriteLine(
                    $"{Truncate(entity.EntityName, 15),-15} " +
                    $"{entity.RowsRead,6} {entity.RowsConverted,6} " +
                    $"{entity.RowsSkipped,6} {entity.RowsFailed,6} " +
                    $"{entity.RowsWritten,6}  {entity.Errors.Count}");
            }
        }

        if (report.FatalErrors.Count > 0)
        {
            writer.WriteLine();
            writer.WriteLine("Fatal errors:");
            foreach (var fatal in report.FatalErrors)
            {
                writer.WriteLine($"  - {fatal}");
            }
        }
    }

    private static string Truncate(string s, int max)
        => s.Length <= max ? s : s.Substring(0, max);

    internal static void PrintUsage(TextWriter writer)
    {
        writer.WriteLine("cmremote-migrate — import an upstream Remotely database into the v2 schema.");
        writer.WriteLine();
        writer.WriteLine("Usage:");
        writer.WriteLine("  cmremote migrate --from <sourceConn> --to <targetConn> [--dry-run] [--batch-size N]");
        writer.WriteLine();
        writer.WriteLine("Options:");
        writer.WriteLine("  --from, -f <conn>     Source ADO.NET connection string (SQLite / SQL Server / PostgreSQL).");
        writer.WriteLine("  --to,   -t <conn>     Target Postgres connection string (the v2 server's database).");
        writer.WriteLine("  --dry-run             Read + convert + report without writing to the target.");
        writer.WriteLine("  --batch-size N        Rows per keyset page (default 500).");
        writer.WriteLine();
        writer.WriteLine("Exit codes: 0 ok, 1 row-level failures, 2 fatal error, 64 usage error.");
    }

    /// <summary>Parsed command-line arguments.</summary>
    internal sealed class ParsedArgs
    {
        public string? From { get; set; }
        public string? To { get; set; }
        public bool DryRun { get; set; }
        public int BatchSize { get; set; }
    }
}
