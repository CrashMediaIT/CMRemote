using System;
using System.IO;

namespace Remotely.Shared.PackageManager;

/// <summary>
/// Validation helpers for operator-uploaded MSI bytes. Both the server
/// (on upload) and the agent (after download) call into this so the
/// "is this really an MSI?" decision is one piece of code with one set
/// of test vectors.
///
/// <para>An MSI is a Compound File Binary Format / OLE2 container, so
/// every legitimate MSI begins with the OLE2 magic
/// <c>D0 CF 11 E0 A1 B1 1A E1</c>. We deliberately do <em>not</em>
/// rely on the file extension, the HTTP <c>Content-Type</c>, or the
/// browser-supplied filename — operators upload from arbitrary
/// machines and we must not trust client-side content typing.</para>
/// </summary>
public static class MsiFileValidator
{
    /// <summary>
    /// The 8-byte OLE2 / CFBF signature. Every well-formed MSI begins
    /// with these bytes; anything that does not is rejected outright.
    /// </summary>
    public static ReadOnlySpan<byte> Ole2Signature => new byte[]
    {
        0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1
    };

    /// <summary>
    /// Hard upper bound on a single uploaded MSI. Sized generously to
    /// fit large vendor installers (e.g. Office, Visual Studio Build
    /// Tools redistributables) without running an unbounded-upload
    /// attack surface. The server also enforces this via
    /// <c>RequestSizeLimit</c>.
    /// </summary>
    public const long MaxMsiSizeBytes = 2L * 1024 * 1024 * 1024; // 2 GiB

    /// <summary>
    /// Bytes required to make a magic-byte decision. Equals the length
    /// of <see cref="Ole2Signature"/>.
    /// </summary>
    public const int MagicByteCount = 8;

    /// <summary>
    /// True iff <paramref name="prefix"/> contains at least
    /// <see cref="MagicByteCount"/> bytes whose first
    /// <see cref="MagicByteCount"/> match the OLE2 signature.
    /// </summary>
    public static bool HasOle2Magic(ReadOnlySpan<byte> prefix)
    {
        if (prefix.Length < MagicByteCount)
        {
            return false;
        }
        return prefix[..MagicByteCount].SequenceEqual(Ole2Signature);
    }

    /// <summary>
    /// True iff the file at <paramref name="path"/> exists and starts
    /// with the OLE2 magic. Does not load the whole file into memory.
    /// </summary>
    public static bool HasOle2Magic(string path)
    {
        if (string.IsNullOrEmpty(path) || !File.Exists(path))
        {
            return false;
        }
        Span<byte> buffer = stackalloc byte[MagicByteCount];
        using var fs = new FileStream(
            path,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            bufferSize: MagicByteCount,
            useAsync: false);
        var read = fs.Read(buffer);
        return read == MagicByteCount && HasOle2Magic(buffer);
    }

    /// <summary>
    /// Return the lowercase hex SHA-256 of the supplied stream. Reads
    /// from the current position to the end and does NOT rewind the
    /// stream afterwards — callers are responsible for seek behaviour
    /// (we don't seek so we can chain the hash off a forward-only
    /// upload stream).
    /// </summary>
    public static string ComputeSha256Hex(Stream input)
    {
        ArgumentNullException.ThrowIfNull(input);
        using var sha = System.Security.Cryptography.SHA256.Create();
        var hash = sha.ComputeHash(input);
        return Convert.ToHexString(hash).ToLowerInvariant();
    }

    /// <summary>
    /// Sanitises a browser-supplied filename to a safe leaf name we can
    /// drop on disk. Strips any path component, replaces invalid path
    /// chars with underscores, and bounds the length. Always returns a
    /// non-empty value (defaults to <c>"upload.msi"</c>).
    /// </summary>
    public static string SanitiseFileName(string? candidate)
    {
        if (string.IsNullOrWhiteSpace(candidate))
        {
            return "upload.msi";
        }
        var leaf = Path.GetFileName(candidate.Trim());
        if (string.IsNullOrEmpty(leaf))
        {
            return "upload.msi";
        }
        var invalid = Path.GetInvalidFileNameChars();
        Span<char> buffer = stackalloc char[Math.Min(leaf.Length, 255)];
        for (var i = 0; i < buffer.Length; i++)
        {
            var c = leaf[i];
            // NUL must always be stripped — it's the universal C-string
            // terminator and a notorious upload-path attack surface.
            if (c == '\0' || Array.IndexOf(invalid, c) >= 0)
            {
                buffer[i] = '_';
            }
            else
            {
                buffer[i] = c;
            }
        }
        var safe = new string(buffer).Trim('.', ' ');
        return string.IsNullOrEmpty(safe) ? "upload.msi" : safe;
    }
}
