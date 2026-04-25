# ADR 0001 — Fork-Creation Instructions for `CrashMediaIT/webrtc-cmremote`

- **Parent ADR:** [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- **Spike report:** [0001-spike-report.md](0001-spike-report.md)
- **Spike PoC crate:** formerly `agent-rs/crates/cmremote-webrtc-crypto-spike/` — deleted by Step 8 once the fork was wired in via `[patch.crates-io]`; `cargo test` evidence preserved in git history
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

In a working clone of the new repository:

```bash
git clone https://github.com/CrashMediaIT/webrtc-cmremote.git
cd webrtc-cmremote
git remote add upstream https://github.com/webrtc-rs/dtls.git
git fetch upstream --tags
# v0.5.4 is the version pinned by the symbol report.
git checkout -b cmremote/v0.5.4-aws-lc-rs v0.5.4
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

The PoC crate previously at
`agent-rs/crates/cmremote-webrtc-crypto-spike/` was a **standalone,
runnable demonstration** that this exact substitution preserves
behaviour for every distinct symbol. It was deleted in the same PR
that wired the fork in via `[patch.crates-io]` (Step 8) — its
`cargo test` evidence is preserved in git history at the commit
that landed the runbook
([`docs/decisions/0001-spike-fork-instructions.md`](0001-spike-fork-instructions.md))
and the spike report ([`0001-spike-report.md`](0001-spike-report.md)).
To re-run that evidence, check out the parent commit of the
deletion and run:

```bash
cd path/to/CMRemote/agent-rs
cargo test -p cmremote-webrtc-crypto-spike
```

Expected (at the deletion commit's parent): 11/11 passed (Ed25519,
ECDSA‑P256‑ASN1, ECDSA‑P384‑ASN1 verify, RSA‑PKCS#1‑SHA256/384/512,
RSA‑PKCS#1‑SHA1 legacy verify, the negative-path tampering check,
the empty-public-key error check, and the trait-object compile
assertion).

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

---

## Step 10+ — Fork the rest of the `webrtc-rs` crate graph (slice R7.l)

Steps 1–9 cover **`webrtc-dtls@v0.5.4` only**, because that was the
sole sub-crate the original feasibility spike audited. Slice R7.l
extended the audit to the full `webrtc-rs/webrtc@v0.17.0` workspace
and produced four `needs-fork` verdicts (see
[`0001-webrtc-crate-graph-audit.md`](0001-webrtc-crate-graph-audit.md)
§"Verdict table"): the umbrella `webrtc` crate plus
`webrtc-dtls@v0.17.0` (rebase), `webrtc-stun`, and `webrtc-turn`.
The remaining workspace members are either `clean` or `clean
*(transitive only)*` and need no source changes.

The audit document recommended **Option L2 — one monorepo fork
mirroring upstream's workspace layout** (see audit doc §"Recommended
fork repository layout"). Steps 10–14 below are written against
Option L2; if a maintainer instead prefers Option L1 (one fork per
sub-crate), apply the Step-2 / Step-3 / Step-5 bodies once per repo
and rewrite each fork's `[workspace.dependencies]` to git deps.

The `[bans].deny` entry on `ring` in
[`agent-rs/deny.toml`](../../agent-rs/deny.toml) **continues to be
untouched** at every step, exactly as for Step 8 of the dtls
runbook.

## Step 10 — Repurpose `CrashMediaIT/webrtc-cmremote` as a monorepo fork

The dtls v0.5.4 fork lives on the `cmremote/v0.5.4-aws-lc-rs` branch
of [`CrashMediaIT/webrtc-cmremote`](https://github.com/CrashMediaIT/webrtc-cmremote)
and is tagged `v0.5.4-cmremote.1`. That branch and tag are kept
**immutable** so the existing dormant `[patch.crates-io].webrtc-dtls`
entry continues to resolve. The new monorepo fork lives on a
**fresh branch** of the same repo:

```bash
git clone https://github.com/CrashMediaIT/webrtc-cmremote.git
cd webrtc-cmremote
git remote add upstream-monorepo https://github.com/webrtc-rs/webrtc.git
git fetch upstream-monorepo --tags
# v0.17.0 is the version pinned by the slice R7.l audit
# (the final feature release of the Tokio-coupled API).
git checkout -b cmremote/v0.17.0-aws-lc-rs v0.17.0
```

Note: the new branch shares **no commit history** with
`cmremote/v0.5.4-aws-lc-rs`. That is intentional — upstream
`webrtc-rs/dtls` and upstream `webrtc-rs/webrtc` are different
repositories at the git level, and the monorepo branch needs the
full upstream workspace tree, not a continuation of the old
dtls-only branch.

If the maintainer prefers Option L1 instead, repeat Step 1 four
times to create four new repos
(`webrtc-cmremote-webrtc`, `webrtc-cmremote-dtls-v0_17`,
`webrtc-cmremote-stun`, `webrtc-cmremote-turn`) and seed each from
the matching `webrtc-rs/webrtc@v0.17.0` sub-directory.

## Step 11 — Rebase `dtls/` onto upstream `v0.17.0`

The v0.5.4 → v0.17.0 upstream diff is non-trivial (almost two years
of upstream changes), but the `ring` → `aws-lc-rs` substitution
remains **mechanical and per-file**. Inside the new branch:

```bash
# Apply the same sed patch the v0.5.4 spike used, scoped to dtls/.
git ls-files 'dtls/src/**/*.rs' | xargs sed -i \
    -e 's|use ring::|use aws_lc_rs::|g' \
    -e 's|ring::signature::|aws_lc_rs::signature::|g' \
    -e 's|ring::rand::|aws_lc_rs::rand::|g' \
    -e 's|ring::hmac::|aws_lc_rs::hmac::|g'
# Sanity check: no ring references should remain in dtls/src/.
! grep -rn '\bring\b' dtls/src/
```

In `dtls/Cargo.toml`:

1. Remove `ring = "0.17.14"`; add `aws-lc-rs = "1"`.
2. Change `rustls = { version = "0.23.27", default-features = false, features = ["std", "ring"] }`
   to `rustls = { version = "0.23.27", default-features = false, features = ["std", "aws_lc_rs"] }`
   (verify the exact rustls 0.23 feature name against the
   manifest at the rebased upstream tag).
3. Change `rcgen = "0.13"` to
   `rcgen = { version = "0.13", default-features = false, features = ["aws_lc_rs", "pem"] }`
   — the audit doc's verdict for `dtls` calls this out as a
   feature-list change, not a code change.
4. Re-run the v0.5.4 spike's symbol-mapping test plan (§"Step 5 —
   Run the fork's own test suite") for the new tree:
   `cargo test -p webrtc-dtls`. Expected: green.

## Step 12 — Apply the substitution to `stun/`

The audit doc's verdict for `webrtc-stun` is "uses `ring::hmac` for
the RFC 5389 §15.4 message-integrity attribute; substitution is
mechanical via `aws_lc_rs::hmac`". Same sed pattern, scoped to
`stun/`:

```bash
git ls-files 'stun/src/**/*.rs' | xargs sed -i \
    -e 's|use ring::|use aws_lc_rs::|g' \
    -e 's|ring::hmac::|aws_lc_rs::hmac::|g'
! grep -rn '\bring\b' stun/src/
```

In `stun/Cargo.toml`: remove `ring = "0.17.14"`; add
`aws-lc-rs = "1"`. Run `cargo test -p webrtc-stun`.

`aws_lc_rs::hmac::Key::new(HMAC_SHA1, …)` / `sign` / `verify_tag`
are name-identical to the `ring::hmac` equivalents the upstream
`stun` code calls; if any test fails, the substitution diverged
from "mechanical" and Step 12 should be paused per the same
"do not 'fix' tests" rule as Step 5.

## Step 13 — Apply the substitution to `turn/`

The audit doc's verdict for `webrtc-turn` is "uses `ring::hmac`
for the RFC 5766 §10.2 long-term credential MESSAGE-INTEGRITY
computation; same substitution as `stun`". Same sed pattern,
scoped to `turn/`:

```bash
git ls-files 'turn/src/**/*.rs' | xargs sed -i \
    -e 's|use ring::|use aws_lc_rs::|g' \
    -e 's|ring::hmac::|aws_lc_rs::hmac::|g'
! grep -rn '\bring\b' turn/src/
```

In `turn/Cargo.toml`: remove `ring = "0.17.14"`; add
`aws-lc-rs = "1"`. Run `cargo test -p webrtc-turn`.

## Step 14 — Apply the substitution to the umbrella `webrtc/` crate

The audit doc's verdict for the umbrella crate is "direct
`ring = "0.17.14"` line plus `rcgen 0.13` with the default crypto
provider; substitution is the same sed pattern plus the rcgen
feature-list swap". Same sed pattern, scoped to `webrtc/`:

```bash
git ls-files 'webrtc/src/**/*.rs' | xargs sed -i \
    -e 's|use ring::|use aws_lc_rs::|g' \
    -e 's|ring::signature::|aws_lc_rs::signature::|g' \
    -e 's|ring::rand::|aws_lc_rs::rand::|g' \
    -e 's|ring::hmac::|aws_lc_rs::hmac::|g' \
    -e 's|ring::digest::|aws_lc_rs::digest::|g'
! grep -rn '\bring\b' webrtc/src/
```

In `webrtc/Cargo.toml`:

1. Remove `ring = "0.17.14"`; add `aws-lc-rs = "1"`.
2. Change `rcgen = { version = "0.13", features = ["pem", "x509-parser"] }`
   to `rcgen = { version = "0.13", default-features = false, features = ["aws_lc_rs", "pem", "x509-parser"] }`.
3. Run `cargo test -p webrtc` (this exercises the dtls / stun / turn
   forks transitively via `workspace = true` and is the
   single-command end-to-end smoke check for the monorepo fork).

## Step 15 — Cross-compile coverage on all five target triples

Per Step 6 of the dtls runbook, but now scoped to the four
forked workspace members at once. The matrix is identical:
`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu` (the highest-risk leg), `x86_64-apple-darwin`,
`aarch64-apple-darwin`. All five must be green on the fork's
`cmremote/v0.17.0-aws-lc-rs` branch before Step 16 runs.

## Step 16 — Tag the monorepo fork

```bash
git tag -a v0.17.0-cmremote.1 \
    -m "CMRemote v0.17.0 monorepo fork: ring -> aws-lc-rs in webrtc/, dtls/, stun/, turn/"
git push origin v0.17.0-cmremote.1
```

The naming convention (`v<upstream-version>-cmremote.<rev>`) is
identical to the dtls runbook's Step 7. The monorepo tag stands
alongside the existing `v0.5.4-cmremote.1` dtls tag; both remain
addressable indefinitely.

## Step 17 — Wire the monorepo fork into `agent-rs/` (the R7.m driver PR)

The follow-up PR that lands the WebRTC driver against the
[`DesktopTransportProvider`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
seam (slice R7.m) extends `agent-rs/Cargo.toml`'s
`[patch.crates-io]` to point each forked crate at the same git tag
but a different `path = "..."` sub-directory:

```toml
[patch.crates-io]
# Existing dtls v0.5.4 entry (kept as-is for now; the R7.m PR
# replaces it with the v0.17 monorepo entry below in the same
# commit that adds the umbrella `webrtc` dep).
# webrtc-dtls = { git = "https://github.com/CrashMediaIT/webrtc-cmremote.git", tag = "v0.5.4-cmremote.1" }

# New monorepo entries (slice R7.m). Cargo clones the fork once
# and discovers each named member by walking the fork's
# `[workspace.members]`; per-entry `path = "..."` selectors are
# **not** valid here — cargo rejects `git` + `path` as ambiguous.
webrtc       = { git = "https://github.com/CrashMediaIT/webrtc-cmremote.git", tag = "v0.17.0-cmremote.1" }
webrtc-dtls  = { git = "https://github.com/CrashMediaIT/webrtc-cmremote.git", tag = "v0.17.0-cmremote.1" }
webrtc-stun  = { git = "https://github.com/CrashMediaIT/webrtc-cmremote.git", tag = "v0.17.0-cmremote.1" }
webrtc-turn  = { git = "https://github.com/CrashMediaIT/webrtc-cmremote.git", tag = "v0.17.0-cmremote.1" }
```

The `[sources].allow-git` entry in
[`agent-rs/deny.toml`](../../agent-rs/deny.toml) is **already**
shaped to match the host (`https://github.com/CrashMediaIT/webrtc-cmremote`)
and needs no further change for the four new patch entries — the
git-host allow-list is per-host, not per-tag. The slice R7.l PR
adds **commented-out placeholder entries** below the existing
allow-list line so a maintainer flipping them on for Option L1
(one repo per sub-crate) has the URLs at the ready; those
placeholders stay commented out for Option L2.

The slice R7.m PR also adds a `cargo tree -i openssl-sys
--no-default-features` smoke check to the workspace CI to enforce
the audit doc's policy gate on `webrtc-srtp`'s opt-in `openssl`
feature (see audit doc §"Verdict table" → `srtp/` row).

The `[bans].deny` `ring` entry in `agent-rs/deny.toml` is
**still** untouched.

## Step 18 — Document the rebase cadence (monorepo edition)

The dtls runbook's Step 9 already specifies the per-fork
maintenance contract. For the monorepo fork the cadence is
unchanged but the trigger surface widens:

- **Trigger 1:** every upstream `webrtc-rs/webrtc` minor release.
  Rebase `cmremote/v<NEW>-aws-lc-rs` from upstream's tag, re-apply
  the four sed patches from Steps 11–14, re-run
  `cargo test --workspace`, tag `v<NEW>-cmremote.1`.
- **Trigger 2:** every advisory affecting `aws-lc-rs`, `rustls`,
  `rcgen`, or any of the WebRTC RFC stack (RFCs 5389 / 5763 /
  5764 / 5766 / 6347 / 6904 / 8261). Out-of-band rebase regardless
  of upstream cadence.
- **Trigger 3:** an upstream `webrtc-rs/rtc` v0.20.x release —
  re-run the slice R7.l audit against the new repo and decide
  whether the v0.17 monorepo fork supersedes or coexists.
- **Owner:** `agent-rs/` CMRemote CODEOWNERS, same as the dtls
  fork.

## Cross-references

- Parent ADR: [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- Spike approval (gate #1): [0001-spike-approval.md](0001-spike-approval.md)
- Spike report (deliverable #1, dtls v0.5.4):
  [0001-spike-report.md](0001-spike-report.md)
- Crate-graph supply-chain audit (slice R7.l, the input to
  Step-10+): [0001-webrtc-crate-graph-audit.md](0001-webrtc-crate-graph-audit.md)
- Spike PoC crate (deliverable #2 — running-code evidence):
  formerly at `agent-rs/crates/cmremote-webrtc-crypto-spike/`,
  deleted by Step 8 once the fork was wired in via
  `[patch.crates-io]`; `cargo test` evidence preserved in git
  history at the parent of the deletion commit.
- ROADMAP — slice R7 row: [../../ROADMAP.md](../../ROADMAP.md)
