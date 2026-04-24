using Microsoft.AspNetCore.Identity;

namespace Remotely.Server.Services.Setup;

/// <summary>
/// Result of an <see cref="IAdminBootstrapService.CreateInitialAdminAsync"/>
/// call. Modelled as a small discriminated record set so the wizard can
/// distinguish "you typed a weak password" (re-render the form with the
/// password rules) from "an org with this email already exists" (suggest
/// they sign in instead) from "we wrote the row, advance the wizard".
/// </summary>
public sealed class AdminBootstrapResult
{
    /// <summary>True when the org + admin user were both persisted.</summary>
    public bool IsSuccess { get; init; }

    /// <summary>
    /// Operator-visible error messages. Empty when
    /// <see cref="IsSuccess"/> is <c>true</c>. Sourced from the
    /// underlying <see cref="IdentityResult.Errors"/> when the
    /// failure is a password-policy / duplicate-email rejection;
    /// otherwise contains a single human-readable wrapper message.
    /// </summary>
    public IReadOnlyList<string> Errors { get; init; } = Array.Empty<string>();

    /// <summary>
    /// The newly-created user's id when <see cref="IsSuccess"/> is
    /// <c>true</c>; <c>null</c> otherwise. Surfaced so the
    /// <c>/setup/done</c> step can sign the operator in via
    /// <see cref="SignInManager{TUser}.SignInAsync"/> without a
    /// second database round-trip.
    /// </summary>
    public string? CreatedUserId { get; init; }

    internal static AdminBootstrapResult Success(string userId) =>
        new() { IsSuccess = true, CreatedUserId = userId };

    internal static AdminBootstrapResult Failure(params string[] errors) =>
        new() { IsSuccess = false, Errors = errors };

    internal static AdminBootstrapResult FromIdentityErrors(IEnumerable<IdentityError> errors) =>
        new()
        {
            IsSuccess = false,
            Errors = errors.Select(e => e.Description).ToList(),
        };
}

/// <summary>
/// M1.4 — first-organisation + first-server-admin bootstrap. Wraps
/// the ASP.NET Identity pipeline (so the password hash + security
/// stamp end up exactly the way the rest of the app expects them)
/// and the v2 Postgres schema's `Organizations` table in one
/// transaction-style operation: either both rows land or neither
/// does.
///
/// The step is *optional*: if M1.3 imported a populated user table
/// the wizard skips straight to M1.5 — see <see cref="IsRequiredAsync"/>.
///
/// Deliberately a separate service rather than a method on
/// <see cref="DataService"/> so the wizard can exercise it in
/// isolation in tests and so future refactors of `DataService` do
/// not silently change the bootstrap contract.
/// </summary>
public interface IAdminBootstrapService
{
    /// <summary>
    /// True when the database has zero <c>Users</c> rows AND zero
    /// <c>Organizations</c> rows — the "greenfield + nothing
    /// imported" case where the operator must create the first
    /// admin before the panel becomes usable. False otherwise: the
    /// wizard will auto-skip M1.4 (admin bootstrap is meaningless
    /// once *some* admin exists from an import).
    /// </summary>
    Task<bool> IsRequiredAsync(CancellationToken cancellationToken = default);

    /// <summary>
    /// Creates a fresh organisation named <paramref name="organizationName"/>
    /// and a single <c>RemotelyUser</c> inside it with
    /// <c>IsAdministrator = true</c> and <c>IsServerAdmin = true</c>,
    /// using the configured ASP.NET Identity password hasher. The
    /// email address is also used as the user name (matching the
    /// rest of the app's convention).
    ///
    /// Idempotency is intentionally *not* the contract here: the
    /// caller (the wizard page) only invokes this method when
    /// <see cref="IsRequiredAsync"/> returned <c>true</c>; if the
    /// caller invokes it again after success, Identity will return
    /// a duplicate-email error and the wrapper translates that to
    /// an operator-visible "this email already exists" message.
    /// </summary>
    Task<AdminBootstrapResult> CreateInitialAdminAsync(
        string organizationName,
        string email,
        string password,
        CancellationToken cancellationToken = default);
}
