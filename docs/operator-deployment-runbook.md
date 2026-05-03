# CMRemote operator deployment runbook

CMRemote is still mid-rewrite. This runbook is the deployment gate for a
tagged pre-release: an operator should not promote a release candidate until
every applicable check below is complete and the evidence is attached to the
release notes or the deployment ticket.

## Scope

This runbook covers the final pre-release pass requested for the Rust-agent
cut-over:

- verify release assets end-to-end from a tagged draft release;
- publish and validate the publisher manifest used by server-side agent
  upgrade dispatch;
- exercise signed self-update on Linux, Windows, and macOS lab machines;
- record the minimum server, database, browser/WebRTC, and rollback evidence
  an operator needs before a controlled deployment.

It does not replace the first-boot setup wizard guide, the legacy migration
guide, or the publisher-manifest contract. Keep those documents open while
running this checklist:

- [Setup wizard](./Setup-Wizard.md)
- [Legacy migration](./Migration.md)
- [Publisher manifest](./publisher-manifest.md)
- [Wire protocol](./wire-protocol.md)

## Required lab inventory

Prepare a disposable lab before touching a customer deployment:

| Component | Minimum requirement | Evidence to keep |
|---|---|---|
| Server | Fresh CMRemote server at the release candidate commit | server commit SHA, configuration file, container/image digest if applicable |
| Database | PostgreSQL instance with backups enabled | backup ID before migration, `SELECT 1` setup evidence, migration report |
| Browser workstation | Current Chromium-family browser with WebRTC enabled | browser version, screen recording or screenshots of remote session |
| Linux agent | x86_64 lab VM with systemd | package manager logs, agent logs, `ConnectionInfo.json` permissions |
| Windows agent | Windows 10/11 or Server lab VM | MSI logs, service account, `ConnectionInfo.json` ACL evidence |
| macOS agent | Supported macOS lab host | installer logs, agent logs, notification evidence |
| Release signing | GitHub OIDC/cosign path for the tagged release | cosign verification output and certificate identity |

Use separate devices for "already enrolled" and "fresh enrolment" when
possible. The upgrade path and the first-install path exercise different
failure modes.

## Pre-release asset verification

Run this against the tagged draft release before publishing the manifest to a
server.

1. Confirm the tag points at the intended commit and is immutable for the
   deployment window.
2. Download the complete draft-release asset set into a clean directory.
3. Confirm the expected platform artifacts are present:
   - Linux: `.deb` and `.rpm`
   - Windows: `.msi`
   - macOS: `.pkg`
   - `publisher-manifest.json`
   - cosign bundles for every installable artifact
   - SBOMs and provenance attestations emitted by the release workflow
4. Recompute SHA-256 for every installable artifact and compare it with the
   corresponding `builds[].sha256` entry in `publisher-manifest.json`.
5. Validate `publisher-manifest.json` against
   `docs/publisher-manifest.schema.json`.
6. Verify every cosign bundle against the expected GitHub Actions identity:
   `https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/<tag>`.
7. Confirm every `builds[].signature` value is a basename, not a path, and
   that it resolves beside the manifest.
8. Confirm every `builds[].agentVersion` matches the manifest `version`.
9. Confirm each target/format pair is unique.
10. Save the manifest, SHA-256 output, and cosign output as deployment
    evidence.

Do not edit a generated manifest by hand. If any value is wrong, fix the
release workflow or release inputs and generate a new draft release.

## Server deployment gate

Before pointing devices at a new server build:

1. Restore the target database backup into a disposable PostgreSQL database.
2. Run the setup wizard or migration flow against the restored database.
3. Preserve `migration-report.json` and confirm fatal errors are zero.
4. Run the live PostgreSQL integration test against the target database class
   of service:

   ```bash
   CMREMOTE_POSTGRES_CONNECTION_STRING='Host=<host>;Port=<port>;Database=postgres;Username=<user>;Password=<password>' \
     dotnet test Tests/Server.Tests/Server.Tests.csproj \
       --configuration Release \
       --filter TestCategory=PostgreSqlIntegration \
       -p:EnableWindowsTargeting=true
   ```

5. Configure `AgentUpgrade:ManifestUrls` for the release channel(s) being
   tested.
6. Start the server and confirm it uses the manifest-backed dispatcher when a
   manifest URL is present.
7. Confirm `/hubs/service` is reachable from the lab agents.
8. Confirm the dashboard shows enrolled devices, their current version, and
   pending upgrade state.

## Browser/WebRTC lab gate

The desktop lab is not complete until a real browser receives video frames
from a controlled desktop and the rendered pixels match the controlled frame
source.

Record the following evidence for each browser run:

1. Server URL, commit SHA, browser version, and agent OS.
2. Operator starts a remote-control session from the browser UI.
3. Agent emits a host-local "connected" notification without requiring local
   approval.
4. Browser receives and renders video frames.
5. Pixel check compares the rendered browser frame with the controlled desktop
   pattern and records pass/fail.
6. Mouse, keyboard, and clipboard events are either verified or explicitly
   marked out of scope for that run.
7. Operator stops the session.
8. Agent emits a host-local "disconnected" notification.
9. Server audit/session evidence identifies the operator, device, start time,
   and stop time without logging access keys, TURN credentials, clipboard
   contents, or typed text.

The hosted `Desktop E2E lab` workflow remains the CI gate for the pieces that
can run without dedicated browser/display hardware. The hardware/browser lab
above is the manual promotion gate.

## Signed self-update lab

For each OS, start with an older enrolled agent and upgrade to the draft
release version via the server's manifest-backed agent-upgrade pipeline.

### Common checks

1. `ConnectionInfo.json` exists before the upgrade and contains the expected
   host, organization, and device id.
2. The server enrolls the device into `AgentUpgradeStatus`.
3. The dispatcher resolves the correct manifest entry for the device target
   and format.
4. The agent downloads the artifact and cosign bundle from the manifest
   sibling URLs.
5. The agent verifies SHA-256 before invoking the installer.
6. The agent verifies the cosign bundle and expected `signedBy` identity
   before invoking the installer.
7. The native installer runs.
8. The agent reconnects to `/hubs/service`.
9. The server observes the target `AgentVersion` and marks the row succeeded.
10. Failure cases leave the previous agent usable or produce a clear rollback
    action.

### Linux

- Validate both `.deb` and `.rpm` on matching distributions when possible.
- Confirm service ownership and file modes after upgrade:
  - `/etc/cmremote/ConnectionInfo.json` is readable only by the agent account
    and root.
  - systemd unit is enabled only when expected by the package script.
- Preserve package-manager logs and `journalctl -u cmremote-agent` output.

### Windows

- Validate the MSI path from an enrolled service context.
- Confirm `ConnectionInfo.json` ACL after install/update:
  - LocalSystem: full control
  - Administrators: full control
  - Built-in Users: read
- Confirm the service restarts under the expected account and reconnects.
- Preserve MSI logs, Windows Event Log entries, and ACL evidence.

### macOS

- Validate the `.pkg` path on the oldest supported macOS version and a current
  macOS version.
- Confirm launchd/service registration and reconnect.
- Confirm the host-local session notification path still works after update.
- Preserve installer logs and agent logs.

## Rollback gate

Before promoting a release:

1. Keep the previous channel manifest available.
2. Confirm the server can point a lab device at the previous channel.
3. Confirm the agent refuses tampered artifacts and unsigned manifest entries.
4. Confirm a failed update does not erase `ConnectionInfo.json`.
5. Confirm a database backup can be restored and the server starts from it.

## Promotion record

Attach this minimum record to the release/deployment ticket:

- release tag and commit SHA;
- release asset list with SHA-256 values;
- cosign verification output for each installable artifact;
- publisher manifest URL(s);
- PostgreSQL integration test result;
- browser/WebRTC lab result;
- Linux, Windows, and macOS self-update results;
- rollback test result;
- known warnings accepted for this release.

If any required line item is missing, keep the release as a draft/pre-release
and do not promote it to an operator deployment.
