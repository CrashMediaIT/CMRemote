# CMRemote Publisher Manifest

> **Status:** Slice R8 — *frozen for the Linux `.deb` / `.rpm` channel*.
> This document is the authoritative specification of the publisher
> manifest the CMRemote release process emits and that the M3
> agent-upgrade dispatcher (`Server.Services.AgentUpgrade.ManifestBackedAgentUpgradeDispatcher`)
> consumes. Both producer and consumer conform to it. When an
> implementation and this document disagree, the document wins.
>
> This is a clean-room specification — no upstream source was consulted
> while writing it.
>
> **Companion samples:** [`publisher-manifest-samples/`](./publisher-manifest-samples/).
> **Companion JSON schema:** [`publisher-manifest.schema.json`](./publisher-manifest.schema.json).

## Goals

1. Give the M3 dispatcher a stable, signed contract for "the latest
   build per platform per channel" so the Rust agent, the .NET agent,
   and the package-manager fetch handlers (slice R6) all bind against
   one shape.
2. Make the format trivial to verify offline — every entry carries a
   SHA-256 over the artifact bytes and a path to a Sigstore cosign
   bundle so a relying party never has to trust the download URL on
   its own.
3. Allow versioned, backwards-compatible evolution: every manifest
   carries a `schemaVersion` integer and the consumer is required to
   reject unknown major versions with a structured error rather than
   silently misbehaving.
4. Be small and rsync-friendly — a single manifest fits in one HTTP
   response and is the only document a relying party fetches before
   the artifact itself.

## Non-goals

- The manifest does not pin the agent's wire-protocol version; that is
  pinned by `docs/wire-protocol.md` and re-derived per release. A
  manifest entry only commits the publisher to "this build runs the
  CMRemote agent at this version".
- The manifest does not list every historical build. Only the current
  promotion targets per channel are listed; rollback uses `previous`
  channel or an out-of-band manifest URL.

## Top-level shape

A publisher manifest is a single UTF-8 JSON document. Top-level keys
(case-sensitive, camelCase to match `docs/wire-protocol.md`):

| Field | Type | Required | Notes |
|---|---|---|---|
| `schemaVersion` | integer | yes | Major version of this spec. The current value is `1`. Consumers MUST refuse a manifest whose major version they do not recognise. |
| `publisher` | string | yes | Free-form identifier for the publisher (e.g. `"crashmedia.ca"`). Surfaced in the M4 dashboard. Not used for trust decisions — those are gated by the cosign signature. |
| `generatedAt` | string (RFC 3339 UTC) | yes | When the manifest was emitted. Surfaced in the M4 dashboard. |
| `channel` | string | yes | One of `"stable"`, `"preview"`, or `"previous"`. The dispatcher selects the channel per device based on the `agent-channel` device setting (see ROADMAP.md slice R0). |
| `version` | string (SemVer 2.0.0) | yes | The agent version this manifest promotes. Every entry under `builds` MUST have this same `agentVersion`. |
| `builds` | array of [BuildEntry](#buildentry) | yes | One entry per supported `(target, format)` tuple. May be empty (signals "channel exists but currently empty"; the dispatcher leaves rows `Pending`). |
| `notes` | string | no | Free-form release notes. Surfaced in the M4 dashboard. Not used for any trust or routing decision. |

## BuildEntry

| Field | Type | Required | Notes |
|---|---|---|---|
| `agentVersion` | string (SemVer 2.0.0) | yes | MUST equal the manifest's top-level `version`. Repeated per entry so a relying party that splits builds across files can still trust each entry standalone. |
| `target` | string | yes | Rust-style target triple, e.g. `"x86_64-unknown-linux-gnu"`, `"aarch64-unknown-linux-gnu"`, `"x86_64-pc-windows-msvc"`, `"x86_64-apple-darwin"`, `"aarch64-apple-darwin"`. The dispatcher matches this against the device's reported `RustTarget` field in `DeviceSnapshot`. Unknown targets are skipped. |
| `format` | string | yes | One of `"deb"`, `"rpm"`, `"msi"`, `"pkg"`, `"tar.gz"`. The dispatcher matches this against the device's reported `PackageFormat`. The `tar.gz` format is reserved for the unattended-installer path the agent itself drives during a self-upgrade. |
| `file` | string | yes | File name of the artifact (no path components — the dispatcher resolves it relative to the manifest's URL). MUST match the regex `^[A-Za-z0-9._-]+$` and MUST NOT contain `..`. |
| `size` | integer | yes | Size of the artifact in bytes. Consumers SHOULD compare this against the bytes they actually downloaded before computing the SHA-256 — a mismatch is a hard failure. |
| `sha256` | string | yes | Lower-case hex SHA-256 over the artifact bytes. Consumers MUST refuse to install a build whose computed SHA-256 does not match this value. Constant-time comparison is recommended. |
| `signature` | string | no | Path (relative to the manifest URL) to a Sigstore cosign bundle (`.sig` + Rekor entry) over the artifact. When the consumer is configured to require cosign verification (`AgentUpgrade:RequireSignature=true`, default `false` until agent-side certificate verification lands), an entry without a `signature` is treated as unavailable. |
| `signedBy` | string | no | Cosign certificate identity expected on the bundle (e.g. `"https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/v2.0.0"`). Required when `signature` is present. |

## Trust rules

A consumer (M3 dispatcher, R6 fetch handler, or the agent's own
self-upgrade flow) MUST:

1. Refuse a manifest whose `schemaVersion` major number it does not
   recognise.
2. Refuse a manifest entry whose `agentVersion` does not equal the
   top-level `version`.
3. Refuse a manifest entry whose `file` field contains a path
   separator (`/`, `\`) or a `..` segment.
4. After downloading the artifact: compare the byte length against
   `size` (hard failure on mismatch) and the SHA-256 against `sha256`
   using a constant-time comparison (hard failure on mismatch).
5. When configured to require signatures (S5): verify the cosign
   bundle named by `signature` against the certificate identity
   named by `signedBy`, using the publisher's pinned Rekor public key.

A consumer MUST NOT:

- Trust the `publisher` or `notes` fields for any routing or trust
  decision.
- Treat a manifest with no matching `(target, format)` entry as a
  failure — that case is "no upgrade currently published for this
  device" and leaves the M3 row in `Pending`.

## Routing

Given a device with `RustTarget=T`, `PackageFormat=F`, and
`AgentChannel=C`:

1. Resolve the manifest URL for channel `C` (the dispatcher's
   `AgentUpgrade:ManifestUrls:<channel>` configuration). If no URL is
   configured for the channel, return *no target available*.
2. Fetch and parse the manifest. Apply the trust rules above.
3. Find the unique entry whose `target == T` and `format == F`. If
   none, return *no target available*. If more than one, return *no
   target available* with a structured warning logged (the manifest
   is malformed).
4. If `version` equals the device's current `AgentVersion`, return
   *no target available* (already on target).
5. Return the resolved download URL (manifest URL with `file` joined
   onto the path), `sha256`, and `version`.

## Versioning

`schemaVersion` is a single integer. Increments are reserved for
**breaking** shape changes only — adding a new optional field or a
new enum value (e.g. a new `format`) does NOT bump it. Today
`schemaVersion = 1`.

## Vectors

Per-spec vectors live under [`publisher-manifest-samples/`](./publisher-manifest-samples/):

- `empty-channel.json` — a well-formed manifest with `builds: []`. The
  dispatcher must treat this as "no upgrade available", not as an
  error.
- `linux-stable.json` — a populated stable-channel manifest with one
  `x86_64-unknown-linux-gnu` `.deb` and one `x86_64-unknown-linux-gnu`
  `.rpm` entry, both pointing at the artifacts that
  `.github/workflows/release.yml` produces.

`Tests/Server.Tests/PublisherManifestTests.cs` and
`Tests/Server.Tests/ManifestBackedAgentUpgradeDispatcherTests.cs`
replay both vectors.

## Rust-agent install handoff

After the M3 dispatcher resolves a build, it invokes the Rust agent's
`InstallAgentUpdate(downloadUrl, version, sha256)` hub method. The agent:

1. downloads the artifact through the same R6 `ArtifactDownloader` used by
   package installs;
2. re-computes SHA-256 over the staged bytes and compares it to the manifest
   value in constant time;
3. stages the verified artifact under the agent update staging directory; and
4. invokes the native package installer with fixed argv slots selected only by
   artifact extension and host OS.

The current command mapping is:

| Host | Artifact | Command |
|---|---|---|
| Linux | `.deb` | `/usr/bin/dpkg -i <artifact>` |
| Linux | `.rpm` | `/usr/bin/rpm -Uvh <artifact>` |
| Windows | `.msi` | `%SystemRoot%\System32\msiexec.exe /i <artifact> /qn /norestart` |
| macOS | `.pkg` | `/usr/sbin/installer -pkg <artifact> -target /` |

Unsupported host/artifact combinations fail closed with a structured installer
error. The agent never shells out through a command string and removes the
staged artifact after the native installer exits.
