namespace Remotely.Shared.Dtos;

/// <summary>
/// Payload sent from agent → server when an uninstall completes (or fails).
/// </summary>
public class UninstallApplicationResultDto
{
    public string RequestId { get; set; } = string.Empty;

    /// <summary>
    /// Application key that was targeted. Echoed back so the server can
    /// invalidate any cached uninstall token for this app.
    /// </summary>
    public string ApplicationKey { get; set; } = string.Empty;

    public bool Success { get; set; }
    public int ExitCode { get; set; }

    /// <summary>
    /// Captured stdout, truncated by the agent to keep wire size bounded.
    /// </summary>
    public string? Stdout { get; set; }

    /// <summary>
    /// Captured stderr, truncated by the agent to keep wire size bounded.
    /// </summary>
    public string? Stderr { get; set; }

    public long DurationMs { get; set; }

    public string? ErrorMessage { get; set; }
}
