using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;

namespace Remotely.Migration.Legacy;

/// <summary>
/// Default <see cref="IMigrationRunner"/>. Wires the supplied schema
/// inspector to the supplied converters and produces a
/// <see cref="MigrationReport"/>.
///
/// **Scope.** As of the M2 inspector slice, the runner does open
/// the source connection through the supplied
/// <see cref="ILegacySchemaInspector"/> (the default
/// <see cref="LegacySchemaInspector"/> probes for the canonical
/// upstream table set across SQLite / SQL Server / PostgreSQL).
/// What still does **not** ship in this slice are the per-entity
/// row readers and the target writer — so a "known schema" run
/// produces one zero-row <see cref="EntityReport"/> entry per
/// applicable converter today, and the actual row-level migration
/// lands in the next M2 slice. The public surface of this
/// orchestrator is fixed so the wizard's import step (M1.3) can
/// already bind against it.
/// </summary>
public class MigrationRunner : IMigrationRunner
{
    private readonly ILegacySchemaInspector _inspector;
    private readonly IReadOnlyList<object> _converters;
    private readonly ILogger<MigrationRunner> _logger;

    /// <param name="inspector">Schema inspector for the source connection.</param>
    /// <param name="converters">
    /// Heterogeneous set of <see cref="IRowConverter{TLegacy, TV2}"/>
    /// instances. Typed as <see cref="object"/> because each instance
    /// has a different generic signature; the runner picks the ones
    /// whose <c>HandlesSchemaVersion</c> matches via reflection on the
    /// known interface marker.
    /// </param>
    public MigrationRunner(
        ILegacySchemaInspector inspector,
        IEnumerable<object> converters,
        ILogger<MigrationRunner>? logger = null)
    {
        _inspector = inspector ?? throw new ArgumentNullException(nameof(inspector));
        _converters = (converters ?? throw new ArgumentNullException(nameof(converters)))
            .ToList();
        _logger = logger ?? NullLogger<MigrationRunner>.Instance;
    }

    /// <inheritdoc />
    public async Task<MigrationReport> RunAsync(
        MigrationOptions options,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(options);

        var report = new MigrationReport
        {
            StartedAtUtc = DateTimeOffset.UtcNow,
            DryRun = options.DryRun,
        };

        try
        {
            var version = await _inspector
                .DetectAsync(options.SourceConnectionString, cancellationToken)
                .ConfigureAwait(false);

            report.DetectedSchemaVersion = version;
            _logger.LogInformation(
                "Migration runner detected source schema version {Version}.", version);

            switch (version)
            {
                case LegacySchemaVersion.Unknown:
                    report.FatalErrors.Add(
                        "Source schema did not match any known upstream layout. " +
                        "Refusing to import — converter pass-through against an " +
                        "unknown schema risks silent data loss.");
                    break;

                case LegacySchemaVersion.Empty:
                    _logger.LogInformation(
                        "Source database is empty; nothing to migrate.");
                    break;

                default:
                    EnumerateApplicableConverters(version, report);
                    break;
            }
        }
        catch (OperationCanceledException)
        {
            // Bubble cancellation up so the wizard / CLI can surface
            // it as an explicit user abort rather than a fatal error.
            throw;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Migration runner failed.");
            report.FatalErrors.Add($"Migration runner failed: {ex.Message}");
        }
        finally
        {
            report.CompletedAtUtc = DateTimeOffset.UtcNow;
        }

        return report;
    }

    /// <summary>
    /// Records one (currently empty) <see cref="EntityReport"/> per
    /// converter that handles the detected schema version. The
    /// next-slice reader fills in the row counts; for the scaffold
    /// the entries appear with zero rows so the wizard's progress
    /// page can render the eventual layout against today's build.
    /// </summary>
    private void EnumerateApplicableConverters(
        LegacySchemaVersion version,
        MigrationReport report)
    {
        foreach (var converter in _converters)
        {
            var converterType = converter.GetType();
            var iface = converterType.GetInterfaces()
                .FirstOrDefault(i => i.IsGenericType
                    && i.GetGenericTypeDefinition() == typeof(IRowConverter<,>));
            if (iface is null)
            {
                continue;
            }

            var entityNameProp = iface.GetProperty(nameof(IRowConverter<object, object>.EntityName));
            var versionProp = iface.GetProperty(
                nameof(IRowConverter<object, object>.HandlesSchemaVersion));

            var handles = (LegacySchemaVersion?)versionProp?.GetValue(converter);
            if (handles != version)
            {
                continue;
            }

            var entityName = (string?)entityNameProp?.GetValue(converter)
                ?? converterType.Name;

            report.Entities.Add(new EntityReport { EntityName = entityName });
        }
    }
}
