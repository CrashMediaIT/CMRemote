using System.ComponentModel.DataAnnotations;

namespace Remotely.Shared.Entities;

/// <summary>
/// Captured outcome of a <see cref="PackageInstallJob"/> reported by the
/// agent. One row per terminated job — written exactly once when the
/// job transitions to a terminal state.
/// </summary>
public class PackageInstallResult
{
    [Key]
    public Guid Id { get; set; } = Guid.NewGuid();

    public Guid PackageInstallJobId { get; set; }

    public PackageInstallJob? PackageInstallJob { get; set; }

    public int ExitCode { get; set; }

    public bool Success { get; set; }

    public long DurationMs { get; set; }

    /// <summary>
    /// Tail of stdout the agent captured (truncated to a few KB to keep
    /// rows small — full logs stay on the agent).
    /// </summary>
    [StringLength(16 * 1024)]
    public string? StdoutTail { get; set; }

    /// <summary>
    /// Tail of stderr — surfaced in the UI on failure to give the
    /// operator immediate feedback.
    /// </summary>
    [StringLength(16 * 1024)]
    public string? StderrTail { get; set; }

    /// <summary>
    /// Operator-facing error summary set by the agent when the
    /// invocation failed before producing a real exit code (provider
    /// missing, package not found, timeout, etc.).
    /// </summary>
    [StringLength(1024)]
    public string? ErrorMessage { get; set; }
}
