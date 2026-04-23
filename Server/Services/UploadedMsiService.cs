using Microsoft.AspNetCore.Components.Forms;
using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.PackageManager;

namespace Remotely.Server.Services;

/// <summary>
/// Possible reasons an MSI upload can be refused. Surfaced through
/// <see cref="UploadMsiResult"/> so the page renders a precise message
/// without leaking server internals.
/// </summary>
public enum UploadMsiOutcome
{
    Ok = 0,
    InvalidArgs,
    EmptyFile,
    TooLarge,
    NotAnMsi,
    DuplicateSha256,
    StorageFailure,
}

/// <summary>
/// Outcome of an <see cref="IUploadedMsiService.UploadAsync"/> call.
/// On success carries the freshly-created <see cref="UploadedMsi"/>;
/// on failure carries a structured <see cref="UploadMsiOutcome"/> and
/// a message suitable for surfacing in the UI.
/// </summary>
public class UploadMsiResult
{
    public UploadMsiOutcome Outcome { get; init; }
    public string Message { get; init; } = string.Empty;
    public UploadedMsi? Value { get; init; }

    public bool IsSuccess => Outcome == UploadMsiOutcome.Ok && Value is not null;

    public static UploadMsiResult Ok(UploadedMsi value) =>
        new() { Outcome = UploadMsiOutcome.Ok, Value = value, Message = "Upload succeeded." };

    public static UploadMsiResult Fail(UploadMsiOutcome outcome, string message) =>
        new() { Outcome = outcome, Message = message };
}

/// <summary>
/// Org-scoped CRUD for <see cref="UploadedMsi"/> rows. The service is
/// the single entry point for putting MSI bytes into and out of the
/// system; magic-byte and SHA-256 validation, max-size enforcement,
/// org-scoping, and the tombstone-then-purge workflow all live here so
/// pages, hubs, and tests share one implementation.
/// </summary>
public interface IUploadedMsiService
{
    /// <summary>
    /// Validate, hash, and persist an operator-uploaded MSI.
    /// Validation order is deterministic: empty / too-large / not-an-MSI
    /// / duplicate / storage. The first failure short-circuits and the
    /// bytes are not persisted.
    /// </summary>
    Task<UploadMsiResult> UploadAsync(
        string organizationId,
        string? userId,
        string displayName,
        IBrowserFile file,
        string? description = null,
        CancellationToken cancellationToken = default);

    Task<IReadOnlyList<UploadedMsi>> GetForOrgAsync(
        string organizationId,
        bool includeTombstoned = false);

    Task<UploadedMsi?> GetAsync(string organizationId, Guid id);

    /// <summary>
    /// Tombstone (soft-delete) the row. The on-disk bytes stay so any
    /// in-flight install jobs referencing the matching <c>Package</c>
    /// can still resolve. Hard-deletion happens in
    /// <see cref="PurgeTombstonedAsync"/> once nothing references it.
    /// </summary>
    Task<bool> TombstoneAsync(string organizationId, Guid id);

    /// <summary>
    /// Hard-delete tombstoned rows whose <c>SharedFile</c> bytes are no
    /// longer referenced by a non-terminal <c>PackageInstallJob</c>.
    /// Idempotent and safe to call from a background sweeper. Returns
    /// the number of rows purged.
    /// </summary>
    Task<int> PurgeTombstonedAsync(CancellationToken cancellationToken = default);
}

public class UploadedMsiService : IUploadedMsiService
{
    private readonly IAppDbFactory _dbFactory;
    private readonly IDataService _dataService;
    private readonly ILogger<UploadedMsiService> _logger;

    public UploadedMsiService(
        IAppDbFactory dbFactory,
        IDataService dataService,
        ILogger<UploadedMsiService> logger)
    {
        _dbFactory = dbFactory;
        _dataService = dataService;
        _logger = logger;
    }

    public async Task<UploadMsiResult> UploadAsync(
        string organizationId,
        string? userId,
        string displayName,
        IBrowserFile file,
        string? description = null,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return UploadMsiResult.Fail(UploadMsiOutcome.InvalidArgs, "Organization is required.");
        }
        if (string.IsNullOrWhiteSpace(displayName))
        {
            return UploadMsiResult.Fail(UploadMsiOutcome.InvalidArgs, "Display name is required.");
        }
        if (file is null)
        {
            return UploadMsiResult.Fail(UploadMsiOutcome.InvalidArgs, "No file supplied.");
        }
        if (file.Size <= 0)
        {
            return UploadMsiResult.Fail(UploadMsiOutcome.EmptyFile, "Uploaded file is empty.");
        }
        if (file.Size > MsiFileValidator.MaxMsiSizeBytes)
        {
            return UploadMsiResult.Fail(
                UploadMsiOutcome.TooLarge,
                $"Uploaded file exceeds the maximum allowed size of {MsiFileValidator.MaxMsiSizeBytes / (1024 * 1024)} MiB.");
        }

        // Buffer to memory so we can both magic-check the prefix and
        // rehash the body without re-reading the browser stream (which
        // is forward-only). Bounded by MaxMsiSizeBytes above.
        using var ms = new MemoryStream(checked((int)Math.Min(file.Size, int.MaxValue)));
        await using (var src = file.OpenReadStream(MsiFileValidator.MaxMsiSizeBytes, cancellationToken))
        {
            await src.CopyToAsync(ms, cancellationToken);
        }

        var bytes = ms.ToArray();
        if (bytes.Length < MsiFileValidator.MagicByteCount ||
            !MsiFileValidator.HasOle2Magic(bytes.AsSpan(0, MsiFileValidator.MagicByteCount)))
        {
            return UploadMsiResult.Fail(
                UploadMsiOutcome.NotAnMsi,
                "Uploaded file is not a valid MSI (magic-byte check failed).");
        }

        ms.Position = 0;
        var sha256 = MsiFileValidator.ComputeSha256Hex(ms);
        var safeName = MsiFileValidator.SanitiseFileName(file.Name);

        using var db = _dbFactory.GetContext();

        // Refuse exact duplicates within the same org so the library
        // doesn't fill up with copies of the same bytes. Different orgs
        // may legitimately have the same MSI, so the dedupe is org-scoped.
        var duplicate = await db.UploadedMsis
            .AsNoTracking()
            .Where(x => x.OrganizationID == organizationId &&
                        !x.IsTombstoned &&
                        x.Sha256 == sha256)
            .FirstOrDefaultAsync(cancellationToken);
        if (duplicate is not null)
        {
            return UploadMsiResult.Fail(
                UploadMsiOutcome.DuplicateSha256,
                $"This MSI is already in the library as '{duplicate.Name}'.");
        }

        string sharedFileId;
        try
        {
            // Persist the bytes via the existing SharedFiles pipeline so
            // we re-use one storage path (and one cleanup sweep).
            sharedFileId = await _dataService.AddSharedFile(
                new MemoryStreamFormFile(bytes, safeName, "application/x-msi"),
                organizationId);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to persist uploaded MSI bytes. OrgId={orgId} FileName={fileName}",
                organizationId, safeName);
            return UploadMsiResult.Fail(UploadMsiOutcome.StorageFailure, "Failed to store uploaded file.");
        }

        var row = new UploadedMsi
        {
            Id = Guid.NewGuid(),
            OrganizationID = organizationId,
            SharedFileId = sharedFileId,
            Name = displayName.Trim(),
            FileName = safeName,
            SizeBytes = bytes.Length,
            Sha256 = sha256,
            Description = description,
            UploadedAt = DateTimeOffset.UtcNow,
            UploadedByUserId = userId,
        };
        db.UploadedMsis.Add(row);
        await db.SaveChangesAsync(cancellationToken);

        _logger.LogInformation(
            "Uploaded MSI accepted. Id={id} OrgId={orgId} Name={name} Size={size} Sha256={sha256} ByUser={userId}",
            row.Id, organizationId, row.Name, row.SizeBytes, sha256, userId);

        return UploadMsiResult.Ok(row);
    }

    public async Task<IReadOnlyList<UploadedMsi>> GetForOrgAsync(
        string organizationId,
        bool includeTombstoned = false)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            return Array.Empty<UploadedMsi>();
        }
        using var db = _dbFactory.GetContext();
        var query = db.UploadedMsis
            .AsNoTracking()
            .Where(x => x.OrganizationID == organizationId);
        if (!includeTombstoned)
        {
            query = query.Where(x => !x.IsTombstoned);
        }
        return await query
            .OrderByDescending(x => x.UploadedAt)
            .ToListAsync();
    }

    public async Task<UploadedMsi?> GetAsync(string organizationId, Guid id)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || id == Guid.Empty)
        {
            return null;
        }
        using var db = _dbFactory.GetContext();
        return await db.UploadedMsis
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.Id == id && x.OrganizationID == organizationId);
    }

    public async Task<bool> TombstoneAsync(string organizationId, Guid id)
    {
        if (string.IsNullOrWhiteSpace(organizationId) || id == Guid.Empty)
        {
            return false;
        }
        using var db = _dbFactory.GetContext();
        var row = await db.UploadedMsis
            .FirstOrDefaultAsync(x => x.Id == id && x.OrganizationID == organizationId);
        if (row is null || row.IsTombstoned)
        {
            return false;
        }
        row.IsTombstoned = true;
        row.TombstonedAt = DateTimeOffset.UtcNow;
        await db.SaveChangesAsync();

        _logger.LogInformation(
            "Uploaded MSI tombstoned. Id={id} OrgId={orgId} Name={name}",
            row.Id, organizationId, row.Name);
        return true;
    }

    public async Task<int> PurgeTombstonedAsync(CancellationToken cancellationToken = default)
    {
        using var db = _dbFactory.GetContext();
        var tombstoned = await db.UploadedMsis
            .Where(x => x.IsTombstoned)
            .ToListAsync(cancellationToken);
        if (tombstoned.Count == 0)
        {
            return 0;
        }

        var purged = 0;
        foreach (var row in tombstoned)
        {
            // Block hard-delete while a non-terminal job is still
            // referencing a Package that points at this MSI id. Once
            // every such job has reached a terminal status, we can
            // safely drop the row and its bytes.
            var idAsString = row.Id.ToString("D");
            var blocking = await db.PackageInstallJobs
                .AsNoTracking()
                .Where(j => j.Status == Shared.Enums.PackageInstallJobStatus.Queued ||
                            j.Status == Shared.Enums.PackageInstallJobStatus.Running)
                .Join(db.Packages.AsNoTracking(),
                      j => j.PackageId,
                      p => p.Id,
                      (j, p) => p)
                .AnyAsync(p => p.Provider == Shared.Enums.PackageProvider.UploadedMsi &&
                               p.PackageIdentifier == idAsString,
                          cancellationToken);
            if (blocking)
            {
                continue;
            }

            var sharedFile = await db.SharedFiles
                .FirstOrDefaultAsync(f => f.ID == row.SharedFileId, cancellationToken);

            db.UploadedMsis.Remove(row);
            if (sharedFile is not null)
            {
                db.SharedFiles.Remove(sharedFile);
            }
            purged++;
        }

        if (purged > 0)
        {
            await db.SaveChangesAsync(cancellationToken);
            _logger.LogInformation("Purged {count} tombstoned uploaded MSI(s).", purged);
        }
        return purged;
    }

    /// <summary>
    /// Adapter that lets us hand a buffered byte[] to
    /// <see cref="IDataService.AddSharedFile(Microsoft.AspNetCore.Http.IFormFile, string)"/>
    /// without re-reading from the browser stream. Internal because the
    /// shape is purely an implementation detail of the upload path.
    /// </summary>
    private sealed class MemoryStreamFormFile : Microsoft.AspNetCore.Http.IFormFile
    {
        private readonly byte[] _bytes;

        public MemoryStreamFormFile(byte[] bytes, string fileName, string contentType)
        {
            _bytes = bytes;
            FileName = fileName;
            Name = fileName;
            ContentType = contentType;
            Headers = new Microsoft.AspNetCore.Http.HeaderDictionary();
        }

        public string ContentType { get; }
        public string ContentDisposition => string.Empty;
        public Microsoft.AspNetCore.Http.IHeaderDictionary Headers { get; }
        public long Length => _bytes.LongLength;
        public string Name { get; }
        public string FileName { get; }
        public void CopyTo(Stream target) => target.Write(_bytes, 0, _bytes.Length);

        public Task CopyToAsync(Stream target, CancellationToken cancellationToken = default) =>
            target.WriteAsync(_bytes, 0, _bytes.Length, cancellationToken);

        public Stream OpenReadStream() => new MemoryStream(_bytes, writable: false);
    }
}
