namespace Remotely.Shared.Enums;

/// <summary>
/// State machine for a <c>PackageInstallJob</c>. Transitions are
/// strictly:
/// <list type="bullet">
///   <item><c>Queued → Running → (Success | Failed | Cancelled)</c></item>
///   <item><c>Queued → Cancelled</c> (operator cancels before dispatch)</item>
/// </list>
/// Terminal states (<c>Success</c>, <c>Failed</c>, <c>Cancelled</c>) cannot
/// transition further.
/// </summary>
public enum PackageInstallJobStatus
{
    Queued = 0,
    Running = 1,
    Success = 2,
    Failed = 3,
    Cancelled = 4,
}
