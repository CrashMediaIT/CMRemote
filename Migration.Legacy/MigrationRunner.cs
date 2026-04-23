using System.Reflection;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;

namespace Remotely.Migration.Legacy;

/// <summary>
/// Default <see cref="IMigrationRunner"/>. Wires the supplied schema
/// inspector + readers + converters + writers and produces a
/// <see cref="MigrationReport"/>.
///
/// **Scope.** As of the M2 target-writer slice, the runner detects
/// the source schema (via the supplied
/// <see cref="ILegacySchemaInspector"/>), pairs each applicable
/// <see cref="IRowConverter{TLegacy, TV2}"/> with the matching
/// <see cref="ILegacyRowReader{TLegacy}"/> and (optionally) the
/// matching <see cref="ILegacyRowWriter{TV2}"/> (all matched on
/// <c>EntityName</c> + <c>HandlesSchemaVersion</c>), streams the
/// source rows through the converter, persists each Ok-row through
/// the writer (when <see cref="MigrationOptions.DryRun"/> is false
/// and a writer is registered), and counts the verdicts into
/// <see cref="EntityReport"/>. Converters that have no matching
/// reader yet land an <see cref="EntityReport"/> with zero rows and
/// a warning; converters that have a reader but no writer when
/// <c>DryRun=false</c> are demoted to dry-run for the remainder of
/// that entity's stream and a single warning is recorded so the
/// wizard surfaces "this entity isn't writable yet" rather than
/// silently dropping converted rows.
/// </summary>
public class MigrationRunner : IMigrationRunner
{
    private readonly ILegacySchemaInspector _inspector;
    private readonly IReadOnlyList<object> _converters;
    private readonly IReadOnlyList<object> _readers;
    private readonly IReadOnlyList<object> _writers;
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
        : this(inspector, converters, Array.Empty<object>(), Array.Empty<object>(), logger)
    {
    }

    /// <param name="inspector">Schema inspector for the source connection.</param>
    /// <param name="converters">Converters; see other overloads.</param>
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
        : this(inspector, converters, readers, Array.Empty<object>(), logger)
    {
    }

    /// <param name="inspector">Schema inspector for the source connection.</param>
    /// <param name="converters">Converters; see other overloads.</param>
    /// <param name="readers">Readers; see other overload.</param>
    /// <param name="writers">
    /// Heterogeneous set of <see cref="ILegacyRowWriter{TV2}"/>
    /// instances. Paired with converters by matching
    /// <c>EntityName</c> + <c>HandlesSchemaVersion</c>. A converter
    /// with no matching writer when
    /// <see cref="MigrationOptions.DryRun"/> is <c>false</c> is
    /// demoted to dry-run-for-this-entity (one warning recorded; rows
    /// still read + converted but not written). A writer with no
    /// matching converter is silently ignored (writers without a
    /// converter source are vestigial).
    /// </param>
    /// <param name="logger">Optional logger.</param>
    public MigrationRunner(
        ILegacySchemaInspector inspector,
        IEnumerable<object> converters,
        IEnumerable<object> readers,
        IEnumerable<object> writers,
        ILogger<MigrationRunner>? logger = null)
    {
        _inspector = inspector ?? throw new ArgumentNullException(nameof(inspector));
        _converters = (converters ?? throw new ArgumentNullException(nameof(converters)))
            .ToList();
        _readers = (readers ?? throw new ArgumentNullException(nameof(readers)))
            .ToList();
        _writers = (writers ?? throw new ArgumentNullException(nameof(writers)))
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

            var writer = FindMatchingWriter(version, entityName, v2Type);
            if (writer is null && !options.DryRun)
            {
                // No writer yet but the operator asked for a real
                // import: demote this entity to dry-run-for-this-entity
                // (rows still read + converted but not written) and
                // record one warning rather than spamming one per row.
                _logger.LogWarning(
                    "No legacy row writer registered for entity {EntityName} " +
                    "at schema {Version}; demoting this entity to dry-run.",
                    entityName, version);
                entityReport.Errors.Add(
                    $"No legacy row writer is registered for '{entityName}' yet. " +
                    "Rows will be read and converted but not persisted to the " +
                    "target — this entity is effectively dry-run until the " +
                    "concrete writer ships.");
            }

            // Reflect into the typed StreamAsync<TLegacy,TV2> so the
            // hot loop stays generic-typed and the converter / writer
            // calls are direct interface calls rather than per-row
            // reflection.
            var streamMethod = typeof(MigrationRunner)
                .GetMethod(nameof(StreamEntityAsync),
                    BindingFlags.Instance | BindingFlags.NonPublic)!
                .MakeGenericMethod(legacyType, v2Type);

            var task = (Task)streamMethod.Invoke(this, new[]
            {
                reader, converter, writer!, entityReport,
                (object)options, (object)cancellationToken,
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
    /// Finds the first registered writer that handles the same
    /// schema version, exposes the same logical entity name, and
    /// writes the v2 CLR type the converter produces on its right
    /// generic argument.
    /// </summary>
    private object? FindMatchingWriter(
        LegacySchemaVersion version,
        string entityName,
        Type v2Type)
    {
        foreach (var writer in _writers)
        {
            var iface = writer.GetType().GetInterfaces()
                .FirstOrDefault(i => i.IsGenericType
                    && i.GetGenericTypeDefinition() == typeof(ILegacyRowWriter<>));
            if (iface is null)
            {
                continue;
            }

            if (iface.GetGenericArguments()[0] != v2Type)
            {
                continue;
            }

            var writerVersion = (LegacySchemaVersion?)iface
                .GetProperty(nameof(ILegacyRowWriter<object>.HandlesSchemaVersion))
                ?.GetValue(writer);
            if (writerVersion != version)
            {
                continue;
            }

            var writerEntity = (string?)iface
                .GetProperty(nameof(ILegacyRowWriter<object>.EntityName))
                ?.GetValue(writer);
            if (!string.Equals(writerEntity, entityName, StringComparison.Ordinal))
            {
                continue;
            }

            return writer;
        }
        return null;
    }

    /// <summary>
    /// Typed inner loop reached via reflection from
    /// <see cref="ProcessConvertersAsync"/>. Streams every row from
    /// <paramref name="reader"/>, runs the converter, and (when
    /// <paramref name="writer"/> is non-null and
    /// <see cref="MigrationOptions.DryRun"/> is <c>false</c>) writes
    /// each Ok row to the target. Counts go on
    /// <paramref name="entityReport"/>: <c>RowsRead</c> per source
    /// row, <c>RowsConverted</c> per Ok converter result,
    /// <c>RowsSkipped</c> per Skip, <c>RowsFailed</c> per converter /
    /// writer exception or Fail, and <c>RowsWritten</c> per
    /// successful writer call. Per-row writer exceptions are caught
    /// and recorded against the row — the run does not abort over a
    /// single bad row.
    /// </summary>
    private async Task StreamEntityAsync<TLegacy, TV2>(
        ILegacyRowReader<TLegacy> reader,
        IRowConverter<TLegacy, TV2> converter,
        ILegacyRowWriter<TV2>? writer,
        EntityReport entityReport,
        MigrationOptions options,
        CancellationToken cancellationToken)
    {
        var shouldWrite = writer is not null && !options.DryRun;

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

                if (shouldWrite)
                {
                    try
                    {
                        await writer!.WriteAsync(
                                result.Value!,
                                options.TargetConnectionString,
                                cancellationToken)
                            .ConfigureAwait(false);
                        entityReport.RowsWritten++;
                    }
                    catch (OperationCanceledException)
                    {
                        // Honour explicit cancellation rather than
                        // burying it under a per-row failure.
                        throw;
                    }
                    catch (Exception ex)
                    {
                        entityReport.RowsFailed++;
                        AppendCappedError(entityReport,
                            $"Writer '{writer!.EntityName}' threw: {ex.Message}");
                    }
                }
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
