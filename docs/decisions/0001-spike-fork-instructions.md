# ADR 0001 — Fork-Creation Instructions for `CrashMediaIT/webrtc-cmremote`

- **Parent ADR:** [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- **Spike report:** [0001-spike-report.md](0001-spike-report.md)
- **Spike PoC crate:** [`agent-rs/crates/cmremote-webrtc-crypto-spike/`](../../agent-rs/crates/cmremote-webrtc-crypto-spike/)
- **Status:** Active — this is the runbook for creating the external repository the ADR's Option B requires
- **Audience:** CMRemote maintainers with `CrashMediaIT` org admin rights

This document is the **runbook** for creating the
`CrashMediaIT/webrtc-cmremote` repository, applying the symbol
substitutions the spike report enumerated (and the spike PoC crate
proved with `cargo test`), and pinning it from `agent-rs/` via
`[patch.crates-io]`. It exists because the external repository is
**outside the agent-repo PR boundary** — the cloud agent that
landed the spike PoC cannot create new repositories under the
`CrashMediaIT` GitHub organization. A human maintainer with org
admin rights is required.

The order below is **load-bearing**. In particular, the
`[bans].deny` entry on `ring` in
[`agent-rs/deny.toml`](../../agent-rs/deny.toml) **is not touched**
at any point — its continued presence is the load-bearing assertion
that Option B is being honoured (per parent ADR §"Consequences").
If at any step `cargo deny check bans` reports a `ring` finding in
the `agent-rs/` workspace, **stop** and re-evaluate Option C.

---

## Step 1 — Create the `CrashMediaIT/webrtc-cmremote` repository

A maintainer with `CrashMediaIT` org admin rights:

1. Create a **new public repository** in the `CrashMediaIT`
   organisation named `webrtc-cmremote`. Description suggested:
   *"CMRemote fork of `webrtc-rs/dtls` with `ring` swapped for
   `aws-lc-rs`. Tracks ADR 0001 in
   [CMRemote/docs/decisions/0001-webrtc-crypto-provider.md](https://github.com/CrashMediaIT/CMRemote/blob/main/docs/decisions/0001-webrtc-crypto-provider.md)."*
2. License: `MIT/Apache-2.0` (matches upstream).
3. Default branch: `main`.
4. **Branch protection** on `main`: required reviews (1
   maintainer), status checks must pass, no force-pushes, no
   deletion. Same posture as `CrashMediaIT/CMRemote`.
5. **CODEOWNERS:** add `agent-rs/` CMRemote CODEOWNERS as the
   default reviewers — every release rebase has to be approved by
   the same set of people who own the `agent-rs/` workspace.
6. **Topics:** `webrtc`, `dtls`, `aws-lc-rs`, `cmremote`.
7. Enable Dependabot security updates and `cargo` ecosystem
   updates so transitive CVEs surface against the fork on the same
   cadence as the parent repo.

## Step 2 — Seed the fork from `webrtc-rs/dtls@v0.5.4`

Set `$Root` to the parent folder where the clone will be created, then run
the block below from a PowerShell session that has `git` and `gh` on `$PATH`:

```powershell
Set-Location $Root
git clone https://github.com/CrashMediaIT/webrtc-cmremote.git
Set-Location .\webrtc-cmremote

git remote add upstream https://github.com/webrtc-rs/dtls.git
git fetch upstream --tags
git checkout -b cmremote/v0.5.4-aws-lc-rs v0.5.4

# Add the dual licence files upstream uses (matches "MIT/Apache-2.0").
# webrtc-rs/dtls already ships these at v0.5.4, so this is usually a no-op.
git ls-files | Select-String -Pattern '^LICENSE'

# Add CODEOWNERS (Step 1.5). Replace the team handle with the actual
# CMRemote agent-rs CODEOWNERS team — check
# https://github.com/CrashMediaIT/CMRemote/blob/main/.github/CODEOWNERS
# (or wherever your CODEOWNERS lives) and reuse the same handle.
New-Item -ItemType Directory -Force -Path .github | Out-Null
@'
* @CrashMediaIT/cmremote-maintainers
'@ | Set-Content -Encoding utf8 .github/CODEOWNERS

git add .github/CODEOWNERS
git commit -m "Add CODEOWNERS (mirrors CMRemote agent-rs owners)"

# First push: create main from this branch so branch protection has something to protect.
git push -u origin cmremote/v0.5.4-aws-lc-rs
git push origin cmremote/v0.5.4-aws-lc-rs:refs/heads/main

# Now apply branch protection on main (Step 1.4).
$body = @{
    required_status_checks = @{
        strict = $true
        contexts = @()   # fill in after Step 6 adds the CI matrix; see Step 6.
    }
    enforce_admins = $true
    required_pull_request_reviews = @{
        required_approving_review_count = 1
        require_code_owner_reviews = $true
        dismiss_stale_reviews = $true
    }
    restrictions = $null
    allow_force_pushes = $false
    allow_deletions  = $false
} | ConvertTo-Json -Depth 6

$body | gh api -X PUT repos/CrashMediaIT/webrtc-cmremote/branches/main/protection `
    -H "Accept: application/vnd.github+json" --input -
```

(Why a branch off the tag rather than a fork-of-fork: the spike
report's symbol enumeration was performed against `v0.5.4`. The
diff is mechanical only against that exact tree.)

## Step 3 — Apply the symbol substitutions from the spike report

The complete list is in
[`0001-spike-report.md`](0001-spike-report.md) §"Direct symbols
used by `webrtc-dtls` and their `aws-lc-rs` equivalents". For every
file in `src/` that the report's symbol enumeration identified
(only `src/crypto/mod.rs` for `webrtc-dtls@v0.5.4`):

1. Replace `use ring::rand::SystemRandom;` with
   `use aws_lc_rs::rand::SystemRandom;`.
2. Replace `use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, RsaKeyPair};`
   with `use aws_lc_rs::signature::{EcdsaKeyPair, Ed25519KeyPair, RsaKeyPair};`.
3. Replace every fully-qualified `ring::signature::*` reference
   with the matching `aws_lc_rs::signature::*` reference. The
   constants are name-identical
   (e.g. `ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING` becomes
   `aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING`).
4. Replace the `&dyn ring::signature::VerificationAlgorithm` type
   annotation with `&dyn aws_lc_rs::signature::VerificationAlgorithm`.

The mechanical sed patch is, literally:

```bash
# Run from the repo root.
git ls-files 'src/**/*.rs' | xargs sed -i \
    -e 's|use ring::|use aws_lc_rs::|g' \
    -e 's|ring::signature::|aws_lc_rs::signature::|g' \
    -e 's|ring::rand::|aws_lc_rs::rand::|g'
# Sanity check: no ring references should remain in src/.
! grep -rn 'ring' src/
```

The PoC crate at
[`agent-rs/crates/cmremote-webrtc-crypto-spike/`](../../agent-rs/crates/cmremote-webrtc-crypto-spike/)
is a **standalone, runnable demonstration** that this exact
substitution preserves behaviour for every distinct symbol. Re-run
its tests against the fork to confirm:

```bash
cd path/to/CMRemote/agent-rs
cargo test -p cmremote-webrtc-crypto-spike
```

Expected: 11/11 passed (Ed25519, ECDSA‑P256‑ASN1, ECDSA‑P384‑ASN1
verify, RSA‑PKCS#1‑SHA256/384/512, RSA‑PKCS#1‑SHA1 legacy verify,
the negative-path tampering check, the empty-public-key error
check, and the trait-object compile assertion).

## Step 4 — Update `Cargo.toml` to drop `ring` and pin `aws-lc-rs`

In the fork's `Cargo.toml`:

1. **Remove** the `ring = "0.16.19"` dependency line.
2. **Add** `aws-lc-rs = "1"` to `[dependencies]`. Pin the major
   version only — this matches the constraint
   `agent-rs/crates/cmremote-webrtc-crypto-spike/Cargo.toml`
   already uses, so the fork and the workspace will resolve to a
   single `aws-lc-rs` version (the cargo-deny `multiple-versions`
   policy is `warn`, not `deny`, so a brief drift won't fail CI,
   but a single resolution is the goal).
3. **Bump** `rcgen` from `0.9.2` to `^0.13` and select its
   `aws-lc-rs` crypto-provider feature
   (`rcgen = { version = "0.13", default-features = false, features = ["aws_lc_rs", "pem"] }`
   — verify the exact feature name against the rcgen 0.13 manifest;
   the indirect-dependency table in
   [`0001-spike-report.md`](0001-spike-report.md) §"Indirect
   dependencies on `ring`" calls this out as a **low-medium-risk**
   item that has to land in the same PR as the symbol substitution.
4. **Bump** `rustls` from `0.19` to a current version (or, per the
   spike report's preference, **drop the `rustls`-type borrow
   entirely** in favour of `rustls-pki-types::CertificateDer` — the
   fork only borrows type plumbing, not actual TLS, so the cleanest
   diff is `s/rustls::Certificate/rustls_pki_types::CertificateDer/`
   in the four places `webrtc-dtls`'s `crypto/mod.rs` and
   `handshaker.rs` reference it).
5. Run `cargo update -p ring --precise 0.0.0 || true` then
   `cargo tree -i ring --target all` to confirm `ring` is **gone**
   from the fork's lock-file.

## Step 5 — Run the fork's own test suite

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

If any test fails, the substitution diverged from "mechanical" —
**stop** and re-read the failing site against the symbol-mapping
table in [`0001-spike-report.md`](0001-spike-report.md). Do not
"fix" tests; the substitution is supposed to be observably
identical.

## Step 6 — Cross-compile coverage on all five target triples

Per the parent ADR's Question 5 ("All five targets in
`agent-rs/deny.toml` must build green on the fork before the
driver PR merges"), add a CI matrix to the fork that builds for:

- `x86_64-pc-windows-msvc` (windows-latest runner)
- `x86_64-unknown-linux-gnu` (ubuntu-latest)
- `aarch64-unknown-linux-gnu` (cross / ubuntu-latest with the
  `aarch64-linux-gnu-gcc` toolchain — this is the highest-risk
  leg per Question 5)
- `x86_64-apple-darwin` (macos-13)
- `aarch64-apple-darwin` (macos-latest)

A reference workflow is the matrix already in
[`.github/workflows/rust.yml`](../../.github/workflows/rust.yml) of
this repo, which proves `aws-lc-rs` builds for the same set of
triples in `cmremote-agent`.

**Gate condition:** all five legs must be green on the fork's
`main` branch before Step 7 runs.

## Step 7 — Tag the fork

```bash
git tag -a v0.5.4-cmremote.1 -m "CMRemote v0.5.4 fork: ring -> aws-lc-rs"
git push origin v0.5.4-cmremote.1
```

The tag name follows the convention `v<upstream-version>-cmremote.<rev>`:
`<upstream-version>` is the rebased upstream tag (`v0.5.4`),
`<rev>` is incremented on every CMRemote-side bump. This makes the
provenance line ("we are at upstream v0.5.4 plus our rev N
patches") obvious from the tag alone.

## Step 8 — Wire the fork into `agent-rs/` (separate PR against this repo)

Open a follow-up PR against `CrashMediaIT/CMRemote` that:

1. Adds `[patch.crates-io]` to
   [`agent-rs/Cargo.toml`](../../agent-rs/Cargo.toml):
   ```toml
   [patch.crates-io]
   webrtc-dtls = { git = "https://github.com/CrashMediaIT/webrtc-cmremote.git", tag = "v0.5.4-cmremote.1" }
   ```
2. Adds the host to `[sources].allow-git` in
   [`agent-rs/deny.toml`](../../agent-rs/deny.toml):
   ```toml
   [sources]
   allow-git = ["https://github.com/CrashMediaIT/webrtc-cmremote"]
   ```
3. Lands the WebRTC driver behind the existing
   [`DesktopTransportProvider`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
   seam — no wire-protocol changes needed because slices R7.a–R7.j
   already shipped the contract.
4. **Does not touch** the `[bans].deny` entry for `ring`. If the
   fork's lock-file doesn't have `ring`, neither will the agent
   workspace's, and the ban can stay authoritative without a
   policy change.

That follow-up PR is also where the spike PoC crate
(`agent-rs/crates/cmremote-webrtc-crypto-spike/`) becomes
**deletable**: once the real fork is wired in, the PoC has served
its purpose. Delete it in the same PR that wires `[patch.crates-io]`,
linking back to this runbook so the `cargo test` evidence is
preserved in git history.

## Step 9 — Document the rebase cadence

Add a `MAINTENANCE.md` to the fork repo describing:

- **Trigger 1:** every upstream `webrtc-rs/dtls` minor release.
  Rebase `cmremote/v<NEW>-aws-lc-rs` from upstream's tag,
  re-apply the sed patch from Step 3, re-run the agent-side
  spike PoC, tag `v<NEW>-cmremote.1`.
- **Trigger 2:** every advisory affecting `aws-lc-rs` or the
  WebRTC RFC stack (RFCs 5763 / 5764 / 6347 / 6904 / 8261).
  Out-of-band rebase regardless of upstream cadence.
- **Owner:** `agent-rs/` CMRemote CODEOWNERS (per parent ADR
  Question 3).

## Failure mode — Option C re-evaluation

If at any step the fork cannot be made to build green on all five
triples, or the rcgen / rustls-pki-types diff turns out to require
re-implementing cryptographic primitives (which the spike report
specifically said it would not), **stop** and reopen
[`0001-webrtc-crypto-provider.md`](0001-webrtc-crypto-provider.md)
per its §"Consequences" failure path. Option C remains the
documented fallback. Do **not** silently fall back to Option A
(admitting `ring`) without a fresh Track S decision and a new ADR.

## Cross-references

- Parent ADR: [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- Spike approval (gate #1): [0001-spike-approval.md](0001-spike-approval.md)
- Spike report (deliverable #1): [0001-spike-report.md](0001-spike-report.md)
- Spike PoC crate (deliverable #2 — running-code evidence):
  [`agent-rs/crates/cmremote-webrtc-crypto-spike/`](../../agent-rs/crates/cmremote-webrtc-crypto-spike/)
- ROADMAP — slice R7 row: [../../ROADMAP.md](../../ROADMAP.md)
