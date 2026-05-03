# CMRemote

CMRemote is a clean-room continuation of Remotely focused on self-hosted endpoint management, unattended remote control, scripting, package installs, migration tooling, and a new Rust endpoint agent.

This repository is mid-rewrite and **not production-deployable yet**. Use the roadmap and docs below as the source of truth for deployability status.

## Current status

- Rust agent Track R is implemented behind the `agent-rs/` workspace, including WebSocket/SignalR transport, device heartbeat, command execution, package-manager dispatch, WebRTC desktop transport, installer wrappers, and self-update handoff.
- Migration M5 tests/docs are shipped for legacy-to-v2 conversion.
- Release-side S5 integrity is wired: tagged releases generate SBOMs, cosign bundles, and provenance attestations.
- Remaining deployability work is tracked in `ROADMAP.md`.

## Start here

| Need | File |
|---|---|
| Roadmap and remaining deployability work | [`ROADMAP.md`](ROADMAP.md) |
| Rust agent architecture and local commands | [`agent-rs/README.md`](agent-rs/README.md) |
| Agent packaging and release artifacts | [`agent-rs/packaging/README.md`](agent-rs/packaging/README.md) |
| Publisher manifest contract | [`docs/publisher-manifest.md`](docs/publisher-manifest.md) |
| Wire protocol and frozen vectors | [`docs/wire-protocol.md`](docs/wire-protocol.md) |
| Setup wizard operator guide | [`docs/Setup-Wizard.md`](docs/Setup-Wizard.md) |
| Legacy migration guide | [`docs/Migration.md`](docs/Migration.md) |
| Threat model | [`docs/threat-model.md`](docs/threat-model.md) |

## Repository layout

- `Server/` — ASP.NET Core/Blazor server and SignalR hubs.
- `Shared/` — shared .NET DTOs, entities, and interfaces.
- `Agent/` — legacy .NET agent kept for compatibility during the cut-over.
- `agent-rs/` — clean-room Rust agent workspace.
- `Desktop.*` — legacy desktop client/native projects.
- `Migration.Legacy/` and `Migration.Cli/` — legacy database migration tooling.
- `Tests/` — .NET unit/integration tests.
- `docs/` — protocol, migration, setup, release, and security documentation.

## Common validation commands

Run commands from the repository root unless noted.

```bash
# .NET server tests
dotnet test Tests/Server.Tests/Server.Tests.csproj --no-restore

# Shared DTO tests
dotnet test Tests/Shared.Tests/Shared.Tests.csproj --no-restore

# Legacy migration converter/importer tests
dotnet test Tests/Migration.Legacy.Tests/Migration.Legacy.Tests.csproj --no-restore

# Rust agent tests
cd agent-rs
cargo test --workspace --all-targets

# Rust WebRTC desktop contract tests
cargo test -p cmremote-platform --features webrtc-driver desktop::webrtc::tests
```

## Desktop remote-control contract

The Rust agent's remote-control path is designed for unattended access:

- A valid operator session must not block on host-local approval prompts.
- The agent emits host-local connected and disconnected notifications for the controlled machine.
- Notification text is sanitised and must not include access keys, TURN credentials, clipboard contents, or typed text.
- WebRTC signalling payload size caps are mirrored in Rust and .NET.

The `Desktop E2E lab` GitHub Actions workflow validates the current browser/viewer DTO → .NET hub → Rust WebRTC transport contract that can run on hosted CI. Full browser/video lab coverage still requires runner-level WebRTC/display support.

## Release integrity

Tagged release builds produce:

- Native Rust agent packages: `.deb`, `.rpm`, `.msi`, and `.pkg`.
- `publisher-manifest.json` with SHA-256, cosign bundle, and certificate identity metadata.
- CycloneDX SBOMs for the Rust agent workspace and .NET server.
- Sigstore cosign bundles for artifacts and release metadata.
- SLSA provenance attestations through GitHub artifact attestations.

The Rust agent self-update path downloads the package, re-checks SHA-256, verifies the cosign bundle against the publisher identity, and only then invokes the native package installer.

## Deployment note

Do not treat legacy Remotely quickstart snippets as authoritative for CMRemote. The deployment path is being rebuilt around the Rust agent, publisher manifests, setup wizard, and migration tooling. Check `ROADMAP.md` before attempting an installation or handing work to another agent.

## License

See [`LICENSE`](LICENSE).
