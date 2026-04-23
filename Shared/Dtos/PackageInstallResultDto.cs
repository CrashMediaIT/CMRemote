namespace Remotely.Shared.Dtos;

/// <summary>
/// Wire payload reporting the outcome of a <c>PackageInstallRequestDto</c>
/// from agent to server.
/// </summary>
public class PackageInstallResultDto
{
    public string JobId { get; set; } = string.Empty;

    public bool Success { get; set; }

    public int ExitCode { get; set; }

    public long DurationMs { get; set; }

    public string? StdoutTail { get; set; }

    public string? StderrTail { get; set; }

    public string? ErrorMessage { get; set; }
}
