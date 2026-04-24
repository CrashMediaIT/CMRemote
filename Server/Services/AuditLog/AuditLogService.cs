using System.Collections.Concurrent;
using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Services;

namespace Remotely.Server.Services.AuditLog;

/// <summary>
/// Public surface of the immutable audit log (ROADMAP.md "Track S /
/// S7 — Runtime security posture: an immutable audit log"). Holds the
/// hash-chain invariants in one place so callers (auth, install-job
/// dispatch, agent-upgrade dispatch) only have to call
/// <see cref="AppendAsync"/> with their event-specific payload.
/// </summary>
public interface IAuditLogService
{
    /// <summary>
    /// Appends one row to <paramref name="organizationId"/>'s chain,
    /// computing the linkage hash + sequence number under a per-org
    /// write lock. Returns the persisted row.
    /// </summary>
    Task<AuditLogEntry> AppendAsync(
        string organizationId,
        string eventType,
        string actorId,
        string subjectId,
        string summary,
        object? detail = null,
        CancellationToken cancellationToken = default);

    /// <summary>
    /// Verifies <paramref name="organizationId"/>'s chain. Returns the
    /// sequence number of the first row that fails verification (the
    /// row whose <see cref="AuditLogEntry.EntryHash"/> does not match
    /// the recomputed hash, or whose
    /// <see cref="AuditLogEntry.PrevHash"/> does not match the previous
    /// row's <see cref="AuditLogEntry.EntryHash"/>), or <c>null</c>
    /// when the chain is intact end-to-end.
    /// </summary>
    Task<long?> VerifyChainAsync(string organizationId, CancellationToken cancellationToken = default);
}

public class AuditLogService : IAuditLogService
{
    /// <summary>
    /// 64 lower-case zeros — the genesis prev-hash for an empty chain.
    /// </summary>
    public const string GenesisPrevHash = "0000000000000000000000000000000000000000000000000000000000000000";

    private static readonly JsonSerializerOptions _detailJsonOptions = new()
    {
        PropertyNamingPolicy = null,
        WriteIndented = false,
    };

    private readonly IAppDbFactory _dbFactory;
    private readonly ISystemTime _systemTime;
    private readonly ILogger<AuditLogService> _logger;

    /// <summary>
    /// Per-org write lock. The chain is monotone within an org but
    /// independent across orgs, so we shard the lock by org id rather
    /// than serialising every audit append in the process.
    /// </summary>
    private readonly ConcurrentDictionary<string, SemaphoreSlim> _orgLocks =
        new(StringComparer.Ordinal);

    public AuditLogService(
        IAppDbFactory dbFactory,
        ISystemTime systemTime,
        ILogger<AuditLogService> logger)
    {
        _dbFactory = dbFactory;
        _systemTime = systemTime;
        _logger = logger;
    }

    public async Task<AuditLogEntry> AppendAsync(
        string organizationId,
        string eventType,
        string actorId,
        string subjectId,
        string summary,
        object? detail = null,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            throw new ArgumentException("Organization ID is required.", nameof(organizationId));
        }
        if (string.IsNullOrWhiteSpace(eventType))
        {
            throw new ArgumentException("Event type is required.", nameof(eventType));
        }

        var lockObj = _orgLocks.GetOrAdd(organizationId, _ => new SemaphoreSlim(1, 1));
        await lockObj.WaitAsync(cancellationToken);
        try
        {
            using var db = _dbFactory.GetContext();
            var prev = await db.AuditLogEntries
                .AsNoTracking()
                .Where(e => e.OrganizationID == organizationId)
                .OrderByDescending(e => e.Sequence)
                .FirstOrDefaultAsync(cancellationToken);

            var sequence = (prev?.Sequence ?? 0) + 1;
            var prevHash = prev?.EntryHash ?? GenesisPrevHash;

            var detailJson = SerializeDetail(detail);
            var entry = new AuditLogEntry
            {
                Id = Guid.NewGuid(),
                OrganizationID = organizationId,
                Sequence = sequence,
                OccurredAt = _systemTime.Now,
                EventType = Truncate(eventType, 64),
                ActorId = Truncate(actorId ?? string.Empty, 128),
                SubjectId = Truncate(subjectId ?? string.Empty, 256),
                Summary = Truncate(summary ?? string.Empty, 1024),
                DetailJson = detailJson,
                PrevHash = prevHash,
            };
            entry.EntryHash = ComputeEntryHash(entry);

            db.AuditLogEntries.Add(entry);
            await db.SaveChangesAsync(cancellationToken);

            _logger.LogDebug(
                "Audit log appended. Org={org} Seq={seq} Type={type} Actor={actor} Subject={subject}.",
                organizationId, sequence, entry.EventType, entry.ActorId, entry.SubjectId);

            return entry;
        }
        finally
        {
            lockObj.Release();
        }
    }

    public async Task<long?> VerifyChainAsync(string organizationId, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(organizationId))
        {
            throw new ArgumentException("Organization ID is required.", nameof(organizationId));
        }

        using var db = _dbFactory.GetContext();
        var rows = db.AuditLogEntries
            .AsNoTracking()
            .Where(e => e.OrganizationID == organizationId)
            .OrderBy(e => e.Sequence)
            .AsAsyncEnumerable();

        string expectedPrev = GenesisPrevHash;
        long expectedSeq = 1;
        await foreach (var row in rows.WithCancellation(cancellationToken))
        {
            if (row.Sequence != expectedSeq)
            {
                _logger.LogWarning(
                    "Audit chain gap detected for Org={org} at Seq={seq} (expected {expected}).",
                    organizationId, row.Sequence, expectedSeq);
                return row.Sequence;
            }
            if (!string.Equals(row.PrevHash, expectedPrev, StringComparison.Ordinal))
            {
                _logger.LogWarning(
                    "Audit chain link broken for Org={org} at Seq={seq}.",
                    organizationId, row.Sequence);
                return row.Sequence;
            }
            var recomputed = ComputeEntryHash(row);
            if (!string.Equals(row.EntryHash, recomputed, StringComparison.Ordinal))
            {
                _logger.LogWarning(
                    "Audit chain hash mismatch for Org={org} at Seq={seq}.",
                    organizationId, row.Sequence);
                return row.Sequence;
            }
            expectedPrev = row.EntryHash;
            expectedSeq = row.Sequence + 1;
        }

        return null;
    }

    /// <summary>
    /// Computes the SHA-256 over the canonical body of an entry. Public
    /// so tests can re-derive the expected hash.
    /// </summary>
    public static string ComputeEntryHash(AuditLogEntry entry)
    {
        // Canonical form: PrevHash + '\n' + each field tagged + '\n'
        // separated, then hashed with SHA-256, hex lower-case. The
        // tagged form means a future field addition that's serialised
        // as `Tag=` cannot collide with an older row's value.
        var sb = new StringBuilder(512);
        sb.Append(entry.PrevHash).Append('\n');
        sb.Append("Org=").Append(entry.OrganizationID).Append('\n');
        sb.Append("Seq=").Append(entry.Sequence.ToString(CultureInfo.InvariantCulture)).Append('\n');
        sb.Append("OccurredAt=").Append(entry.OccurredAt.ToUniversalTime().ToString("O", CultureInfo.InvariantCulture)).Append('\n');
        sb.Append("Type=").Append(entry.EventType).Append('\n');
        sb.Append("Actor=").Append(entry.ActorId).Append('\n');
        sb.Append("Subject=").Append(entry.SubjectId).Append('\n');
        sb.Append("Summary=").Append(entry.Summary).Append('\n');
        sb.Append("Detail=").Append(entry.DetailJson ?? string.Empty).Append('\n');

        var hash = SHA256.HashData(Encoding.UTF8.GetBytes(sb.ToString()));
        return Convert.ToHexString(hash).ToLowerInvariant();
    }

    private static string? SerializeDetail(object? detail)
    {
        if (detail is null)
        {
            return null;
        }
        var raw = JsonSerializer.Serialize(detail, _detailJsonOptions);
        using var doc = JsonDocument.Parse(raw);
        var sb = new StringBuilder();
        WriteCanonical(doc.RootElement, sb);
        return sb.ToString();
    }

    private static void WriteCanonical(JsonElement element, StringBuilder sb)
    {
        switch (element.ValueKind)
        {
            case JsonValueKind.Object:
                sb.Append('{');
                bool first = true;
                foreach (var prop in element.EnumerateObject().OrderBy(p => p.Name, StringComparer.Ordinal))
                {
                    if (!first) sb.Append(',');
                    first = false;
                    sb.Append(JsonSerializer.Serialize(prop.Name));
                    sb.Append(':');
                    WriteCanonical(prop.Value, sb);
                }
                sb.Append('}');
                break;
            case JsonValueKind.Array:
                sb.Append('[');
                bool firstElem = true;
                foreach (var item in element.EnumerateArray())
                {
                    if (!firstElem) sb.Append(',');
                    firstElem = false;
                    WriteCanonical(item, sb);
                }
                sb.Append(']');
                break;
            default:
                sb.Append(element.GetRawText());
                break;
        }
    }

    private static string Truncate(string s, int max) =>
        string.IsNullOrEmpty(s) || s.Length <= max ? s : s[..max];
}
