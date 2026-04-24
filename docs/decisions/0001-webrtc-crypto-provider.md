# ADR 0001 — Crypto provider for the agent-side WebRTC stack

- **Status:** Accepted — Option B (fork `webrtc` onto `aws-lc-rs`)
- **Date:** 2026-04-24 (proposed); 2026-04-24 (accepted)
- **Slice:** R7.f (precedes R7's "remaining work" item 1)
- **Deciders:** CMRemote maintainers, security reviewers
- **Owners of the answer:** Whoever flips, or refuses to flip, the
  `ring` ban in [`agent-rs/deny.toml`](../../agent-rs/deny.toml).

## Context

Slice R7 ships the wire surface
([`cmremote-wire::desktop`](../../agent-rs/crates/cmremote-wire/src/desktop/)),
the security-contract guards
([`cmremote-platform::desktop::guards`](../../agent-rs/crates/cmremote-platform/src/desktop/guards.rs)),
the capture / encode trait surface
([`cmremote-platform::desktop::media`](../../agent-rs/crates/cmremote-platform/src/desktop/media.rs)),
and the frozen method-surface vectors
([`docs/wire-protocol-vectors/method-surface/`](../wire-protocol-vectors/method-surface/)).
What it does **not** ship is the WebRTC peer-connection driver itself.

Every realistic Rust WebRTC implementation we are aware of pulls in
some piece of crypto for DTLS 1.2 / SRTP / SCTP. The workspace's
current cargo-deny policy ([`agent-rs/deny.toml`](../../agent-rs/deny.toml#L93-L102))
explicitly bans `ring` in favour of `aws-lc-rs`:

```toml
{ name = "ring", reason = "Prefer aws-lc-rs or rustls-webpki defaults." },
```

That ban was a deliberate Track S decision, taken when the only
crypto consumer was rustls (which now defaults to `aws-lc-rs` on
stable). Adding a WebRTC stack changes the calculus, because the
mainstream Rust WebRTC implementations have a hard `ring` dependency
in their DTLS / SRTP layer. Continuing to ship a desktop-transport
*stub* (slice R7's current state) is fine; shipping a real driver
forces a decision.

This ADR documents the three options the maintainers considered,
records the answers to the questions that gated the decision, and
captures the chosen direction: **Option B — fork `webrtc` onto
`aws-lc-rs`.** The `ring` ban stays in place; no `Cargo.toml` and
no `deny.toml` is touched in this slice. Subsequent slices land
the feasibility spike and (if it succeeds) the driver itself.

## Decision drivers

1. **Supply-chain hygiene.** Every crypto crate in the agent's tree
   widens the unsafe boundary and the audit surface. The threat
   model
   ([`docs/threat-model.md`](../threat-model.md))
   requires the agent's crypto to be auditable and to come from a
   single, declared origin.
2. **Maintenance burden.** The Rust agent is maintained by a small
   team. A fork that we have to keep in sync with upstream `webrtc`
   is a permanent drag; a from-scratch DTLS-SRTP-SCTP stack is even
   worse.
3. **Cross-platform reach.** The desktop driver has to build on
   Windows MSVC, two Linux triples (x86_64 and aarch64 GNU), and
   two macOS triples (x86_64 and aarch64). Whatever crypto crate we
   pick has to compile cleanly on all five — that's what
   [`agent-rs/deny.toml`](../../agent-rs/deny.toml#L22-L28) gates on.
4. **Licence story.** `ring` ships under a custom OpenSSL-like
   licence that is *not* on our `[licenses].allow` list and is *not*
   straightforwardly SPDX-classifiable. Admitting it is a provenance
   decision, not just a technical one.
5. **Performance.** The R7 acceptance bar is "latency / FPS within
   10 % of the .NET Desktop client". Whatever crypto we pick has to
   not be the bottleneck; the .NET reference uses the OS-supplied
   crypto (SChannel / Apple CryptoKit / OpenSSL) which is hardware-
   accelerated on every supported host.
6. **Reversibility.** Once a real driver ships and operators have
   negotiated sessions against it, swapping the crypto crate becomes
   a coordinated upgrade rather than a local change. The decision
   we make here is essentially load-bearing for years.

## Options

### Option A — Admit `ring` for the upstream `webrtc` crate

Remove (or scope-narrow) the `ring` ban in
[`agent-rs/deny.toml`](../../agent-rs/deny.toml#L93-L102), pull in
`webrtc = "0.x"` as a normal dependency, and let it drag in `ring` as
its DTLS-SRTP backend. The
[`webrtc`](https://crates.io/crates/webrtc) crate is the de-facto
upstream Rust WebRTC stack; the WebRTC.rs project maintains it under
the Apache-2.0 / MIT dual licence and ships releases broadly aligned
with the W3C spec.

**Pros**

- Smallest footprint of new code we own. The driver becomes "wire
  the existing W3C-shaped API to our `DesktopTransportProvider`".
- Active upstream — bug fixes, RFC updates, congestion-control
  refinements all land for free.
- Lowest risk of cryptographic mistakes: `ring` is one of the most
  audited cryptographic libraries in the Rust ecosystem.
- Leaves the door open to switch crypto providers later if upstream
  `webrtc` grows a runtime-pluggable backend (mirroring the journey
  rustls took with `aws-lc-rs`).

**Cons**

- **Reverses a deliberate Track S decision.** Track S explicitly
  excluded `ring`. Reintroducing it has to be a maintainer-level
  call, not a contributor-level one.
- Adds a non-SPDX-clean licence to the dependency graph. The
  `[licenses].allow` block in
  [`agent-rs/deny.toml`](../../agent-rs/deny.toml#L48-L75) does not
  currently cover `ring`'s ISC / OpenSSL hybrid notice; we would
  either widen that block or add an explicit `[licenses].exceptions`
  entry. Each has a justification we must write down here.
- Two crypto stacks in one process: rustls' default (`aws-lc-rs`)
  *and* `ring` for `webrtc`. That doubles the audit surface and the
  "what does CVE X affect?" question becomes "both, neither, or one
  of them?".
- Cross-compilation footprint: `ring` needs assembler toolchains for
  every target triple. Our CI matrix currently covers all five but
  the build minutes go up.
- Reversibility: once `ring` is in the lock-file, every subsequent
  audit cycle has to clear it. Removing it is harder than not
  adding it.

### Option B — Fork `webrtc` onto `aws-lc-rs`

Maintain a CMRemote fork of the `webrtc` crate (and its DTLS / SRTP
sub-crates) with `ring` swapped out for `aws-lc-rs`. Feed the fork
through the `[sources].allow-git` allow-list so cargo-deny still
gates it.

**Pros**

- Keeps the Track S decision intact: the agent ships exactly one
  crypto provider (`aws-lc-rs`) and the `[bans].deny` list stays
  authoritative.
- We control the patch cadence — a CVE in the fork is on us, but a
  CVE in upstream `ring` no longer touches us at all.
- Upstream `webrtc` is structured around traits (`dtls::Conn`,
  `srtp::Session`); the swap is mostly mechanical for a
  proof-of-concept.

**Cons**

- **Permanent maintenance burden.** Every upstream `webrtc` release
  has to be rebased, re-tested, and re-audited. The Rust WebRTC
  spec surface is large (peer-connection state machine, ICE, DTLS,
  SCTP, congestion control, RTCP feedback) and moves regularly.
- `aws-lc-rs` does not 1:1 cover everything `ring` does for DTLS
  1.2 / SRTP (curve / cipher coverage, GCM / SHA primitives bound to
  particular FFI shapes); some translation glue is non-trivial.
- We become a downstream of `aws-lc-rs` for a domain
  (DTLS / SRTP) that `aws-lc-rs` was not primarily designed for —
  expect to discover gaps. Pre-work: a feasibility spike that
  enumerates every `ring` symbol called from `webrtc-dtls` /
  `webrtc-srtp` and matches each to an `aws-lc-rs` equivalent.
- Engineering cost dwarfs the cost of the rest of slice R7. This
  is a multi-month commitment.

### Option C — Thinner SRTP / DTLS / SCTP on top of `aws-lc-rs` directly

Skip the `webrtc` crate entirely. Implement the minimal slice of
the WebRTC stack the agent actually needs (one peer connection, one
data channel for input, one video track) on top of:

- `aws-lc-rs` for DTLS 1.2 (or DTLS 1.3, if maintainers accept the
  W3C spec change)
- a hand-rolled SRTP transformer using `aws-lc-rs` AEAD primitives
- `webrtc-sctp` (if we can extract it without `ring`) or a minimal
  SCTP-over-DTLS implementation

**Pros**

- Smallest possible runtime / audit surface — we only ship the
  features we use, which is roughly 10 % of upstream `webrtc`.
- No fork-rebase churn.
- Single crypto provider end-to-end.

**Cons**

- **By far the largest engineering cost** of any option here. RFC
  5763 / 5764 / 6347 / 6904 are not small RFCs; getting interop with
  the .NET viewer right (which goes through the browser's WebRTC
  stack) is a long-tail debugging effort.
- **By far the largest cryptographic risk.** A bespoke SRTP
  transformer is a footgun even with `aws-lc-rs` doing the AEAD —
  rekeying, replay-window handling, and SDES-vs-DTLS-SRTP key
  derivation are all easy to get subtly wrong. This option needs
  external crypto review before it ships.
- Schedule: pushes the R7 acceptance bar (latency / FPS parity)
  out by quarters, not weeks.

## Decision

**Accepted — Option B: fork the upstream `webrtc` crate (and its
`webrtc-dtls` / `webrtc-srtp` sub-crates) onto `aws-lc-rs`.**

The agent will continue to ship exactly one crypto provider
(`aws-lc-rs`). The
[`agent-rs/deny.toml`](../../agent-rs/deny.toml) ban on `ring`
**stays in place** — that is the whole point of Option B. The
WebRTC driver lands behind the existing
[`DesktopTransportProvider`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
seam introduced in slices R7.a–R7.i, with the forked `webrtc`
crate consumed via a `[patch.crates-io]` entry pointing at a
pinned git ref in a CMRemote-org-owned repository, allow-listed
under `[sources].allow-git` in `deny.toml`.

### Decision rationale

Option A was rejected on structural grounds, not cost grounds.
Admitting `ring` would reverse a deliberate Track S decision
(driver 1), introduce a non-SPDX-clean licence to the dependency
graph (driver 4), and put two crypto providers in one process —
permanently doubling the audit surface and complicating CVE
triage. Reversibility (driver 6) is the killer: once operators
have negotiated sessions against a `ring`-backed driver, swapping
crypto providers is a coordinated migration, not a `Cargo.toml`
edit. Picking the option with the worst exit cost for short-term
velocity is the wrong trade for a product that is not yet in
production.

Option C was rejected on risk and schedule grounds. A bespoke
SRTP transformer on top of `aws-lc-rs` AEAD primitives is exactly
the class of code where replay-window handling, rekeying cadence,
SDES-vs-DTLS-SRTP key derivation, and key zeroisation are easy to
get subtly wrong (driver 5; see also Question 4 below). Interop
debugging against the .NET viewer's browser-backed WebRTC stack
would be a long-tail effort measured in quarters, pushing the R7
acceptance bar (latency / FPS parity with the .NET Desktop client)
out far past what the roadmap can absorb. Option C remains the
documented fallback **iff** the Option B feasibility spike (see
*Consequences* below) shows that the `ring` → `aws-lc-rs` symbol
mapping is not tractable.

Option B was chosen because its costs — a fork to rebase per
upstream release, plus a one-time symbol-mapping spike — are
real but **bounded and visible**, and because upstream `webrtc`
is structured around the `dtls::Conn` / `srtp::Session` traits
that make backend swaps mostly mechanical. Track S's
single-crypto-provider posture survives intact (drivers 1, 4),
the `deny.toml` ban stays authoritative without a licence
carve-out (driver 4), CVE blast radius shrinks (driver 1), and
the driver lands behind seams the slices R7.a–R7.i already
shipped (driver 6).

## Maintainer questions — answers of record

The questions below were the gating items in the *Proposed*
revision of this ADR. They are answered here as part of accepting
Option B.

1. **Crypto-provider single-source rule.** *One provider per
   process is a hard constraint.* The agent ships `aws-lc-rs`
   only. This is what disqualifies Option A; it is also what
   forces the fork in Option B rather than consuming upstream
   `webrtc` directly.
2. **Licence policy on `ring`.** *No change to
   `[licenses].allow` or `[licenses].exceptions`.* Because
   Option B keeps `ring` out of the dependency graph entirely,
   the existing licence allow-list remains authoritative as
   written. If the fork ever pulls `ring` back in transitively
   that is a fork bug, not a policy change, and `cargo deny`
   will surface it via the existing `[bans].deny` entry.
3. **Fork maintenance commitment.** *Owner:* the CMRemote
   maintainers (CODEOWNERS for `agent-rs/`). *Location:* a
   dedicated `CrashMediaIT/webrtc-cmremote` repository in the
   CMRemote GitHub organisation, consumed via a
   `[patch.crates-io]` entry in
   [`agent-rs/Cargo.toml`](../../agent-rs/Cargo.toml) that pins
   a tagged git ref, with the host added to
   `[sources].allow-git` in `deny.toml`. *Rebase cadence:* on
   every upstream `webrtc` minor release **and** on every
   advisory affecting `aws-lc-rs` or the WebRTC RFC stack
   (RFC 5763 / 5764 / 6347 / 6904 / 8261), whichever comes
   first. A vendored copy under `agent-rs/vendor/` was
   considered and rejected — it muddies provenance and makes
   downstream consumption of upstream tags awkward.
4. **Crypto review for bespoke SRTP.** *Not applicable to
   Option B.* The fork inherits upstream `webrtc`'s SRTP /
   DTLS state machines verbatim; only the AEAD / curve / hash
   primitives are routed through `aws-lc-rs`. The crypto-review
   commitment in this question only re-enters scope if we fall
   back to Option C; in that case the requirement stands as
   originally written and the threat model
   ([`docs/threat-model.md`](../threat-model.md)) must be
   amended before any bespoke-SRTP code ships.
5. **Cross-compile coverage.** *All five targets in
   [`agent-rs/deny.toml`](../../agent-rs/deny.toml#L22-L28) must
   build green on the fork before the driver PR merges.* The
   highest-risk leg is `aws-lc-rs` on
   `aarch64-unknown-linux-gnu` (C-toolchain dependency); the
   feasibility spike (see *Consequences*) is responsible for
   demonstrating it. The PR that introduces the
   `[patch.crates-io]` entry must include a CI run that actually
   builds for all five triples; "it builds on x86_64" is not
   sufficient evidence.
6. **Reversibility plan.** *Exit path:* if upstream `webrtc`
   ever grows a runtime-pluggable crypto backend (mirroring the
   journey rustls took with `aws-lc-rs`), retire the fork and
   consume upstream directly with the `aws-lc-rs` backend
   selected. *Failure path:* if the fork becomes infeasible to
   maintain (e.g. upstream restructures around a `ring`-only
   primitive with no clean shim), this ADR is reopened and
   Option C re-evaluated against the threat budget at that time.
   In neither case does the agent fall back to Option A without
   a fresh Track S decision and a new ADR.
7. **Cut-over coordination.** *The .NET `IDesktopHubClient`
   side proceeds against the slice R7.g signalling DTOs and
   provider hooks, which are stable wire contract regardless of
   the agent-side crypto choice.* Option B does not change R7.g's
   shape; the .NET side is unblocked today. Agent-side driver
   work is gated on the feasibility spike below; the R7
   acceptance bar (latency / FPS parity) is met when both sides
   land together against a stable build of the fork.

## Consequences

- **Immediately (this ADR):** no `Cargo.toml`, no `deny.toml`,
  and no source code under `agent-rs/` is changed. The
  [`NotSupportedDesktopTransport`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
  stub remains the only registered provider; every desktop hub
  invocation continues to surface as a structured "not supported
  on `<host_os>`" failure that the operator UI already handles,
  and the slice R7.b guards continue to refuse hostile requests
  with a precise message.
- **Next step (gating the driver PR):** a **time-boxed
  feasibility spike** — budgeted at roughly two engineer-weeks —
  that enumerates every `ring` symbol called from
  `webrtc-dtls` and `webrtc-srtp` and matches each to an
  `aws-lc-rs` equivalent (or to a small shim). The spike output
  is a short report appended to this ADR (or linked from it) and
  a green CI run on all five target triples. If the spike
  uncovers gaps that cannot be shimmed without re-implementing
  cryptographic primitives, this ADR is reopened and Option C
  is re-evaluated.
  - **Status (2026-04-24):** Spike **approved to proceed** — see
    [0001-spike-approval.md](0001-spike-approval.md) for gate #1
    sign-off and deliverables. Deliverable #1 (symbol mapping) is
    landed at [0001-spike-report.md](0001-spike-report.md) with a
    **GO** recommendation. Deliverable #2 (PoC with green CI
    demonstrating the substitution works) was landed as the
    `cmremote-webrtc-crypto-spike` workspace member (11/11 tests
    passed against real `aws-lc-rs` 1.16.x, exercising every
    distinct symbol from the report) and has now been deleted by
    Step 8 of the runbook once the fork was wired in via
    `[patch.crates-io]`; `cargo test` evidence is preserved in git
    history. **Maintainer gate #2 is hereby ACCEPTED (2026-04-24)**
    on the basis of those two deliverables — the
    `CrashMediaIT/webrtc-cmremote` repository was subsequently
    created per the runbook at
    [0001-spike-fork-instructions.md](0001-spike-fork-instructions.md)
    and Step 8 of that runbook (wiring `[patch.crates-io]` to
    tag `v0.5.4-cmremote.1` and adding the `[sources].allow-git`
    entry) has landed against this repository.
- **After the spike succeeds:** the follow-up PR that creates the
  `CrashMediaIT/webrtc-cmremote` repository, adds the
  `[patch.crates-io]` entry and the `[sources].allow-git`
  allow-list entry to
  [`agent-rs/deny.toml`](../../agent-rs/deny.toml), and lands the
  WebRTC driver behind the existing
  [`DesktopTransportProvider`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
  seam. The fork-creation and the `[patch.crates-io]` /
  `[sources].allow-git` parts have landed (see "Status" above);
  the actual WebRTC driver remains the next R7 slice and is the
  step that activates the dormant `webrtc-dtls` patch by pulling
  it into the crate graph. The `[bans].deny` entry for `ring` is
  **not** touched in any of these PRs — its continued presence is
  the load-bearing assertion that Option B is still being honoured.
- **Ongoing:** every upstream `webrtc` minor release triggers a
  fork-rebase task owned by the `agent-rs/` CODEOWNERS, with the
  rebased fork re-pinned via the `[patch.crates-io]` git ref.
  CVE notifications affecting `aws-lc-rs` or the relevant RFCs
  trigger an out-of-band rebase regardless of upstream cadence.

## References

- [W3C WebRTC 1.0 — peer connection IDL surface](https://www.w3.org/TR/webrtc/)
- [RFC 5763 — Framework for Establishing a Secure Real-time Transport Protocol (SRTP) Security Context Using DTLS](https://www.rfc-editor.org/rfc/rfc5763)
- [RFC 5764 — DTLS Extension to Establish Keys for SRTP](https://www.rfc-editor.org/rfc/rfc5764)
- [RFC 8261 — Datagram Transport Layer Security (DTLS) Encapsulation of SCTP Packets](https://www.rfc-editor.org/rfc/rfc8261)
- [`webrtc` crate (WebRTC.rs)](https://crates.io/crates/webrtc)
- [`aws-lc-rs` crate](https://crates.io/crates/aws-lc-rs)
- [CMRemote threat model](../threat-model.md)
- [CMRemote roadmap — slice R7 row](../../ROADMAP.md)
- [Feasibility spike approval — gate #1](0001-spike-approval.md)
- [Feasibility spike report — `ring` → `aws-lc-rs` symbol mapping](0001-spike-report.md)
- Spike PoC crate — formerly `agent-rs/crates/cmremote-webrtc-crypto-spike/`; deleted by Step 8 once the fork was wired in via `[patch.crates-io]`; `cargo test` evidence preserved in git history
- [Fork-creation runbook for `CrashMediaIT/webrtc-cmremote`](0001-spike-fork-instructions.md)
