using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Shared.Dtos;
using Remotely.Shared.Enums;

namespace Remotely.Shared.Tests;

/// <summary>
/// Pins the .NET PascalCase JSON wire form for the WebRTC signalling
/// (slice R7.g) + ICE / TURN server-config (R7.i) + ProvideIceServers
/// hub-method (R7.j) DTOs against the frozen Rust vectors under
/// <c>docs/wire-protocol-vectors/method-surface/</c>. Every change to
/// either side breaks one of these tests on purpose.
/// </summary>
[TestClass]
public class WebRtcSignallingDtoTests
{
    private static readonly JsonSerializerOptions SerializeOptions = new()
    {
        WriteIndented = true,
    };

    private static string VectorsRoot()
    {
        var dir = AppContext.BaseDirectory;
        for (var i = 0; i < 12 && dir is not null; i++)
        {
            var candidate = Path.Combine(dir, "docs", "wire-protocol-vectors", "method-surface");
            if (Directory.Exists(candidate))
            {
                return candidate;
            }
            dir = Path.GetDirectoryName(dir);
        }

        throw new DirectoryNotFoundException(
            "Could not locate docs/wire-protocol-vectors/method-surface/ from " +
            AppContext.BaseDirectory);
    }

    private static string ReadVector(params string[] segments)
    {
        var path = Path.Combine(new[] { VectorsRoot() }.Concat(segments).ToArray());
        return File.ReadAllText(path);
    }

    /// <summary>
    /// Asserts that round-tripping <paramref name="vectorJson"/> through
    /// <typeparamref name="T"/> produces JSON whose property tree is
    /// identical to the original — i.e. no missing fields, no extra
    /// fields, no renamed keys, no enum form drift.
    /// </summary>
    private static T RoundTrip<T>(string vectorJson) where T : notnull
    {
        var dto = JsonSerializer.Deserialize<T>(vectorJson)!;
        Assert.IsNotNull(dto, "Vector failed to deserialise into {0}", typeof(T).Name);

        var reSerialised = JsonSerializer.Serialize(dto, SerializeOptions);

        var original = JsonNode.Parse(vectorJson)!;
        var actual = JsonNode.Parse(reSerialised)!;
        AssertJsonNodesEqual(original, actual, typeof(T).Name);

        return dto;
    }

    private static void AssertJsonNodesEqual(JsonNode? expected, JsonNode? actual, string path)
    {
        if (expected is null && actual is null)
        {
            return;
        }

        Assert.IsNotNull(expected, "Expected null mismatch at {0}", path);
        Assert.IsNotNull(actual, "Actual null mismatch at {0}", path);

        switch (expected)
        {
            case JsonObject expectedObj:
                {
                    var actualObj = actual as JsonObject;
                    Assert.IsNotNull(actualObj, "Expected object at {0}, got {1}", path, actual!.GetType().Name);

                    var expectedKeys = expectedObj.Select(p => p.Key).OrderBy(x => x).ToArray();
                    var actualKeys = actualObj.Select(p => p.Key).OrderBy(x => x).ToArray();
                    CollectionAssert.AreEqual(
                        expectedKeys,
                        actualKeys,
                        $"Property set mismatch at {path}: expected [{string.Join(",", expectedKeys)}] " +
                        $"actual [{string.Join(",", actualKeys)}]");

                    foreach (var key in expectedKeys)
                    {
                        AssertJsonNodesEqual(expectedObj[key], actualObj[key], $"{path}.{key}");
                    }

                    break;
                }
            case JsonArray expectedArr:
                {
                    var actualArr = actual as JsonArray;
                    Assert.IsNotNull(actualArr, "Expected array at {0}", path);
                    Assert.AreEqual(expectedArr.Count, actualArr.Count, "Array length mismatch at {0}", path);
                    for (var i = 0; i < expectedArr.Count; i++)
                    {
                        AssertJsonNodesEqual(expectedArr[i], actualArr[i], $"{path}[{i}]");
                    }

                    break;
                }
            case JsonValue:
                {
                    Assert.AreEqual(
                        expected.ToJsonString(),
                        actual.ToJsonString(),
                        "Value mismatch at {0}", path);
                    break;
                }
            default:
                Assert.Fail("Unexpected JsonNode type at {0}: {1}", path, expected.GetType().Name);
                break;
        }
    }

    // --------------------------------------------------------------------
    // Slice R7.g — SDP offer / answer / ICE candidate.
    // --------------------------------------------------------------------

    [TestMethod]
    public void SdpOffer_RoundTripsFrozenVectorByteForByte()
    {
        var dto = RoundTrip<SdpOfferDto>(ReadVector("signalling", "sdp-offer.json"));

        Assert.AreEqual("viewer-conn-1", dto.ViewerConnectionId);
        Assert.AreEqual("11111111-2222-3333-4444-555555555555", dto.SessionId);
        Assert.AreEqual("Alice Operator", dto.RequesterName);
        Assert.AreEqual("Acme Corp", dto.OrgName);
        Assert.AreEqual("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", dto.OrgId);
        Assert.AreEqual(SdpKind.Offer, dto.Kind);
        StringAssert.StartsWith(dto.Sdp, "v=0\r\n");
        StringAssert.Contains(dto.Sdp, "a=rtpmap:96 H264/90000");
    }

    [TestMethod]
    public void SdpAnswer_RoundTripsFrozenVectorByteForByte()
    {
        var dto = RoundTrip<SdpAnswerDto>(ReadVector("signalling", "sdp-answer.json"));

        Assert.AreEqual("viewer-conn-1", dto.ViewerConnectionId);
        Assert.AreEqual(SdpKind.Answer, dto.Kind);
        StringAssert.StartsWith(dto.Sdp, "v=0\r\n");
        StringAssert.Contains(dto.Sdp, "a=setup:active");
    }

    [TestMethod]
    public void IceCandidate_RoundTripsFrozenVectorByteForByte()
    {
        var dto = RoundTrip<IceCandidateDto>(ReadVector("signalling", "ice-candidate.json"));

        Assert.AreEqual("candidate:1 1 UDP 2130706431 192.0.2.1 12345 typ host", dto.Candidate);
        Assert.AreEqual("0", dto.SdpMid);
        Assert.AreEqual((ushort)0, dto.SdpMlineIndex);
    }

    [TestMethod]
    public void IceCandidate_EndOfCandidatesMarker_RoundTripsByteForByte()
    {
        // The RFC 8838 end-of-candidates marker uses an empty Candidate
        // string with both SdpMid and SdpMlineIndex serialised as JSON
        // null — the wire form must preserve those nulls so the agent
        // can distinguish "I have no more candidates" from "I have a
        // candidate without a mid".
        var dto = RoundTrip<IceCandidateDto>(
            ReadVector("signalling", "ice-candidate-end-of-candidates.json"));

        Assert.AreEqual(string.Empty, dto.Candidate);
        Assert.IsNull(dto.SdpMid);
        Assert.IsNull(dto.SdpMlineIndex);
    }

    [TestMethod]
    public void DesktopTransportResult_SuccessVector_RoundTripsByteForByte()
    {
        var dto = RoundTrip<DesktopTransportResultDto>(
            ReadVector("signalling", "result-success.json"));

        Assert.AreEqual("11111111-2222-3333-4444-555555555555", dto.SessionId);
        Assert.IsTrue(dto.Success);
        Assert.IsNull(dto.ErrorMessage);
    }

    [TestMethod]
    public void DesktopTransportResult_FailureVector_RoundTripsByteForByte()
    {
        var dto = RoundTrip<DesktopTransportResultDto>(
            ReadVector("signalling", "result-failure.json"));

        Assert.IsFalse(dto.Success);
        Assert.AreEqual(
            "Desktop transport for \"SendSdpOffer\" is not supported on Linux.",
            dto.ErrorMessage);
    }

    // --------------------------------------------------------------------
    // Slice R7.i — ICE / TURN server configuration.
    // --------------------------------------------------------------------

    [TestMethod]
    public void IceServerConfig_StunPlusTurnVector_RoundTripsByteForByte()
    {
        var dto = RoundTrip<IceServerConfigDto>(
            ReadVector("ice-config", "ice-server-config.json"));

        Assert.AreEqual(2, dto.IceServers.Count);
        Assert.AreEqual(IceTransportPolicy.All, dto.IceTransportPolicy);

        var stun = dto.IceServers[0];
        CollectionAssert.AreEqual(new[] { "stun:stun.example.org:3478" }, stun.Urls);
        Assert.IsNull(stun.Username);
        Assert.IsNull(stun.Credential);
        Assert.AreEqual(IceCredentialType.Password, stun.CredentialType);

        var turn = dto.IceServers[1];
        Assert.AreEqual(2, turn.Urls.Count);
        Assert.AreEqual("agent-bob", turn.Username);
        Assert.AreEqual("hunter2", turn.Credential);
        Assert.AreEqual(IceCredentialType.Password, turn.CredentialType);
    }

    [TestMethod]
    public void IceServerConfig_RelayOnlyVector_RoundTripsByteForByte()
    {
        var dto = RoundTrip<IceServerConfigDto>(
            ReadVector("ice-config", "ice-server-config-relay-only.json"));

        Assert.AreEqual(1, dto.IceServers.Count);
        Assert.AreEqual(IceTransportPolicy.Relay, dto.IceTransportPolicy);
        CollectionAssert.AreEqual(
            new[] { "turns:relay.example.org:5349?transport=tcp" },
            dto.IceServers[0].Urls);
    }

    // --------------------------------------------------------------------
    // Slice R7.j — ProvideIceServers hub-method request.
    // --------------------------------------------------------------------

    [TestMethod]
    public void ProvideIceServersRequest_RoundTripsFrozenVectorByteForByte()
    {
        var dto = RoundTrip<ProvideIceServersRequestDto>(
            ReadVector("provide-ice-servers", "request.json"));

        Assert.AreEqual("viewer-conn-1", dto.ViewerConnectionId);
        Assert.AreEqual("11111111-2222-3333-4444-555555555555", dto.SessionId);
        Assert.AreEqual("REDACTED-IN-LOGS", dto.AccessKey);
        Assert.AreEqual("Alice Operator", dto.RequesterName);
        Assert.AreEqual("Acme Corp", dto.OrgName);
        Assert.AreEqual("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", dto.OrgId);

        Assert.AreEqual(2, dto.IceServerConfig.IceServers.Count);
        Assert.AreEqual(IceTransportPolicy.All, dto.IceServerConfig.IceTransportPolicy);
        Assert.AreEqual("agent-bob", dto.IceServerConfig.IceServers[1].Username);
        Assert.AreEqual("REDACTED-IN-LOGS", dto.IceServerConfig.IceServers[1].Credential);
    }

    [TestMethod]
    public void ProvideIceServersRequest_FailureResult_RoundTripsByteForByte()
    {
        var dto = RoundTrip<DesktopTransportResultDto>(
            ReadVector("provide-ice-servers", "result-failure.json"));

        Assert.AreEqual("11111111-2222-3333-4444-555555555555", dto.SessionId);
        Assert.IsFalse(dto.Success);
        Assert.AreEqual(
            "Desktop transport for \"ProvideIceServers\" is not supported on Linux.",
            dto.ErrorMessage);
    }

    // --------------------------------------------------------------------
    // Constants — pin the .NET-side cap mirrors against the documented
    // Rust values so a future drift on either side breaks loudly here.
    // --------------------------------------------------------------------

    [TestMethod]
    public void WireConstants_MatchRustCaps()
    {
        Assert.AreEqual(16 * 1024, SdpOfferDto.MaxSdpBytes, "MAX_SDP_BYTES drift");
        Assert.AreEqual(1024, SdpOfferDto.MaxSignallingStringLen, "MAX_SIGNALLING_STRING_LEN drift");
        Assert.AreEqual(8, IceServerConfigDto.MaxIceServers, "MAX_ICE_SERVERS drift");
        Assert.AreEqual(4, IceServerDto.MaxUrlsPerIceServer, "MAX_URLS_PER_ICE_SERVER drift");
        Assert.AreEqual(512, IceServerDto.MaxIceUrlLen, "MAX_ICE_URL_LEN drift");
        Assert.AreEqual(512, IceServerDto.MaxIceCredentialLen, "MAX_ICE_CREDENTIAL_LEN drift");
    }

    // --------------------------------------------------------------------
    // Defaults — fail-closed contract on every DTO.
    // --------------------------------------------------------------------

    [TestMethod]
    public void DesktopTransportResult_Default_FailsClosed()
    {
        var dto = new DesktopTransportResultDto();
        Assert.IsFalse(dto.Success, "Success must default to false so a partial payload fails closed");
        Assert.IsNull(dto.ErrorMessage);
    }

    [TestMethod]
    public void SdpOffer_Default_KindIsOffer()
    {
        Assert.AreEqual(SdpKind.Offer, new SdpOfferDto().Kind);
    }

    [TestMethod]
    public void SdpAnswer_Default_KindIsAnswer()
    {
        Assert.AreEqual(SdpKind.Answer, new SdpAnswerDto().Kind);
    }

    [TestMethod]
    public void IceServer_Default_CredentialTypeIsPassword()
    {
        Assert.AreEqual(IceCredentialType.Password, new IceServerDto().CredentialType);
    }

    [TestMethod]
    public void IceServerConfig_Default_TransportPolicyIsAll()
    {
        Assert.AreEqual(IceTransportPolicy.All, new IceServerConfigDto().IceTransportPolicy);
    }

    // --------------------------------------------------------------------
    // Enum on-the-wire form — string discriminants matching the Rust
    // serde "PascalCase" rename rule. Asserted via JSON round-trip so a
    // drift to integer encoding (System.Text.Json default) breaks here.
    // --------------------------------------------------------------------

    [TestMethod]
    public void Enums_SerialiseAsPascalCaseStrings()
    {
        var server = new IceServerDto
        {
            Urls = new List<string> { "stun:stun.example.org:3478" },
            CredentialType = IceCredentialType.Password,
        };
        var config = new IceServerConfigDto
        {
            IceServers = new List<IceServerDto> { server },
            IceTransportPolicy = IceTransportPolicy.Relay,
        };

        var json = JsonSerializer.Serialize(config);
        StringAssert.Contains(json, "\"CredentialType\":\"Password\"");
        StringAssert.Contains(json, "\"IceTransportPolicy\":\"Relay\"");

        var offer = new SdpOfferDto { Kind = SdpKind.Offer };
        StringAssert.Contains(JsonSerializer.Serialize(offer), "\"Kind\":\"Offer\"");

        var answer = new SdpAnswerDto { Kind = SdpKind.Answer };
        StringAssert.Contains(JsonSerializer.Serialize(answer), "\"Kind\":\"Answer\"");
    }
}
