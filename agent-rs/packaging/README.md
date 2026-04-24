# CMRemote Agent — installer wrappers (slice R8)

This directory holds the installer / packaging configuration for every
target the CMRemote Rust agent ships to. Each target produces an
artifact whose SHA-256 is recorded in the
[publisher manifest](../../docs/publisher-manifest.md) and consumed by
the M3 background agent-upgrade pipeline.

| Target | Format | Driver | Lives in | Output |
|---|---|---|---|---|
| `x86_64-unknown-linux-gnu` | `.deb` | [`cargo-deb`](https://github.com/kornelski/cargo-deb) ≥ 2.0 | `cmremote-agent/Cargo.toml` `[package.metadata.deb]` + `packaging/systemd/` | `target/debian/cmremote-agent_<ver>-1_amd64.deb` |
| `x86_64-unknown-linux-gnu` | `.rpm` | [`cargo-generate-rpm`](https://github.com/cat-in-136/cargo-generate-rpm) ≥ 0.14 | `cmremote-agent/Cargo.toml` `[package.metadata.generate-rpm]` + `packaging/systemd/` | `target/generate-rpm/cmremote-agent-<ver>-1.x86_64.rpm` |
| `x86_64-pc-windows-msvc` | `.msi` | [`cargo-wix`](https://github.com/volks73/cargo-wix) ≥ 0.3 + WiX Toolset 3.x | `packaging/wix/main.wxs` | `target/wix/cmremote-agent-<ver>-x86_64.msi` |
| `universal2-apple-darwin` | `.pkg` | `pkgbuild` + `lipo` (macOS Xcode CLI tools) | `packaging/macos/build-pkg.sh` | `target/macos/cmremote-agent-<ver>-universal.pkg` |

## Building locally

### Linux `.deb`

```sh
cd agent-rs
cargo install cargo-deb
cargo build --release -p cmremote-agent
cargo deb -p cmremote-agent --no-build
```

### Linux `.rpm`

```sh
cd agent-rs
cargo install cargo-generate-rpm
cargo build --release -p cmremote-agent
cargo generate-rpm -p crates/cmremote-agent
```

### Windows `.msi` (Windows runner only)

```powershell
cd agent-rs
cargo install cargo-wix
cargo build --release -p cmremote-agent --target x86_64-pc-windows-msvc
cargo wix -p cmremote-agent --no-build --nocapture
```

### macOS `.pkg` (macOS runner only)

```sh
cd agent-rs
cargo build --release -p cmremote-agent --target x86_64-apple-darwin
cargo build --release -p cmremote-agent --target aarch64-apple-darwin
CMREMOTE_VERSION=$(cargo pkgid -p cmremote-agent | sed 's/.*#//')
CMREMOTE_BIN_X64=target/x86_64-apple-darwin/release/cmremote-agent \
CMREMOTE_BIN_ARM64=target/aarch64-apple-darwin/release/cmremote-agent \
    bash packaging/macos/build-pkg.sh
```

## Continuous integration

`.github/workflows/release.yml` runs all four targets on every `v*` tag,
emits the publisher manifest with the resulting SHA-256s, and uploads
the manifest + artifacts as release assets. Sigstore cosign keyless
signs every artifact (Track S / S5).

## Why systemd hardening?

The shipped `cmremote-agent.service` unit applies the systemd hardening
directives (`ProtectSystem`, `RestrictAddressFamilies`,
`SystemCallFilter=@system-service`, etc.) that bound the agent's runtime
surface to what the agent actually needs:

- Outbound HTTPS to the configured CMRemote server only — no listening
  sockets.
- Read/write access only to `/var/lib/cmremote` (state) and
  `/var/log/cmremote` (logs); the rest of the filesystem is mounted
  read-only.
- No privilege escalation (`NoNewPrivileges=true`,
  `RestrictSUIDSGID=true`).

A compromised agent therefore cannot pivot off the endpoint without
also defeating systemd's own sandbox.
