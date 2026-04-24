using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;

namespace Remotely.Server.Services.Setup;

/// <inheritdoc cref="ISetupWizardProgressService" />
public class SetupWizardProgressService : ISetupWizardProgressService
{
    /// <summary>
    /// Fixed Guid that identifies the wizard-progress row in
    /// <c>KeyValueRecords</c>. Sister key to
    /// <see cref="SetupStateService.SetupCompletedKey"/>; deliberately
    /// chosen as a different fixed Guid so the two records do not
    /// collide.
    /// </summary>
    public static Guid WizardProgressKey { get; } =
        Guid.Parse("c074e0d3-7a0e-4b4f-9b0e-5e10b513d002");

    private readonly IAppDbFactory _dbFactory;
    private readonly ILogger<SetupWizardProgressService> _logger;

    public SetupWizardProgressService(
        IAppDbFactory dbFactory,
        ILogger<SetupWizardProgressService> logger)
    {
        _dbFactory = dbFactory;
        _logger = logger;
    }

    /// <inheritdoc />
    public async Task<SetupWizardStep> GetCurrentStepAsync(
        CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var record = await db.KeyValueRecords
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.Key == WizardProgressKey, cancellationToken);
        if (record is null || string.IsNullOrWhiteSpace(record.Value))
        {
            return SetupWizardStep.Welcome;
        }

        try
        {
            var marker = JsonSerializer.Deserialize<WizardProgressMarker>(record.Value);
            if (marker is null)
            {
                return SetupWizardStep.Welcome;
            }
            // Defensive: if a future build wrote a step value we do
            // not recognise, fall back to Welcome rather than throw.
            return Enum.IsDefined(typeof(SetupWizardStep), marker.Step)
                ? marker.Step
                : SetupWizardStep.Welcome;
        }
        catch (JsonException ex)
        {
            _logger.LogWarning(ex,
                "Wizard progress marker is malformed; treating as Welcome.");
            return SetupWizardStep.Welcome;
        }
    }

    /// <inheritdoc />
    public async Task SetCurrentStepAsync(
        SetupWizardStep step,
        CancellationToken cancellationToken = default)
    {
        if (!Enum.IsDefined(typeof(SetupWizardStep), step))
        {
            throw new ArgumentOutOfRangeException(nameof(step));
        }

        using var db = _dbFactory.GetContext();
        var record = await db.KeyValueRecords
            .FirstOrDefaultAsync(x => x.Key == WizardProgressKey, cancellationToken);

        var existingStep = SetupWizardStep.Welcome;
        if (record is not null && !string.IsNullOrWhiteSpace(record.Value))
        {
            try
            {
                existingStep = JsonSerializer
                    .Deserialize<WizardProgressMarker>(record.Value)?.Step
                    ?? SetupWizardStep.Welcome;
            }
            catch (JsonException)
            {
                // Treat a corrupt marker as Welcome and overwrite below.
                existingStep = SetupWizardStep.Welcome;
            }
        }

        if (existingStep >= step)
        {
            // Never move backwards.
            return;
        }

        var serialised = JsonSerializer.Serialize(new WizardProgressMarker
        {
            Step = step,
            UpdatedAtUtc = DateTimeOffset.UtcNow,
        });

        if (record is null)
        {
            await db.KeyValueRecords.AddAsync(
                new KeyValueRecord { Key = WizardProgressKey, Value = serialised },
                cancellationToken);
        }
        else
        {
            record.Value = serialised;
        }

        await db.SaveChangesAsync(cancellationToken);
        _logger.LogInformation("Wizard progress advanced to {Step}.", step);
    }

    private sealed class WizardProgressMarker
    {
        public SetupWizardStep Step { get; set; }
        public DateTimeOffset UpdatedAtUtc { get; set; }
    }
}
