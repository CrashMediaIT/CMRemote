namespace Remotely.Migration.Legacy;

/// <summary>
/// Pure mapping function from one legacy-schema row (<typeparamref name="TLegacy"/>)
/// to one v2-schema row (<typeparamref name="TV2"/>).
///
/// Per `ROADMAP.md` "M2 — Schema converter library" converters are
/// **versioned** — there is one implementation per (legacy schema
/// version, entity) pair, so a previously-shipped converter set can
/// stay byte-stable while a new upstream version gets a fresh set.
///
/// Implementations must:
/// <list type="bullet">
///   <item>Be pure: no I/O, no static mutable state. The runner is
///         responsible for batching, transactions, and writes.</item>
///   <item>Be deterministic: the same legacy row converts to the same
///         v2 row on every call (the importer is resumable).</item>
///   <item>Preserve identity: an entity that exists in the source
///         under id <c>X</c> must exist in the target under id
///         <c>X</c> too, so already-deployed agents reconnect under
///         the same record (per ROADMAP M1.3).</item>
/// </list>
/// </summary>
public interface IRowConverter<TLegacy, TV2>
{
    /// <summary>Logical entity name surfaced in <see cref="EntityReport.EntityName"/>.</summary>
    string EntityName { get; }

    /// <summary>Schema version this converter handles (a converter is single-version).</summary>
    LegacySchemaVersion HandlesSchemaVersion { get; }

    /// <summary>Maps one legacy row.</summary>
    ConverterResult<TV2> Convert(TLegacy legacyRow);
}

/// <summary>
/// Outcome of a single <see cref="IRowConverter{TLegacy, TV2}.Convert"/> call.
/// </summary>
public readonly struct ConverterResult<T>
{
    private ConverterResult(T? value, bool isSkipped, string? skipReason, string? errorMessage)
    {
        Value = value;
        IsSkipped = isSkipped;
        SkipReason = skipReason;
        ErrorMessage = errorMessage;
    }

    public T? Value { get; }
    public bool IsSkipped { get; }
    public string? SkipReason { get; }
    public string? ErrorMessage { get; }

    public bool IsSuccess => !IsSkipped && ErrorMessage is null && Value is not null;
    public bool IsFailure => ErrorMessage is not null;

    /// <summary>The row converted cleanly; runner writes it to the target.</summary>
    public static ConverterResult<T> Ok(T value) => new(value, false, null, null);

    /// <summary>The row was deliberately skipped (e.g. orphaned FK). Counted, not written.</summary>
    public static ConverterResult<T> Skip(string reason) => new(default, true, reason, null);

    /// <summary>The row failed to convert (e.g. invalid invariant). Counted, not written, error retained in the report.</summary>
    public static ConverterResult<T> Fail(string error) => new(default, false, null, error);
}
