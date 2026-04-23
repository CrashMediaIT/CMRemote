# Security Policy

<!-- Source: CMRemote, clean-room implementation. -->

CMRemote is a remote-management product. The agent runs privileged on every
enrolled endpoint 24/7, and the server brokers remote control, script
execution, and package installation across a fleet. The security of both is
taken seriously. This document describes **how to report a vulnerability** and
**what to expect in return**.

This policy is the *operational* counterpart to the normative *Security
model* section of [`docs/wire-protocol.md`](docs/wire-protocol.md#security-model)
and the roadmap's cross-cutting [Track S — Security & supply-chain
baseline](ROADMAP.md#-track-s--security--supply-chain-baseline-cross-cutting).
The roadmap item that introduces this file is **S1**.

## Supported versions

CMRemote is still pre-production. Security fixes are applied as follows:

| Branch / version                | Status            | Security fixes |
|---------------------------------|-------------------|----------------|
| `main`                          | Active, pre-release | ✅ Best-effort, on every release |
| `v1-maintenance` *(planned)*    | Legacy .NET agent line | ✅ Security-only |
| Pre-fork upstream builds        | Out of scope      | ❌ Please report to the upstream project |

Once CMRemote tags `v2.0.0`, this matrix is revised: `main` becomes the
supported release line, and the prior minor version receives security-only
backports for 90 days after its successor ships.

## Reporting a vulnerability

**Do not open a public GitHub issue for security reports.** Use one of the
channels below.

1. **GitHub private vulnerability reporting** *(preferred)*. Open
   <https://github.com/CrashMediaIT/CMRemote/security/advisories/new>.
   This routes the report directly to the maintainers without creating a
   public paper trail.
2. **Email.** Send a report to **security@crashmedia.ca**. Encrypted reports
   are strongly preferred; see the PGP key fingerprint below.

### PGP

The security mailbox PGP key fingerprint is published at
<https://crashmedia.ca/.well-known/security.txt> once that endpoint is live.
Until then, request the key by sending a signed empty email to
`security@crashmedia.ca` and one will be sent in reply. Do not trust any key
material distributed through another channel.

### What to include

A useful report typically contains:

- A short description of the vulnerability class (e.g. *authentication
  bypass*, *command injection*, *SSRF*, *path traversal*).
- The affected component: agent (Rust under `agent-rs/` or .NET under
  `Agent/`), server (`Server/`), wire protocol
  (`docs/wire-protocol.md`), migration importer
  (`CMRemote.Migration.Legacy` once it ships), or agent-upgrade pipeline.
- Steps to reproduce, or a minimal proof-of-concept. A failing test vector
  that fits in [`docs/wire-protocol-vectors/`](docs/wire-protocol-vectors/)
  is ideal for protocol-level bugs.
- The commit SHA or release tag you tested against.
- Your assessment of impact and any mitigating factors.
- Whether you intend to publish independently, and on what timeline. If you
  would like us to credit you in the advisory, please say so.

Please do **not** attach proof-of-concept exploits that contain secrets,
customer data, or data you do not have rights to share.

## Our commitments

When you report a vulnerability through one of the channels above, we will:

1. **Acknowledge** the report within **3 business days**.
2. **Triage** the report — confirm, request more information, or explain why
   we consider it out of scope — within **10 business days**.
3. **Keep you updated** at least every 14 days while the report is open.
4. **Coordinate disclosure** on a **90-day default window** from triage to
   public disclosure, with a faster track for actively-exploited issues and
   a slower track only by mutual agreement. If we cannot fix an issue within
   90 days we will say so explicitly and discuss the path forward before
   the window closes.
5. **Credit you** in the published advisory unless you prefer to remain
   anonymous.
6. **Publish a GitHub Security Advisory** with a CVE where applicable once
   a fix has shipped and users have had a reasonable window to upgrade.

We do not currently run a paid bug bounty program.

## Scope

**In scope:**

- The Rust agent (`agent-rs/`).
- The legacy .NET agent (`Agent/`) while it remains in tree.
- The .NET server (`Server/`, `Server.*/`).
- The shared libraries (`Shared/`).
- The desktop remote-control stack (`Desktop*/`).
- The wire protocol documented in `docs/wire-protocol.md` and its test
  vector corpus under `docs/wire-protocol-vectors/`.
- The migration pipeline and agent-upgrade orchestrator described in
  roadmap items **M1**–**M4**.
- Official container images and installer artefacts built from this
  repository.

**Out of scope:**

- Self-hosted deployments that have been modified outside of the upstream
  Docker image or supported installers. We will still read these reports
  but cannot guarantee a fix.
- Social-engineering attacks against individual maintainers or operators.
- Denial-of-service requiring sustained resource exhaustion from a
  privileged network position (e.g. flooding the agent hub from an already
  authenticated agent). Reports of *amplification* or *cheap unauthenticated
  DoS* are in scope.
- Physical attacks against a host that already has an authorised agent
  installed (e.g. an attacker with local `Administrators` can replace the
  agent binary). Reports of attacks that **escape** the agent's documented
  trust boundary are in scope.
- Findings in third-party dependencies that are already being tracked by
  `cargo-audit`, `dependency-review`, or Dependabot and have no
  demonstrable impact on CMRemote. We still appreciate a heads-up.

For an explicit list of trust boundaries, surfaces, and non-goals, see
the [threat model](docs/threat-model.md) (roadmap item **S3**).

## Safe harbour

We will not pursue legal action against researchers who:

- Make a good-faith effort to avoid privacy violations, destruction of data,
  or interruption of service during their research;
- Only interact with accounts they own or have explicit permission from the
  account holder to access;
- Provide us a reasonable time to resolve the issue before disclosing it
  publicly, per the timelines above.

If legal action is initiated by a third party against you for activities
conducted in accordance with this policy, we will make our good-faith
support known.

## Hardening baseline

These controls are maintained in the repository and enforced by CI. They
are listed here so that reporters know what is already expected to be true
and can focus reports on gaps.

- Every pull request is gated by `cargo-deny`, `cargo-audit`, and GitHub's
  `dependency-review` (roadmap **S2**).
- Dependabot raises security and version-update PRs for `cargo`, `nuget`,
  `github-actions`, and `docker`.
- Secret scanning and push protection are enabled on the repository.
- CodeQL runs on pull requests and weekly on `main` (roadmap **S6**).
- Rust parsers for the wire protocol are exercised against a shared test
  vector corpus; MessagePack and JSON round-trip is byte-stable
  (roadmap **R1a**, **R1b**).
- The `ConnectionInfo.json` file is required to be written with file-mode
  `0600` on Unix; its `Debug` implementation is hand-written to redact the
  server verification token (pinned by a unit test in
  `cmremote-wire`).

Thank you for helping keep CMRemote and its users safe.
