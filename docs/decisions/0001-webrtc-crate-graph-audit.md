# ADR 0001 — `webrtc-rs` Crate-Graph Supply-Chain Audit (slice R7.l)

- **Parent ADR:** [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- **Spike report (dtls only):** [0001-spike-report.md](0001-spike-report.md)
- **Spike runbook:** [0001-spike-fork-instructions.md](0001-spike-fork-instructions.md)
- **Slice:** R7.l (closes the "remaining work item 2a" enumerated by the
  R7 row in [`ROADMAP.md`](../../ROADMAP.md))
- **Status:** Deliverable complete — per-sub-crate verdicts recorded
- **Date:** 2026-04-24
- **Audience:** CMRemote maintainers; whoever owns the follow-up
  `webrtc-cmremote-*` fork PRs and the eventual R7.m driver PR

## Why this audit exists

ADR 0001 selected **Option B — fork `webrtc-rs` onto `aws-lc-rs`** —
and the [spike report](0001-spike-report.md) executed that decision
**for `webrtc-dtls@v0.5.4` only**. At the time of the spike the rest
of the upstream `webrtc-rs/webrtc` crate-graph was assumed to be
either crypto-free or feature-toggleable; that assumption was never
verified against the actual `Cargo.toml` files of the sibling
sub-crates.

The umbrella crate the eventual R7.m driver will pull in
(`webrtc = "0.x"`) drags every sub-crate listed below into the
agent's dependency graph. If any of them carries a transitive
`ring` dependency, the existing `[bans].deny` entry on `ring` in
[`agent-rs/deny.toml`](../../agent-rs/deny.toml#L93-L102) trips and
the build fails closed — exactly as designed, but only after we've
already started writing driver code against a stack that won't
build. R7.l catches that case **before** the driver PR opens.

The audit is also the input to the fork-creation runbook: each
`needs-fork` verdict below corresponds to a new Step entry in
[`0001-spike-fork-instructions.md`](0001-spike-fork-instructions.md)
(see [§"Step 10+ — Fork the rest of the `webrtc-rs` crate
graph"](0001-spike-fork-instructions.md) once the appendix lands)
and a new commented-out
[`[sources].allow-git`](../../agent-rs/deny.toml) placeholder in
`deny.toml`.

Per the parent ADR's *Consequences*, **no `Cargo.toml`, no
`deny.toml` ban list, and no source code under `agent-rs/` is
behaviourally changed by this slice.** The deny.toml additions are
commented-out placeholders only; the `ring` ban stays
authoritative; the production build is byte-identical.

## Method

The upstream snapshot audited is `webrtc-rs/webrtc` at tag
[`v0.17.0`](https://github.com/webrtc-rs/webrtc/tree/v0.17.0) — the
final feature release of the Tokio-coupled async API per the
upstream README's "v0.17.x feature freeze" notice (Feb 2026). The
in-flight v0.20.x sans-I/O rewrite on `master` is **deliberately
excluded**: it is still under active development, has no tagged
release, and (per the upstream architecture note) re-homes the
protocol core into a separate `rtc` repo whose crate-graph will
need its own audit when it stabilises. Pinning the audit to a
tagged release is the same discipline the dtls spike used
(`v0.5.4`).

For each workspace member of `webrtc-rs/webrtc@v0.17.0` the audit
recorded:

1. Direct `[dependencies]` declared in the sub-crate's own
   `Cargo.toml`, filtered for crypto-bearing crates (`ring`,
   `rcgen`, `rustls`, `openssl`, `aws-lc-rs`, plus the RustCrypto
   primitives the dtls and srtp spike reports already enumerated:
   `aes`, `aes-gcm`, `ccm`, `ctr`, `cbc`, `hmac`, `sha1`, `sha2`,
   `md-5`, `chacha20poly1305`, `p256`, `p384`, `x25519-dalek`,
   `ed25519-dalek`, `sec1`, `rsa`).
2. Transitive `ring` exposure inherited via sibling workspace
   members (`workspace = true` declarations).
3. Whether the sub-crate exposes a Cargo feature that selects an
   alternate crypto provider (e.g. `srtp`'s optional `openssl`
   feature flag).

The raw `Cargo.toml` extracts the verdicts below were derived from
are reproducible verbatim with:

```bash
for crate in webrtc data dtls ice interceptor mdns media \
             rtcp rtp sctp sdp srtp stun turn util; do
  echo "=== $crate ==="
  curl -fsSL "https://raw.githubusercontent.com/webrtc-rs/webrtc/v0.17.0/${crate}/Cargo.toml" \
    | sed -n '/^\[dependencies\]/,/^\[/p'
done
```

A future audit refresh (driven by an upstream minor-release rebase
or by a CVE notification) re-runs that one-liner, diffs the result,
and updates the verdict table below in the same PR that bumps the
fork's pinned tag.

## Verdict table

| Sub-crate (workspace member → published name) | Direct crypto deps at `v0.17.0` | Verdict | Rationale | Required action |
|---|---|---|---|---|
| `webrtc/` → [`webrtc`](https://crates.io/crates/webrtc) | `ring 0.17.14`, `rcgen 0.13` (default features → `ring`), `sha2` | **needs-fork** | The umbrella crate the R7.m driver imports has its own direct `ring` line, plus pulls `rcgen` with the default crypto provider. The same mechanical sed substitution the dtls spike used (`s/use ring::/use aws_lc_rs::/g` etc.) applies; `rcgen` exposes an `aws_lc_rs` feature on `0.13` so the swap is a feature-list change, not a code change. | Fork as `webrtc-cmremote/webrtc/` (see runbook Step 10). |
| `dtls/` → [`webrtc-dtls`](https://crates.io/crates/webrtc-dtls) | `ring 0.17.14`, `rcgen 0.13`, `rustls 0.23` (`features=["std","ring"]`), plus full RustCrypto primitives (`p256`, `p384`, `x25519-dalek`, `aes`, `aes-gcm`, `ccm`, `chacha20poly1305`, `hmac`, `sha1`, `sha2`) | **needs-fork** *(rebase)* | The existing fork at [`CrashMediaIT/webrtc-cmremote`](https://github.com/CrashMediaIT/webrtc-cmremote) (tag `v0.5.4-cmremote.1`) is pinned to upstream's pre-monorepo `webrtc-rs/dtls@v0.5.4`. The R7.m driver needs the v0.17 series. The substitution is the same mechanical diff (extended for the `rustls features=["std","ring"]` → `["std","aws_lc_rs"]` swap that was a no-op in v0.5.4). | Rebase the existing fork onto upstream `v0.17.0`'s `dtls/` tree (see runbook Step 11). |
| `stun/` → [`webrtc-stun`](https://crates.io/crates/webrtc-stun) | `ring 0.17.14`, `md-5`, `subtle`, `crc` | **needs-fork** | Uses `ring::hmac` for the HMAC-SHA1 message-integrity attribute (RFC 5389 §15.4). `aws_lc_rs::hmac` exposes the same `Key::new(HMAC_SHA1, …)` / `sign` / `verify_tag` API; substitution is mechanical. No `rcgen` here — narrower diff than `webrtc` or `dtls`. | Fork as `webrtc-cmremote/stun/` (see runbook Step 12). |
| `turn/` → [`webrtc-turn`](https://crates.io/crates/webrtc-turn) | `ring 0.17.14`, `md-5`, `base64` | **needs-fork** | Uses `ring::hmac` for the long-term-credential MESSAGE-INTEGRITY computation (RFC 5766 §10.2). Same mechanical `ring::hmac` → `aws_lc_rs::hmac` substitution as `stun`. | Fork as `webrtc-cmremote/turn/` (see runbook Step 13). |
| `srtp/` → [`webrtc-srtp`](https://crates.io/crates/webrtc-srtp) | `hmac`, `sha1`, `aes`, `aes-gcm`, `ctr`, `aead`, **optional** `openssl 0.10.72` | **clean *(policy gate)*** | No `ring` in the dependency tree. The crypto path is RustCrypto-only by default. The `openssl` feature is opt-in and would trip the `[bans].deny` `openssl-sys` entry in [`agent-rs/deny.toml`](../../agent-rs/deny.toml#L97) if ever enabled — keep it disabled and the sub-crate consumes verbatim. | None in this slice. R7.m driver PR must take `webrtc-srtp` with the default feature set only and assert it by adding a `cargo tree -i openssl-sys --no-default-features` smoke check to its CI. |
| `ice/` → [`webrtc-ice`](https://crates.io/crates/webrtc-ice) | none directly | **clean *(transitive only)*** | No direct crypto deps. Pulls `webrtc-stun`, `webrtc-turn`, `webrtc-mdns`, `webrtc-util` via `workspace = true`. Once `stun` and `turn` are forked, `ice` rides on the forked tree without needing its own fork. | None in this slice. Pinned via `[patch.crates-io]` only if upstream emits a hot-fix that does not yet land in our forks. |
| `mdns/` → [`webrtc-mdns`](https://crates.io/crates/webrtc-mdns) | none | **clean** | No crypto. Plain socket I/O over `webrtc-util`. | None. |
| `interceptor/` → [`interceptor`](https://crates.io/crates/interceptor) | none directly (depends on `srtp`) | **clean *(transitive only)*** | No direct crypto. The `srtp` dependency is consumed with default features, so the policy gate above (no `openssl` feature) covers it transitively. | None in this slice. |
| `data/` → [`webrtc-data`](https://crates.io/crates/webrtc-data) | none directly | **clean** | No crypto. SCTP-over-DTLS data channel framing only; the DTLS layer underneath is `webrtc-dtls`, which is forked separately. | None. |
| `media/` → [`webrtc-media`](https://crates.io/crates/webrtc-media) | none | **clean** | No crypto. Frame / sample types only. | None. |
| `util/` → [`webrtc-util`](https://crates.io/crates/webrtc-util) | none | **clean** | No crypto in the default feature set. The `marshal`, `conn`, `vnet`, `buffer`, `sync`, `ifaces` features that sub-crates enable are all crypto-free. | None. |
| `rtcp/` → [`rtcp`](https://crates.io/crates/rtcp) | none | **clean** | No crypto. | None. |
| `rtp/` → [`rtp`](https://crates.io/crates/rtp) | none | **clean** | No crypto. | None. |
| `sctp/` → [`webrtc-sctp`](https://crates.io/crates/webrtc-sctp) | none | **clean** | No crypto. The CRC-based packet integrity (RFC 4960) uses `crc`, not a keyed primitive. | None. |
| `sdp/` → [`sdp`](https://crates.io/crates/sdp) | none | **clean** | No crypto. Pure parser. | None. |

## Summary

- **Four `needs-fork` verdicts** at `v0.17.0`: `webrtc` (umbrella),
  `webrtc-dtls` (rebase the existing fork from v0.5.4 → v0.17.0),
  `webrtc-stun`, `webrtc-turn`. Every one of these is a mechanical
  symbol-substitution diff of the shape the dtls spike already
  proved out (`ring::hmac` → `aws_lc_rs::hmac`,
  `ring::signature::*` → `aws_lc_rs::signature::*`,
  `ring::rand::SystemRandom` → `aws_lc_rs::rand::SystemRandom`,
  plus the `rustls features` swap and the `rcgen` feature-list
  swap).
- **One `clean *(policy gate)*` verdict:** `webrtc-srtp` is
  RustCrypto-only by default but exposes an opt-in `openssl`
  feature that would trip the existing `openssl-sys` ban. The R7.m
  driver PR must consume `webrtc-srtp` with default features only
  and add a `cargo tree -i openssl-sys` smoke check to its CI. No
  fork is required.
- **All other workspace members** (`ice`, `mdns`, `interceptor`,
  `data`, `media`, `util`, `rtcp`, `rtp`, `sctp`, `sdp`) are
  **clean** — they consume the four forks transitively but do not
  themselves need a substitution diff.

## Recommended fork repository layout

The existing fork at
[`CrashMediaIT/webrtc-cmremote`](https://github.com/CrashMediaIT/webrtc-cmremote)
was created during the v0.5.4 dtls spike when upstream `dtls` lived
in its own repo (`webrtc-rs/dtls`). Upstream has since consolidated
every workspace member into a single monorepo
(`webrtc-rs/webrtc`); the rebase therefore has a choice:

- **Option L1 — One fork per `needs-fork` sub-crate.** Mirror the
  Step-1…Step-9 pattern verbatim. Four repos:
  `webrtc-cmremote-webrtc`, `webrtc-cmremote-dtls`,
  `webrtc-cmremote-stun`, `webrtc-cmremote-turn`. Pros: maps 1:1
  onto the existing runbook; each fork is independently rebasable.
  Cons: four independent CI matrices, four CODEOWNERS surfaces,
  four sets of branch-protection bookkeeping. Inter-crate
  workspace deps need to be patched to git URLs in every fork's
  `Cargo.toml` (each fork loses access to the upstream
  `[workspace.dependencies]` table).
- **Option L2 — One monorepo fork mirroring upstream.** Repurpose
  the existing `CrashMediaIT/webrtc-cmremote` repository as a fork
  of the entire `webrtc-rs/webrtc` workspace at tag `v0.17.0`,
  apply the four mechanical diffs in a single branch
  (`cmremote/v0.17.0-aws-lc-rs`), and emit one tag
  (`v0.17.0-cmremote.1`). The four `[patch.crates-io]` entries in
  `agent-rs/Cargo.toml` all point at the same git ref but
  different `path = "..."` sub-directories (per cargo's documented
  `[patch.crates-io].<name> = { git = "...", path = "..." }`
  syntax). Pros: one rebase target, one CI matrix, one CODEOWNERS,
  one branch-protection ruleset; matches upstream's own structure
  so the upstream `[workspace.dependencies]` table keeps working
  unchanged. Cons: the rebase touches every sub-crate at once;
  any single sub-crate's substitution failing fails the whole
  tag. Mitigation: the sed patch is mechanical and per-file, so
  bisecting failures is straightforward.

**Recommendation: Option L2.** The four mechanical diffs are too
correlated (they all swap the same three `ring` symbols against
the same `aws-lc-rs` API surface) to justify the four-repo
overhead. A single monorepo fork keeps the rebase cadence cheap
and matches the way upstream itself manages cross-crate changes.
The runbook appendix below is written against Option L2; if a
maintainer prefers Option L1, the same Step bodies apply, repeated
per repo, with the cross-crate `workspace = true` lines rewritten
to git deps in each fork.

## Out of scope for R7.l

- **Actually creating any new fork repository.** Creating a repo
  in the `CrashMediaIT` organisation requires org admin rights
  that the cloud agent does not have (same constraint that gated
  the v0.5.4 spike — see [`0001-spike-fork-instructions.md`](0001-spike-fork-instructions.md)
  §preamble). The runbook Step-10+ entries this audit produces are
  a maintainer-executable runbook, not an automated deliverable.
- **Flipping the `[bans].deny` `ring` entry in
  [`agent-rs/deny.toml`](../../agent-rs/deny.toml).** Per the
  parent ADR's *Consequences*, the ban stays in place across every
  intermediate slice. R7.l adds commented-out placeholder
  `[sources].allow-git` entries pointing at the prospective fork
  URLs but the fork URLs are unreachable until the maintainer
  creates them, so the entries are deliberately disabled until
  that lands. The placeholder shape is the only `deny.toml` change
  in this slice.
- **Adding `webrtc = "0.x"` to `agent-rs/Cargo.toml`.** That
  belongs to slice R7.m and is the trigger that activates every
  patch entry. The current PR adds none.
- **Auditing `webrtc-rs/rtc` (the v0.20-track sans-I/O repo).** That
  audit is gated on upstream tagging a v0.20 release. When it
  does, this document gets a sibling table for the new repo and
  the verdict either supersedes or augments the v0.17 plan.

## Reference workspace audit (2026-04-24)

For provenance: the audit was executed against
`webrtc-rs/webrtc@v0.17.0` on 2026-04-24. The crypto-bearing
deps observed verbatim per sub-crate (the input to the verdict
table above) are:

```text
webrtc       : ring 0.17.14, rcgen 0.13 (default), sha2 0.10
dtls         : ring 0.17.14, rcgen 0.13, rustls 0.23 [std,ring],
               p256 0.13, p384 0.13, x25519-dalek 2, hmac 0.12,
               sha1 0.10, sha2 0.10, aes 0.8, cbc 0.1, aes-gcm 0.10,
               ccm 0.5, chacha20poly1305 0.10.1, sec1 0.7
stun         : ring 0.17.14, md-5 0.10, subtle 2.4, crc 3
turn         : ring 0.17.14, md-5 0.10, base64 0.22.1
srtp         : hmac 0.12, sha1 0.10, ctr 0.9, aes 0.8, aead 0.5,
               aes-gcm 0.10, [optional] openssl 0.10.72
ice          : (none direct; transitive via stun + turn + mdns)
mdns         : (none direct)
interceptor  : (none direct; transitive via srtp)
data         : (none direct)
media        : (none direct)
util         : (none direct)
rtcp         : (none direct)
rtp          : (none direct)
sctp         : (none direct)
sdp          : (none direct)
```

## Cross-references

- Parent ADR: [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- Spike approval: [0001-spike-approval.md](0001-spike-approval.md)
- Spike report (dtls v0.5.4 symbol mapping):
  [0001-spike-report.md](0001-spike-report.md)
- Fork-creation runbook (Step-1…9 cover dtls v0.5.4; the new
  Step-10+ entries land alongside this audit):
  [0001-spike-fork-instructions.md](0001-spike-fork-instructions.md)
- ROADMAP — slice R7 row: [../../ROADMAP.md](../../ROADMAP.md)
- Agent-side deny policy:
  [`agent-rs/deny.toml`](../../agent-rs/deny.toml)
