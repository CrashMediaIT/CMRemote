using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;

namespace Remotely.Server.Services;

/// <inheritdoc cref="ISetupStateService"/>
public class SetupStateService : ISetupStateService
{
    /// <summary>
    /// Fixed Guid that identifies the <c>CMRemote.Setup.Completed</c>
    /// marker row in <c>KeyValueRecords</c>. The table's key column is a
    /// Guid (matching the convention already used by
    /// <c>SettingsModel.DbKey</c>), so the marker shares storage with the
    /// existing application-settings record without needing a new table /
    /// migration.
    /// </summary>
    public static Guid SetupCompletedKey { get; } =
        Guid.Parse("c074e0d3-7a0e-4b4f-9b0e-5e10b513d001");

    private readonly IAppDbFactory _dbFactory;
    private readonly ILogger<SetupStateService> _logger;

    public SetupStateService(IAppDbFactory dbFactory, ILogger<SetupStateService> logger)
    {
        _dbFactory = dbFactory;
        _logger = logger;
    }

    /// <inheritdoc />
    public async Task<bool> IsSetupCompletedAsync(CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var record = await db.KeyValueRecords
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.Key == SetupCompletedKey, cancellationToken);
        return record is not null && !string.IsNullOrWhiteSpace(record.Value);
    }

    /// <inheritdoc />
    public async Task MarkSetupCompletedAsync(CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var record = await db.KeyValueRecords
            .FirstOrDefaultAsync(x => x.Key == SetupCompletedKey, cancellationToken);

        if (record is { } existing && !string.IsNullOrWhiteSpace(existing.Value))
        {
            // Idempotent: do not overwrite the original completion stamp.
            return;
        }

        var marker = JsonSerializer.Serialize(new SetupCompletedMarker
        {
            CompletedAtUtc = DateTimeOffset.UtcNow,
            SchemaVersion = 1
        });

        if (record is null)
        {
            await db.KeyValueRecords.AddAsync(
                new KeyValueRecord { Key = SetupCompletedKey, Value = marker },
                cancellationToken);
        }
        else
        {
            record.Value = marker;
        }

        await db.SaveChangesAsync(cancellationToken);
        _logger.LogInformation("CMRemote.Setup.Completed marker written.");
    }

    /// <inheritdoc />
    public async Task EnsureMarkerForExistingDeploymentAsync(CancellationToken cancellationToken = default)
    {
        if (await IsSetupCompletedAsync(cancellationToken))
        {
            return;
        }

        using var db = _dbFactory.GetContext();

        // Heuristic: any operator-visible state means this is an existing
        // deployment that pre-dates the wizard. Mark it complete so the
        // redirect middleware never hijacks the operator into /setup.
        var hasExistingData =
            await db.Organizations.AsNoTracking().AnyAsync(cancellationToken) ||
            await db.Users.AsNoTracking().AnyAsync(cancellationToken) ||
            await db.Devices.AsNoTracking().AnyAsync(cancellationToken);

        if (hasExistingData)
        {
            _logger.LogInformation(
                "Existing deployment detected (orgs/users/devices present). " +
                "Auto-writing CMRemote.Setup.Completed marker.");
            await MarkSetupCompletedAsync(cancellationToken);
        }
    }

    private sealed class SetupCompletedMarker
    {
        public DateTimeOffset CompletedAtUtc { get; set; }
        public int SchemaVersion { get; set; }
    }
}
