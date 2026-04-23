using Microsoft.AspNetCore.Components.Forms;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Remotely.Server.Data;
using Remotely.Server.Services;
using Remotely.Shared.Enums;
using System;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace Remotely.Server.Tests;

[TestClass]
public class UploadedMsiServiceTests
{
    private TestData _testData = null!;
    private UploadedMsiService _service = null!;
    private IDataService _dataService = null!;
    private IAppDbFactory _dbFactory = null!;

    private static readonly byte[] Ole2Magic = new byte[]
    {
        0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1
    };

    [TestInitialize]
    public async Task Init()
    {
        _testData = new TestData();
        await _testData.Init();

        _dbFactory = IoCActivator.ServiceProvider.GetRequiredService<IAppDbFactory>();
        _dataService = IoCActivator.ServiceProvider.GetRequiredService<IDataService>();
        _service = new UploadedMsiService(_dbFactory, _dataService, NullLogger<UploadedMsiService>.Instance);
    }

    [TestMethod]
    public async Task UploadAsync_AcceptsValidMsi()
    {
        var bytes = Ole2Magic.Concat(new byte[2048]).ToArray();
        var file = new FakeBrowserFile("setup.msi", bytes);

        var result = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Chrome", file);

        Assert.IsTrue(result.IsSuccess, $"Expected success, got: {result.Outcome} / {result.Message}");
        Assert.IsNotNull(result.Value);
        Assert.AreEqual("Chrome", result.Value!.Name);
        Assert.AreEqual(bytes.Length, result.Value.SizeBytes);
        Assert.AreEqual(64, result.Value.Sha256.Length);
    }

    [TestMethod]
    public async Task UploadAsync_RejectsNonMsiBytes()
    {
        // No magic prefix — must be refused regardless of declared filename.
        var bytes = new byte[] { 0x50, 0x4B, 0x03, 0x04, 0, 0, 0, 0, 1, 2, 3, 4 };
        var file = new FakeBrowserFile("evil.msi", bytes);

        var result = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Evil", file);

        Assert.IsFalse(result.IsSuccess);
        Assert.AreEqual(UploadMsiOutcome.NotAnMsi, result.Outcome);
    }

    [TestMethod]
    public async Task UploadAsync_RejectsEmptyFile()
    {
        var file = new FakeBrowserFile("empty.msi", Array.Empty<byte>());
        var result = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Empty", file);

        Assert.IsFalse(result.IsSuccess);
        Assert.AreEqual(UploadMsiOutcome.EmptyFile, result.Outcome);
    }

    [TestMethod]
    public async Task UploadAsync_RejectsBlankName()
    {
        var bytes = Ole2Magic.Concat(new byte[256]).ToArray();
        var file = new FakeBrowserFile("setup.msi", bytes);
        var result = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "  ", file);

        Assert.IsFalse(result.IsSuccess);
        Assert.AreEqual(UploadMsiOutcome.InvalidArgs, result.Outcome);
    }

    [TestMethod]
    public async Task UploadAsync_DedupesByShaWithinOrg()
    {
        var bytes = Ole2Magic.Concat(new byte[1024]).ToArray();

        var first = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "First",
            new FakeBrowserFile("a.msi", bytes));
        Assert.IsTrue(first.IsSuccess);

        var second = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Second",
            new FakeBrowserFile("b.msi", bytes));
        Assert.IsFalse(second.IsSuccess);
        Assert.AreEqual(UploadMsiOutcome.DuplicateSha256, second.Outcome);
    }

    [TestMethod]
    public async Task UploadAsync_AllowsSameShaInDifferentOrgs()
    {
        // Dedupe is org-scoped — Org2 may legitimately upload the same MSI.
        var bytes = Ole2Magic.Concat(new byte[512]).ToArray();
        var first = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Org1Copy",
            new FakeBrowserFile("c.msi", bytes));
        Assert.IsTrue(first.IsSuccess);

        var second = await _service.UploadAsync(_testData.Org2Id, _testData.Org2Admin1.Id, "Org2Copy",
            new FakeBrowserFile("c.msi", bytes));
        Assert.IsTrue(second.IsSuccess);
    }

    [TestMethod]
    public async Task GetForOrgAsync_IsOrgScoped()
    {
        var bytes = Ole2Magic.Concat(new byte[64]).ToArray();
        await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "OnlyForOrg1",
            new FakeBrowserFile("x.msi", bytes));

        var org1 = await _service.GetForOrgAsync(_testData.Org1Id);
        var org2 = await _service.GetForOrgAsync(_testData.Org2Id);
        Assert.AreEqual(1, org1.Count);
        Assert.AreEqual(0, org2.Count);
    }

    [TestMethod]
    public async Task TombstoneAsync_HidesRowFromDefaultListing()
    {
        var bytes = Ole2Magic.Concat(new byte[64]).ToArray();
        var upload = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "ToDelete",
            new FakeBrowserFile("d.msi", bytes));
        Assert.IsTrue(upload.IsSuccess);
        var id = upload.Value!.Id;

        var ok = await _service.TombstoneAsync(_testData.Org1Id, id);
        Assert.IsTrue(ok);

        var listed = await _service.GetForOrgAsync(_testData.Org1Id);
        Assert.IsFalse(listed.Any(x => x.Id == id));

        var all = await _service.GetForOrgAsync(_testData.Org1Id, includeTombstoned: true);
        Assert.IsTrue(all.Any(x => x.Id == id && x.IsTombstoned));
    }

    [TestMethod]
    public async Task TombstoneAsync_RejectsCrossOrgDelete()
    {
        // An admin in Org2 must NOT be able to delete an Org1 row by GUID.
        var bytes = Ole2Magic.Concat(new byte[64]).ToArray();
        var upload = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Org1Only",
            new FakeBrowserFile("e.msi", bytes));
        Assert.IsTrue(upload.IsSuccess);

        var ok = await _service.TombstoneAsync(_testData.Org2Id, upload.Value!.Id);
        Assert.IsFalse(ok, "Cross-org delete must be refused.");

        // Original row is still alive in Org1.
        var listed = await _service.GetForOrgAsync(_testData.Org1Id);
        Assert.IsTrue(listed.Any(x => x.Id == upload.Value.Id));
    }

    [TestMethod]
    public async Task PurgeTombstonedAsync_DropsRowAndSharedFile()
    {
        var bytes = Ole2Magic.Concat(new byte[64]).ToArray();
        var upload = await _service.UploadAsync(_testData.Org1Id, _testData.Org1Admin1.Id, "Purge",
            new FakeBrowserFile("p.msi", bytes));
        Assert.IsTrue(upload.IsSuccess);
        var sharedFileId = upload.Value!.SharedFileId;

        Assert.IsTrue(await _service.TombstoneAsync(_testData.Org1Id, upload.Value.Id));

        var purged = await _service.PurgeTombstonedAsync();
        Assert.AreEqual(1, purged);

        // Both the row and the underlying SharedFile must be gone.
        using var db = _dbFactory.GetContext();
        Assert.IsFalse(db.UploadedMsis.Any(x => x.Id == upload.Value.Id));
        Assert.IsFalse(db.SharedFiles.Any(x => x.ID == sharedFileId));
    }

    /// <summary>
    /// Minimal in-memory <see cref="IBrowserFile"/> just for these tests.
    /// Returning a fresh stream from <c>OpenReadStream</c> matches the
    /// real Blazor contract (the stream is forward-only / single-use).
    /// </summary>
    private sealed class FakeBrowserFile : IBrowserFile
    {
        private readonly byte[] _bytes;

        public FakeBrowserFile(string name, byte[] bytes)
        {
            Name = name;
            _bytes = bytes;
            LastModified = DateTimeOffset.UtcNow;
            ContentType = "application/x-msi";
        }

        public string Name { get; }
        public DateTimeOffset LastModified { get; }
        public long Size => _bytes.LongLength;
        public string ContentType { get; }

        public Stream OpenReadStream(long maxAllowedSize = 512000, CancellationToken cancellationToken = default)
        {
            if (_bytes.LongLength > maxAllowedSize)
            {
                throw new IOException("Supplied size exceeds maxAllowedSize.");
            }
            return new MemoryStream(_bytes, writable: false);
        }
    }
}
