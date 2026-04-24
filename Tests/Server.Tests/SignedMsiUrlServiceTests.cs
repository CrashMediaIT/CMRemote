using Microsoft.AspNetCore.DataProtection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Services;
using Remotely.Shared.Services;
using System;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests for <see cref="SignedMsiUrlService"/> — pins the
/// device + file binding, TTL, and tamper-detection contracts that
/// the S7 signed-MSI-URL design depends on.
/// </summary>
[TestClass]
public class SignedMsiUrlServiceTests
{
    private SystemTime _systemTime = null!;
    private SignedMsiUrlService _service = null!;

    [TestInitialize]
    public void Init()
    {
        _systemTime = new SystemTime();
        _systemTime.Set(new DateTimeOffset(2026, 4, 24, 12, 0, 0, TimeSpan.Zero));

        // The ephemeral provider uses an in-memory key ring so each
        // test has an isolated key set; tokens minted in one test
        // can't be replayed in another.
        var provider = DataProtectionProvider.Create("CMRemote.Tests.SignedMsi");
        _service = new SignedMsiUrlService(
            provider, _systemTime,
            NullLogger<SignedMsiUrlService>.Instance);
    }

    [TestMethod]
    public void Validate_HappyPath_ReturnsPayload()
    {
        var token = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        var payload = _service.Validate(token, "file-1");
        Assert.IsNotNull(payload);
        Assert.AreEqual("device-1", payload!.DeviceId);
        Assert.AreEqual("file-1", payload.SharedFileId);
    }

    [TestMethod]
    public void Validate_WrongFileId_ReturnsNull()
    {
        var token = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        Assert.IsNull(_service.Validate(token, "file-2"));
    }

    [TestMethod]
    public void Validate_Expired_ReturnsNull()
    {
        var token = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        _systemTime.Offset(TimeSpan.FromMinutes(10));
        Assert.IsNull(_service.Validate(token, "file-1"));
    }

    [TestMethod]
    public void Validate_TamperedToken_ReturnsNull()
    {
        var token = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        // Flip a single character in the middle of the token — the
        // protector's MAC must reject the tampered envelope.
        var midpoint = token.Length / 2;
        var tampered = token.Substring(0, midpoint) +
            (token[midpoint] == 'a' ? 'b' : 'a') +
            token.Substring(midpoint + 1);
        Assert.IsNull(_service.Validate(tampered, "file-1"));
    }

    [TestMethod]
    public void Validate_MalformedToken_ReturnsNull()
    {
        Assert.IsNull(_service.Validate("not-base64-!!!", "file-1"));
    }

    [TestMethod]
    public void Validate_EmptyInputs_ReturnNull()
    {
        Assert.IsNull(_service.Validate("", "file-1"));
        var token = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        Assert.IsNull(_service.Validate(token, ""));
    }

    [TestMethod]
    public void MintToken_TwoMintsAreDifferent()
    {
        // The DataProtector envelope binds a fresh nonce per-call so
        // two mints of the same payload produce different ciphertext —
        // an attacker can't tell two identical permissions apart.
        var t1 = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        var t2 = _service.MintToken("device-1", "file-1", _systemTime.Now.AddMinutes(5));
        Assert.AreNotEqual(t1, t2);
    }

    [TestMethod]
    public void MintToken_RejectsEmptyDeviceId()
    {
        Assert.ThrowsException<ArgumentException>(() =>
            _service.MintToken("", "file-1", _systemTime.Now.AddMinutes(5)));
    }

    [TestMethod]
    public void MintToken_RejectsEmptyFileId()
    {
        Assert.ThrowsException<ArgumentException>(() =>
            _service.MintToken("device-1", "", _systemTime.Now.AddMinutes(5)));
    }
}
