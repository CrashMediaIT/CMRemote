using System;
using System.IO;
using System.Linq;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Shared.PackageManager;

namespace Remotely.Shared.Tests;

[TestClass]
public class MsiFileValidatorTests
{
    private static readonly byte[] Ole2Magic = new byte[]
    {
        0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1
    };

    [TestMethod]
    public void HasOle2Magic_AcceptsExactSignature()
    {
        Assert.IsTrue(MsiFileValidator.HasOle2Magic(Ole2Magic));
    }

    [TestMethod]
    public void HasOle2Magic_AcceptsLongerPrefixThatStartsWithSignature()
    {
        var bytes = Ole2Magic.Concat(new byte[] { 0x01, 0x02, 0x03, 0x04 }).ToArray();
        Assert.IsTrue(MsiFileValidator.HasOle2Magic(bytes));
    }

    [TestMethod]
    public void HasOle2Magic_RejectsShorterThanSignature()
    {
        // 7 bytes — one short of the magic length.
        var bytes = Ole2Magic.Take(7).ToArray();
        Assert.IsFalse(MsiFileValidator.HasOle2Magic(bytes));
    }

    [TestMethod]
    public void HasOle2Magic_RejectsArbitraryBytes()
    {
        // A ZIP-style PK signature must NEVER look like a valid MSI.
        var bytes = new byte[] { 0x50, 0x4B, 0x03, 0x04, 0, 0, 0, 0 };
        Assert.IsFalse(MsiFileValidator.HasOle2Magic(bytes));
    }

    [TestMethod]
    public void HasOle2Magic_RejectsAllZeroes()
    {
        var bytes = new byte[64];
        Assert.IsFalse(MsiFileValidator.HasOle2Magic(bytes));
    }

    [TestMethod]
    public void HasOle2Magic_File_ReturnsFalseForMissingPath()
    {
        Assert.IsFalse(MsiFileValidator.HasOle2Magic("/no/such/file.msi"));
    }

    [TestMethod]
    public void HasOle2Magic_File_DetectsValidPrefix()
    {
        var temp = Path.GetTempFileName();
        try
        {
            File.WriteAllBytes(temp, Ole2Magic.Concat(new byte[1024]).ToArray());
            Assert.IsTrue(MsiFileValidator.HasOle2Magic(temp));
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [TestMethod]
    public void HasOle2Magic_File_RejectsJunkPrefix()
    {
        var temp = Path.GetTempFileName();
        try
        {
            File.WriteAllBytes(temp, Enumerable.Repeat((byte)0x42, 32).ToArray());
            Assert.IsFalse(MsiFileValidator.HasOle2Magic(temp));
        }
        finally
        {
            File.Delete(temp);
        }
    }

    [TestMethod]
    public void ComputeSha256Hex_MatchesKnownVectorForEmptyStream()
    {
        // Empty SHA-256 vector: well-known constant.
        using var ms = new MemoryStream(Array.Empty<byte>());
        var hash = MsiFileValidator.ComputeSha256Hex(ms);
        Assert.AreEqual(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            hash);
    }

    [TestMethod]
    public void ComputeSha256Hex_IsLowercaseHex()
    {
        using var ms = new MemoryStream(new byte[] { 1, 2, 3, 4, 5 });
        var hash = MsiFileValidator.ComputeSha256Hex(ms);
        Assert.AreEqual(64, hash.Length);
        foreach (var c in hash)
        {
            Assert.IsTrue(
                (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'),
                $"Non lowercase-hex char: '{c}' in '{hash}'");
        }
    }

    [TestMethod]
    public void SanitiseFileName_StripsDirectoryComponents()
    {
        // Path traversal attempts must collapse to the leaf name only.
        Assert.AreEqual("evil.msi", MsiFileValidator.SanitiseFileName("../../../etc/evil.msi"));
        Assert.AreEqual("payload.msi", MsiFileValidator.SanitiseFileName("/tmp/payload.msi"));
    }

    [TestMethod]
    public void SanitiseFileName_DefaultsWhenEmptyOrNull()
    {
        Assert.AreEqual("upload.msi", MsiFileValidator.SanitiseFileName(null));
        Assert.AreEqual("upload.msi", MsiFileValidator.SanitiseFileName(""));
        Assert.AreEqual("upload.msi", MsiFileValidator.SanitiseFileName("   "));
    }

    [TestMethod]
    public void SanitiseFileName_ReplacesNullByteAndInvalidChars()
    {
        var sanitised = MsiFileValidator.SanitiseFileName("evil\0name.msi");
        Assert.IsFalse(sanitised.Contains('\0'));
        // Sanitiser should not produce an empty result for a non-empty input.
        Assert.IsTrue(sanitised.Length > 0);
    }
}
