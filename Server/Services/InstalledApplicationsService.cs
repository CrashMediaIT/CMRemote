using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Caching.Memory;
using Remotely.Server.Data;
using Remotely.Shared.Entities;
using Remotely.Shared.Models;
using System.Text.Json;

namespace Remotely.Server.Services;

/// <summary>
/// Per-device installed-applications snapshot store and uninstall-token
/// vault. The wire never carries a raw uninstall string — the WebUI
/// requests an uninstall by passing back the opaque token issued here,
/// and the agent re-resolves the actual command locally from its own
/// fresh inventory.
/// </summary>
public interface IInstalledApplicationsService
{
    /// <summary>
    /// Returns the latest snapshot stored for a device, or null if none
    /// has been collected yet.
    /// </summary>
    Task<(DateTimeOffset FetchedAt, IReadOnlyList<InstalledApplication> Applications)?> GetSnapshotAsync(string deviceId);

    /// <summary>
    /// Replaces the snapshot for a device and returns the materialized
    /// list. Token cache for the device is cleared so stale tokens
    /// referencing applications no longer present cannot be redeemed.
    /// </summary>
    Task<IReadOnlyList<InstalledApplication>> SaveSnapshotAsync(string deviceId, IReadOnlyList<InstalledApplication> applications, DateTimeOffset fetchedAt);

    /// <summary>
    /// Validates that the given application is present in the latest
    /// snapshot for this device, then issues a short-lived opaque token
    /// the WebUI can pass back to <see cref="ResolveUninstallToken"/>.
    /// Returns null when no matching application exists.
    /// </summary>
    string? IssueUninstallToken(string deviceId, string applicationKey);

    /// <summary>
    /// Resolves a token previously issued by <see cref="IssueUninstallToken"/>.
    /// Returns the application key on success; null when the token is
    /// unknown or expired. Tokens are single-use — successful resolution
    /// removes them from the cache.
    /// </summary>
    string? ResolveUninstallToken(string deviceId, string token);
}

public class InstalledApplicationsService : IInstalledApplicationsService
{
    private static readonly TimeSpan _tokenLifetime = TimeSpan.FromMinutes(5);
    private static readonly JsonSerializerOptions _jsonOptions = new() { WriteIndented = false };

    private readonly IAppDbFactory _appDbFactory;
    private readonly IMemoryCache _memoryCache;

    public InstalledApplicationsService(IAppDbFactory appDbFactory, IMemoryCache memoryCache)
    {
        _appDbFactory = appDbFactory;
        _memoryCache = memoryCache;
    }

    public async Task<(DateTimeOffset FetchedAt, IReadOnlyList<InstalledApplication> Applications)?> GetSnapshotAsync(string deviceId)
    {
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            return null;
        }

        using var db = _appDbFactory.GetContext();
        var row = await db.DeviceInstalledApplicationsSnapshots
            .AsNoTracking()
            .FirstOrDefaultAsync(x => x.DeviceId == deviceId);

        if (row is null)
        {
            return null;
        }

        var apps = DeserializeSnapshot(row.ApplicationsJson);
        return (row.FetchedAt, apps);
    }

    public async Task<IReadOnlyList<InstalledApplication>> SaveSnapshotAsync(string deviceId, IReadOnlyList<InstalledApplication> applications, DateTimeOffset fetchedAt)
    {
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            return Array.Empty<InstalledApplication>();
        }

        applications ??= Array.Empty<InstalledApplication>();

        using var db = _appDbFactory.GetContext();
        var json = JsonSerializer.Serialize(applications, _jsonOptions);

        var existing = await db.DeviceInstalledApplicationsSnapshots
            .FirstOrDefaultAsync(x => x.DeviceId == deviceId);

        if (existing is null)
        {
            db.DeviceInstalledApplicationsSnapshots.Add(new DeviceInstalledApplicationsSnapshot
            {
                DeviceId = deviceId,
                ApplicationsJson = json,
                FetchedAt = fetchedAt,
            });
        }
        else
        {
            existing.ApplicationsJson = json;
            existing.FetchedAt = fetchedAt;
        }

        await db.SaveChangesAsync();

        // Drop any uninstall tokens referencing the previous snapshot so
        // operators can't redeem a token that resolves to an app that has
        // since vanished from the device.
        InvalidateAllTokensForDevice(deviceId);

        return applications;
    }

    public string? IssueUninstallToken(string deviceId, string applicationKey)
    {
        if (string.IsNullOrWhiteSpace(deviceId) || string.IsNullOrWhiteSpace(applicationKey))
        {
            return null;
        }

        // Validate against the persisted snapshot so we don't hand out
        // tokens for applications the agent hasn't reported.
        using var db = _appDbFactory.GetContext();
        var row = db.DeviceInstalledApplicationsSnapshots
            .AsNoTracking()
            .FirstOrDefault(x => x.DeviceId == deviceId);

        if (row is null)
        {
            return null;
        }

        var apps = DeserializeSnapshot(row.ApplicationsJson);
        var match = apps.FirstOrDefault(a =>
            string.Equals(a.ApplicationKey, applicationKey, StringComparison.OrdinalIgnoreCase));
        if (match is null)
        {
            return null;
        }

        var token = Guid.NewGuid().ToString("N");
        var entry = new TokenEntry(deviceId, applicationKey);

        _memoryCache.Set(TokenCacheKey(deviceId, token), entry, new MemoryCacheEntryOptions
        {
            AbsoluteExpirationRelativeToNow = _tokenLifetime,
        });
        TrackTokenForDevice(deviceId, token);
        return token;
    }

    public string? ResolveUninstallToken(string deviceId, string token)
    {
        if (string.IsNullOrWhiteSpace(deviceId) || string.IsNullOrWhiteSpace(token))
        {
            return null;
        }

        var key = TokenCacheKey(deviceId, token);
        if (!_memoryCache.TryGetValue(key, out TokenEntry? entry) || entry is null)
        {
            return null;
        }

        if (!string.Equals(entry.DeviceId, deviceId, StringComparison.OrdinalIgnoreCase))
        {
            return null;
        }

        _memoryCache.Remove(key);
        UntrackTokenForDevice(deviceId, token);
        return entry.ApplicationKey;
    }

    private static IReadOnlyList<InstalledApplication> DeserializeSnapshot(string json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            return Array.Empty<InstalledApplication>();
        }
        try
        {
            return JsonSerializer.Deserialize<List<InstalledApplication>>(json, _jsonOptions)
                ?? new List<InstalledApplication>();
        }
        catch (JsonException)
        {
            return Array.Empty<InstalledApplication>();
        }
    }

    private static string TokenCacheKey(string deviceId, string token) => $"InstalledApps:Token:{deviceId}:{token}";

    private static string TokenIndexKey(string deviceId) => $"InstalledApps:TokenIndex:{deviceId}";

    private void TrackTokenForDevice(string deviceId, string token)
    {
        var indexKey = TokenIndexKey(deviceId);
        var set = _memoryCache.GetOrCreate(indexKey, entry =>
        {
            entry.AbsoluteExpirationRelativeToNow = _tokenLifetime;
            return new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        });
        lock (set!)
        {
            set.Add(token);
        }
    }

    private void UntrackTokenForDevice(string deviceId, string token)
    {
        if (_memoryCache.TryGetValue(TokenIndexKey(deviceId), out HashSet<string>? set) && set is not null)
        {
            lock (set)
            {
                set.Remove(token);
            }
        }
    }

    private void InvalidateAllTokensForDevice(string deviceId)
    {
        var indexKey = TokenIndexKey(deviceId);
        if (!_memoryCache.TryGetValue(indexKey, out HashSet<string>? set) || set is null)
        {
            return;
        }
        string[] toRemove;
        lock (set)
        {
            toRemove = set.ToArray();
            set.Clear();
        }
        foreach (var token in toRemove)
        {
            _memoryCache.Remove(TokenCacheKey(deviceId, token));
        }
        _memoryCache.Remove(indexKey);
    }

    private sealed record TokenEntry(string DeviceId, string ApplicationKey);
}
