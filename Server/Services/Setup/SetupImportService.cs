extern alias MigrationLegacy;

using Microsoft.Extensions.Logging;
using MigrationLegacy::Remotely.Migration.Legacy;
using MigrationLegacy::Remotely.Migration.Legacy.Converters;
using MigrationLegacy::Remotely.Migration.Legacy.Readers;
using MigrationLegacy::Remotely.Migration.Legacy.Writers;

namespace Remotely.Server.Services.Setup;

/// <inheritdoc cref="ISetupImportService" />
public class SetupImportService : ISetupImportService
{
    private readonly IConnectionStringWriter _connectionStringWriter;
    private readonly ILoggerFactory _loggerFactory;
    private readonly ILogger<SetupImportService> _logger;
    private readonly Func<MigrationRunner> _runnerFactory;

    public SetupImportService(
        IConnectionStringWriter connectionStringWriter,
        ILoggerFactory loggerFactory,
        ILogger<SetupImportService> logger)
        : this(
            connectionStringWriter,
            loggerFactory,
            logger,
            runnerFactory: null)
    {
    }

    /// <summary>
    /// Test-friendly overload that accepts an explicit runner factory.
    /// Used by <c>SetupImportServiceTests</c> to inject a runner
    /// composed against an in-memory SQLite source — exactly mirroring
    /// the smoke-test pattern in the CLI's own test suite.
    /// </summary>
    internal SetupImportService(
        IConnectionStringWriter connectionStringWriter,
        ILoggerFactory loggerFactory,
        ILogger<SetupImportService> logger,
        Func<MigrationRunner>? runnerFactory)
    {
        _connectionStringWriter = connectionStringWriter;
        _loggerFactory = loggerFactory;
        _logger = logger;
        _runnerFactory = runnerFactory ?? BuildDefaultRunner;
    }

    /// <inheritdoc />
    public async Task<WizardImportReport> DetectSourceAsync(
        string sourceConnectionString,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(sourceConnectionString))
        {
            return WizardImportReport.From(new MigrationReport
            {
                StartedAtUtc = DateTimeOffset.UtcNow,
                CompletedAtUtc = DateTimeOffset.UtcNow,
                DryRun = true,
                DetectedSchemaVersion = LegacySchemaVersion.Unknown,
                FatalErrors =
                {
                    "Source connection string is empty.",
                },
            });
        }

        // Detect == a dry-run with the target wired up to a never-used
        // string. Re-use the runner's existing inspector path so we
        // don't duplicate provider-detection logic; pass a placeholder
        // target that the writers won't see (DryRun=true short-circuits
        // before any write).
        var options = new MigrationOptions
        {
            SourceConnectionString = sourceConnectionString,
            // Detection never opens the target; pass a syntactically
            // valid placeholder so the runner does not reject the
            // options up-front.
            TargetConnectionString = "Host=detect.invalid;Database=cmremote;Username=detect;",
            DryRun = true,
        };

        var runner = _runnerFactory();
        var report = await runner.RunAsync(options, cancellationToken).ConfigureAwait(false);
        return WizardImportReport.From(report);
    }

    /// <inheritdoc />
    public async Task<WizardImportReport> RunImportAsync(
        string sourceConnectionString,
        string targetConnectionString,
        bool dryRun,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(sourceConnectionString))
        {
            throw new ArgumentException(
                "Source connection string must not be empty.",
                nameof(sourceConnectionString));
        }
        if (string.IsNullOrWhiteSpace(targetConnectionString))
        {
            throw new ArgumentException(
                "Target connection string must not be empty.",
                nameof(targetConnectionString));
        }

        var options = new MigrationOptions
        {
            SourceConnectionString = sourceConnectionString,
            TargetConnectionString = targetConnectionString,
            DryRun = dryRun,
        };

        _logger.LogInformation(
            "Wizard import starting (dry-run={DryRun}).", dryRun);

        var runner = _runnerFactory();
        var report = await runner.RunAsync(options, cancellationToken).ConfigureAwait(false);

        await PersistReportAsync(report, cancellationToken).ConfigureAwait(false);
        return WizardImportReport.From(report);
    }

    /// <summary>
    /// Writes the report to disk next to the wizard's settings file
    /// as <c>migration-report.json</c>. Best-effort: a write failure
    /// is logged but does not bubble — the in-memory report is the
    /// authoritative artefact and the operator already sees it on
    /// the page.
    /// </summary>
    private async Task PersistReportAsync(
        MigrationReport report,
        CancellationToken cancellationToken)
    {
        try
        {
            var directory = Path.GetDirectoryName(_connectionStringWriter.TargetSettingsPath);
            if (string.IsNullOrEmpty(directory))
            {
                return;
            }
            Directory.CreateDirectory(directory);
            var path = Path.Combine(directory, "migration-report.json");
            await File.WriteAllTextAsync(
                    path,
                    report.ToJson() + Environment.NewLine,
                    cancellationToken)
                .ConfigureAwait(false);
            _logger.LogInformation("Wizard import report written to {Path}.", path);
        }
        catch (Exception ex) when (
            ex is IOException
            or UnauthorizedAccessException
            or NotSupportedException)
        {
            _logger.LogWarning(ex,
                "Failed to persist migration report to disk; the in-memory report is still authoritative.");
        }
    }

    /// <summary>
    /// Composes the same runner used by the CLI (see
    /// <c>Migration.Cli/Program.cs</c> &gt; <c>BuildRunner</c>). Kept
    /// as a private factory rather than a single shared instance so
    /// each wizard run gets a fresh runner — matching the CLI's
    /// per-process model and avoiding concurrency surprises if the
    /// operator backs out and re-runs the import.
    /// </summary>
    private MigrationRunner BuildDefaultRunner()
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
            logger: _loggerFactory.CreateLogger<MigrationRunner>());
}
