using System.Reflection;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;

namespace Remotely.Migration.Legacy;

/// <summary>
/// Default <see cref="IMigrationRunner"/>. Wires the supplied schema
/// inspector + readers + converters and produces a
/// <see cref="MigrationReport"/>.
///
/// **Scope.** As of the M2 row-reader slice, the runner detects the
/// source schema (via the supplied <see cref="ILegacySchemaInspector"/>),
/// pairs each applicable <see cref="IRowConverter{TLegacy, TV2}"/>
/// with the matching <see cref="ILegacyRowReader{TLegacy}"/> (matched
/// on <c>EntityName</c> + <c>HandlesSchemaVersion</c>), streams the
/// source rows through the converter, and counts the verdicts into
/// <see cref="EntityReport"/>. The **target writer** still does not
/// ship in this slice — every run is effectively a dry-run from the
/// destination DB's point of view, so converted rows are counted but
/// not yet persisted. Converters that have no matching reader yet
/// land an <see cref="EntityReport"/> with zero rows and a warning
/// recorded against it (so the wizard surfaces "this entity isn't
/// importable yet" rather than silently dropping it).
/// </summary>
public class MigrationRunner : IMigrationRunner
{
    private readonly ILegacySchemaInspector _inspector;
    private readonly IReadOnlyList<object> _converters;
    private readonly IReadOnlyList<object> _readers;
    private readonly ILogger<MigrationRunner> _logger;

    /// <param name="inspector">Schema inspector for the source connection.</param>
    /// <param name="converters">
    /// Heterogeneous set of <see cref="IRowConverter{TLegacy, TV2}"/>
    /// instances. Typed as <see cref="object"/> because each instance
    /// has a different generic signature; the runner picks the ones
    /// whose <c>HandlesSchemaVersion</c> matches via reflection on the
    /// known interface marker.
    /// </param>
    /// <param name="logger">Optional logger.</param>
    public MigrationRunner(
        ILegacySchemaInspector inspector,
        IEnumerable<object> converters,
        ILogger<MigrationRunner>? logger = null)
        : this(inspector, converters, Array.Empty<object>(), logger)
    {
    }

    /// <param name="inspector">Schema inspector for the source connection.</param>
    /// <param name="converters">Converters; see other overload.</param>
    /// <param name="readers">
    /// Heterogeneous set of <see cref="ILegacyRowReader{TLegacy}"/>
    /// instances. Paired with converters by matching
    /// <c>EntityName</c> + <c>HandlesSchemaVersion</c>. A converter
    /// with no matching reader is reported as zero rows + a warning;
    /// a reader with no matching converter is silently ignored
    /// (readers without a converter target are vestigial and not the
    /// runner's problem to surface).
    /// </param>
    /// <param name="logger">Optional logger.</param>
    public MigrationRunner(
        ILegacySchemaInspector inspector,
        IEnumerable<object> converters,
        IEnumerable<object> readers,
        ILogger<MigrationRunner>? logger = null)
    {
        _inspector = inspector ?? throw new ArgumentNullException(nameof(inspector));
        _converters = (converters ?? throw new ArgumentNullException(nameof(converters)))
            .ToList();
        _readers = (readers ?? throw new ArgumentNullException(nameof(readers)))
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
                    await ProcessConvertersAsync(version, report, options, cancellationToken)
                        .ConfigureAwait(false);
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
    /// For each converter that handles <paramref name="version"/>:
    /// find the matching reader (by <c>EntityName</c> +
    /// <c>HandlesSchemaVersion</c>), stream every source row through
    /// the converter, and append an <see cref="EntityReport"/> with
    /// the counts. Entries are appended in the order converters were
    /// registered so the wizard renders a deterministic progress
    /// list.
    /// </summary>
    private async Task ProcessConvertersAsync(
        LegacySchemaVersion version,
        MigrationReport report,
        MigrationOptions options,
        CancellationToken cancellationToken)
    {
        foreach (var converter in _converters)
        {
            cancellationToken.ThrowIfCancellationRequested();

            var converterType = converter.GetType();
            var converterIface = converterType.GetInterfaces()
                .FirstOrDefault(i => i.IsGenericType
                    && i.GetGenericTypeDefinition() == typeof(IRowConverter<,>));
            if (converterIface is null)
            {
                continue;
            }

            var handlesVersion = (LegacySchemaVersion?)converterIface
                .GetProperty(nameof(IRowConverter<object, object>.HandlesSchemaVersion))
                ?.GetValue(converter);
            if (handlesVersion != version)
            {
                continue;
            }

            var entityName = (string?)converterIface
                .GetProperty(nameof(IRowConverter<object, object>.EntityName))
                ?.GetValue(converter)
                ?? converterType.Name;

            var entityReport = new EntityReport { EntityName = entityName };
            report.Entities.Add(entityReport);

            var legacyType = converterIface.GetGenericArguments()[0];
            var v2Type = converterIface.GetGenericArguments()[1];

            var reader = FindMatchingReader(version, entityName, legacyType);
            if (reader is null)
            {
                _logger.LogWarning(
                    "No legacy row reader registered for entity {EntityName} " +
                    "at schema {Version}; recording zero rows and continuing.",
                    entityName, version);
                entityReport.Errors.Add(
                    $"No legacy row reader is registered for '{entityName}' yet. " +
                    "The converter is wired but the source-side reader has not " +
                    "shipped — this entity will be skipped until the next M2 slice.");
                continue;
            }

            // Reflect into the typed StreamAsync<TLegacy,TV2> so the
            // hot loop stays generic-typed and the converter call is
            // a direct interface call rather than per-row reflection.
            var streamMethod = typeof(MigrationRunner)
                .GetMethod(nameof(StreamEntityAsync),
                    BindingFlags.Instance | BindingFlags.NonPublic)!
                .MakeGenericMethod(legacyType, v2Type);

            var task = (Task)streamMethod.Invoke(this, new[]
            {
                reader, converter, entityReport, (object)options, (object)cancellationToken,
            })!;
            await task.ConfigureAwait(false);
        }
    }

    /// <summary>
    /// Finds the first registered reader that handles the same
    /// schema version, exposes the same logical entity name, and
    /// reads the legacy CLR type the converter expects on its left
    /// generic argument.
    /// </summary>
    private object? FindMatchingReader(
        LegacySchemaVersion version,
        string entityName,
        Type legacyType)
    {
        foreach (var reader in _readers)
        {
            var iface = reader.GetType().GetInterfaces()
                .FirstOrDefault(i => i.IsGenericType
                    && i.GetGenericTypeDefinition() == typeof(ILegacyRowReader<>));
            if (iface is null)
            {
                continue;
            }

            if (iface.GetGenericArguments()[0] != legacyType)
            {
                continue;
            }

            var readerVersion = (LegacySchemaVersion?)iface
                .GetProperty(nameof(ILegacyRowReader<object>.HandlesSchemaVersion))
                ?.GetValue(reader);
            if (readerVersion != version)
            {
                continue;
            }

            var readerEntity = (string?)iface
                .GetProperty(nameof(ILegacyRowReader<object>.EntityName))
                ?.GetValue(reader);
            if (!string.Equals(readerEntity, entityName, StringComparison.Ordinal))
            {
                continue;
            }

            return reader;
        }
        return null;
    }

    /// <summary>
    /// Typed inner loop reached via reflection from
    /// <see cref="ProcessConvertersAsync"/>. Streams every row from
    /// <paramref name="reader"/>, runs the converter, and accumulates
    /// the verdict counts on <paramref name="entityReport"/>. The
    /// target writer is not yet wired (next M2 slice) so converted
    /// rows are counted but not persisted — i.e. every run today
    /// behaves like a dry-run from the destination DB's point of
    /// view, regardless of <see cref="MigrationOptions.DryRun"/>.
    /// </summary>
    private async Task StreamEntityAsync<TLegacy, TV2>(
        ILegacyRowReader<TLegacy> reader,
        IRowConverter<TLegacy, TV2> converter,
        EntityReport entityReport,
        MigrationOptions options,
        CancellationToken cancellationToken)
    {
        await foreach (var row in reader
            .ReadAsync(options.SourceConnectionString, options.BatchSize, cancellationToken)
            .ConfigureAwait(false))
        {
            cancellationToken.ThrowIfCancellationRequested();
            entityReport.RowsRead++;

            ConverterResult<TV2> result;
            try
            {
                result = converter.Convert(row);
            }
            catch (Exception ex)
            {
                entityReport.RowsFailed++;
                AppendCappedError(entityReport,
                    $"Converter '{converter.EntityName}' threw: {ex.Message}");
                continue;
            }

            if (result.IsSuccess)
            {
                entityReport.RowsConverted++;
            }
            else if (result.IsSkipped)
            {
                entityReport.RowsSkipped++;
            }
            else if (result.IsFailure)
            {
                entityReport.RowsFailed++;
                AppendCappedError(entityReport,
                    result.ErrorMessage ?? "Converter failed without a message.");
            }
        }
    }

    private static void AppendCappedError(EntityReport entityReport, string error)
    {
        if (entityReport.Errors.Count >= EntityReport.MaxErrorsPerEntity)
        {
            return;
        }
        entityReport.Errors.Add(error);
    }
}
