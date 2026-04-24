using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services.AuditLog;
using Remotely.Shared.Services;
using System;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

/// <summary>
/// Tests for <see cref="AuditLogService"/> — the hash chain is the
/// whole point of this service so the tests are heavy on
/// "tamper this row, prove the chain notices".
/// </summary>
[TestClass]
public class AuditLogServiceTests
{
    private TestData _testData = null!;
    private IAppDbFactory _dbFactory = null!;
    private SystemTime _systemTime = null!;
    private AuditLogService _service = null!;

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();
        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _systemTime = new SystemTime();
        _systemTime.Set(new DateTimeOffset(2026, 4, 24, 0, 0, 0, TimeSpan.Zero));
        _service = new AuditLogService(
            _dbFactory, _systemTime,
            NullLogger<AuditLogService>.Instance);
    }

    [TestMethod]
    public async Task Append_FirstEntry_HasGenesisPrevHash()
    {
        var entry = await _service.AppendAsync(
            _testData.Org1Id, "auth.login.success", "user-1", "user-1", "User logged in.");
        Assert.AreEqual(1, entry.Sequence);
        Assert.AreEqual(AuditLogService.GenesisPrevHash, entry.PrevHash);
        Assert.AreEqual(64, entry.EntryHash.Length);
        Assert.AreNotEqual(AuditLogService.GenesisPrevHash, entry.EntryHash);
    }

    [TestMethod]
    public async Task Append_ChainsRowsByPrevHash()
    {
        var first = await _service.AppendAsync(
            _testData.Org1Id, "evt.a", "actor", "subj", "first");
        var second = await _service.AppendAsync(
            _testData.Org1Id, "evt.b", "actor", "subj", "second");
        var third = await _service.AppendAsync(
            _testData.Org1Id, "evt.c", "actor", "subj", "third");

        Assert.AreEqual(2, second.Sequence);
        Assert.AreEqual(3, third.Sequence);
        Assert.AreEqual(first.EntryHash, second.PrevHash);
        Assert.AreEqual(second.EntryHash, third.PrevHash);
    }

    [TestMethod]
    public async Task Append_PerOrgChainsAreIndependent()
    {
        await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "org1");
        var org2First = await _service.AppendAsync(
            _testData.Org2Admin1.OrganizationID, "evt", "a", "s", "org2");
        Assert.AreEqual(1, org2First.Sequence,
            "Org2's chain must restart at sequence 1 — chains are per-org.");
        Assert.AreEqual(AuditLogService.GenesisPrevHash, org2First.PrevHash);
    }

    [TestMethod]
    public async Task VerifyChain_HappyPath_ReturnsNull()
    {
        await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row1");
        await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row2");
        await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row3");

        Assert.IsNull(await _service.VerifyChainAsync(_testData.Org1Id, CancellationToken.None));
    }

    [TestMethod]
    public async Task VerifyChain_TamperedSummary_DetectsRow()
    {
        await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row1");
        var second = await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row2");
        await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row3");

        // Tamper with row 2's summary in-place. The recorded EntryHash
        // is now over the *old* summary and recomputation will detect
        // the mismatch.
        using (var db = _dbFactory.GetContext())
        {
            var row = await db.AuditLogEntries.FirstAsync(e => e.Id == second.Id);
            row.Summary = "tampered";
            await db.SaveChangesAsync();
        }

        var bad = await _service.VerifyChainAsync(_testData.Org1Id, CancellationToken.None);
        Assert.AreEqual(second.Sequence, bad);
    }

    [TestMethod]
    public async Task VerifyChain_BrokenLink_DetectsRow()
    {
        var first = await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row1");
        var second = await _service.AppendAsync(_testData.Org1Id, "evt", "a", "s", "row2");

        // Rewrite row 2's PrevHash so the link to row 1 is broken,
        // *and* re-derive its EntryHash from the broken body so the
        // self-hash check passes — the only thing wrong is the chain
        // link. This proves the link check is independent of the
        // self-hash check.
        using (var db = _dbFactory.GetContext())
        {
            var row = await db.AuditLogEntries.FirstAsync(e => e.Id == second.Id);
            row.PrevHash = new string('1', 64);
            row.EntryHash = AuditLogService.ComputeEntryHash(row);
            await db.SaveChangesAsync();
        }

        var bad = await _service.VerifyChainAsync(_testData.Org1Id, CancellationToken.None);
        Assert.AreEqual(second.Sequence, bad);
    }

    [TestMethod]
    public async Task Append_DetailJson_IsCanonicalized()
    {
        // Two equivalent objects with different key order must produce
        // the same DetailJson — the chain is sensitive to byte order
        // and we don't want a trivial reordering to change the hash.
        var a = await _service.AppendAsync(
            _testData.Org1Id, "evt", "a", "s", "row",
            detail: new { Beta = 2, Alpha = 1 });
        var b = await _service.AppendAsync(
            _testData.Org1Id, "evt", "a", "s", "row",
            detail: new { Alpha = 1, Beta = 2 });
        Assert.AreEqual(a.DetailJson, b.DetailJson);
    }

    [TestMethod]
    public async Task Append_RejectsEmptyOrg()
    {
        await Assert.ThrowsExceptionAsync<ArgumentException>(async () =>
            await _service.AppendAsync("", "evt", "a", "s", "summary"));
    }

    [TestMethod]
    public async Task Append_RejectsEmptyEventType()
    {
        await Assert.ThrowsExceptionAsync<ArgumentException>(async () =>
            await _service.AppendAsync(_testData.Org1Id, "", "a", "s", "summary"));
    }
}
