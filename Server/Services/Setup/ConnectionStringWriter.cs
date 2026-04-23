using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Extensions.Configuration;

namespace Remotely.Server.Services.Setup;

/// <inheritdoc cref="IConnectionStringWriter" />
public class ConnectionStringWriter : IConnectionStringWriter
{
    private readonly IConfiguration _configuration;
    private readonly ILogger<ConnectionStringWriter> _logger;

    public ConnectionStringWriter(
        IWebHostEnvironment hostEnvironment,
        IConfiguration configuration,
        ILogger<ConnectionStringWriter> logger)
        : this(
            DeriveTargetPath(hostEnvironment),
            configuration,
            logger)
    {
    }

    /// <summary>
    /// Test-friendly overload that accepts an explicit target path.
    /// The DI overload above derives the path from
    /// <see cref="IWebHostEnvironment.ContentRootPath"/> and the
    /// hard-coded filename <c>appsettings.Production.json</c>.
    /// </summary>
    internal ConnectionStringWriter(
        string targetSettingsPath,
        IConfiguration configuration,
        ILogger<ConnectionStringWriter> logger)
    {
        if (string.IsNullOrWhiteSpace(targetSettingsPath))
        {
            throw new ArgumentException(
                "Target settings path must not be empty.",
                nameof(targetSettingsPath));
        }
        TargetSettingsPath = targetSettingsPath;
        _configuration = configuration;
        _logger = logger;
    }

    private static string DeriveTargetPath(IWebHostEnvironment hostEnvironment)
    {
        // We deliberately always write to appsettings.Production.json
        // (not appsettings.{Environment}.json) so the wizard's output
        // is stable regardless of the ASPNETCORE_ENVIRONMENT the
        // operator happens to be running under at install time. This
        // matches the M1.2 ROADMAP entry verbatim.
        return Path.Combine(
            hostEnvironment.ContentRootPath,
            "appsettings.Production.json");
    }

    /// <inheritdoc />
    public string TargetSettingsPath { get; }

    /// <inheritdoc />
    public async Task WritePostgresConnectionAsync(
        string postgresConnectionString,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(postgresConnectionString))
        {
            throw new ArgumentException(
                "Postgres connection string must not be empty.",
                nameof(postgresConnectionString));
        }

        var existing = await ReadExistingAsync(cancellationToken).ConfigureAwait(false);

        // ConnectionStrings:PostgreSQL — note the casing matches the
        // reads in PostgreSqlDbContext.cs / DesignTimeContexts.cs.
        var connections = existing["ConnectionStrings"] as JsonObject ?? new JsonObject();
        connections["PostgreSQL"] = postgresConnectionString;
        existing["ConnectionStrings"] = connections;

        // ApplicationOptions:DbProvider=PostgreSql — case-insensitive
        // on the read side (Program.cs lowercases it), but we write
        // PascalCase to match the rest of the file.
        var appOptions = existing["ApplicationOptions"] as JsonObject ?? new JsonObject();
        appOptions["DbProvider"] = "PostgreSql";
        existing["ApplicationOptions"] = appOptions;

        var serialised = existing.ToJsonString(new JsonSerializerOptions
        {
            WriteIndented = true,
        });

        // Atomic write: temp + rename so a crashed write never leaves
        // a half-truncated settings file on disk.
        var directory = Path.GetDirectoryName(TargetSettingsPath)!;
        Directory.CreateDirectory(directory);
        var tempPath = TargetSettingsPath + ".tmp";

        await File.WriteAllTextAsync(
                tempPath,
                serialised + Environment.NewLine,
                Encoding.UTF8,
                cancellationToken)
            .ConfigureAwait(false);

        ApplyOwnerOnlyMode(tempPath);

        // File.Move(overwrite: true) is atomic on Unix and on
        // ReplaceFile-supporting Windows volumes.
        File.Move(tempPath, TargetSettingsPath, overwrite: true);
        ApplyOwnerOnlyMode(TargetSettingsPath);

        _logger.LogInformation(
            "Wrote Postgres connection string to {Path}.", TargetSettingsPath);

        // Trigger an in-process configuration reload so the next
        // request to the wizard's "test" / DbContext path sees the
        // new value without a process restart. IConfiguration in
        // ASP.NET Core is the IConfigurationRoot that backs the
        // builder, so the cast is safe; on the off chance a custom
        // host wrapped it we just log and skip.
        if (_configuration is IConfigurationRoot root)
        {
            try
            {
                root.Reload();
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex,
                    "IConfigurationRoot.Reload failed; new settings will apply on next process restart.");
            }
        }
        else
        {
            _logger.LogWarning(
                "IConfiguration is not the root configuration; new settings will apply on next process restart.");
        }
    }

    private async Task<JsonObject> ReadExistingAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(TargetSettingsPath))
        {
            return new JsonObject();
        }

        try
        {
            await using var stream = File.OpenRead(TargetSettingsPath);
            var node = await JsonNode.ParseAsync(
                    stream,
                    documentOptions: new JsonDocumentOptions
                    {
                        AllowTrailingCommas = true,
                        CommentHandling = JsonCommentHandling.Skip,
                    },
                    cancellationToken: cancellationToken)
                .ConfigureAwait(false);
            return node as JsonObject ?? new JsonObject();
        }
        catch (Exception ex) when (ex is JsonException or IOException)
        {
            _logger.LogWarning(ex,
                "Existing {Path} could not be parsed as JSON; replacing it.",
                TargetSettingsPath);
            return new JsonObject();
        }
    }

    /// <summary>
    /// Best-effort <c>chmod 0600</c> on Unix so the connection string
    /// (which contains the database password) is not world-readable.
    /// On Windows the file inherits the parent directory's ACL,
    /// which Track S / S6 covers separately.
    /// </summary>
    private void ApplyOwnerOnlyMode(string path)
    {
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Linux) &&
            !RuntimeInformation.IsOSPlatform(OSPlatform.OSX) &&
            !RuntimeInformation.IsOSPlatform(OSPlatform.FreeBSD))
        {
            return;
        }

        try
        {
            File.SetUnixFileMode(
                path,
                UnixFileMode.UserRead | UnixFileMode.UserWrite);
        }
        catch (Exception ex) when (
            ex is UnauthorizedAccessException
            or IOException
            or PlatformNotSupportedException)
        {
            _logger.LogWarning(ex,
                "Could not set 0600 mode on {Path}; the file may be readable by other local users.",
                path);
        }
    }
}
