# ADR 0001 — Crypto provider for the agent-side WebRTC stack

- **Status:** Proposed — awaiting maintainer decision
- **Date:** 2026-04-24
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

This ADR documents the three options the maintainers can pick from,
the questions that have to be answered before any of them is
actionable, and the **non-decision** that this ADR itself represents.
The `ring` ban stays in place; no `Cargo.toml` is touched in this
slice.

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

**No decision yet.** This ADR is the artifact slice R7.f produces;
it deliberately does not pick a winner. The
[`agent-rs/deny.toml`](../../agent-rs/deny.toml) ban on `ring` stays
in place. Subsequent slices (the actual WebRTC driver) cannot
proceed until the maintainers answer the questions in the next
section and update this ADR's *Status* line.

## Questions the maintainers must answer before any option is actionable

1. **Crypto-provider single-source rule.** Is the project willing
   to run two crypto providers in one process (Option A), or is
   "one provider per process" a hard constraint (rules out Option
   A, forces Option B or C)? This is a Track S call.
2. **Licence policy on `ring`.** If Option A is picked: do we
   widen `[licenses].allow` to cover `ring`'s notice, or add it as
   an explicit `[licenses].exceptions` entry with a justification?
   Either way the diff in `deny.toml` needs a CODEOWNERS sign-off.
3. **Fork maintenance commitment.** If Option B is picked: who
   owns the fork? What's the rebase cadence (per upstream release,
   per CVE, both)? Where does the fork live (a CMRemote-org repo,
   a fork in `agent-rs/vendor/`, a `[patch.crates-io]` entry
   pointing at a git ref)?
4. **Crypto review for bespoke SRTP.** If Option C is picked:
   which external reviewer signs off on the SRTP transformer
   before it ships? What does the threat model
   ([`docs/threat-model.md`](../threat-model.md)) require us to
   add about replay windows, rekeying cadence, and key-zeroisation?
5. **Cross-compile coverage.** Does the chosen option still build
   on all five targets in
   [`agent-rs/deny.toml`](../../agent-rs/deny.toml#L22-L28)? In
   particular, `ring`'s assembler dependency on Windows MSVC and
   `aws-lc-rs`'s C-toolchain dependency on `aarch64-unknown-linux-gnu`
   both have a history of rough edges; the PR that flips the
   policy bit must include a CI run that actually builds for all
   five.
6. **Reversibility plan.** Whatever option we pick, what is the
   exit strategy? Concretely: if option A is chosen and `ring`
   becomes unmaintained or relicensed in five years, what is the
   migration path? The plan should be one paragraph in *this* ADR
   before we ship a driver against the chosen option.
7. **Cut-over coordination.** The R7 acceptance bar
   ([ROADMAP.md](../../ROADMAP.md)'s R7 row) requires the .NET
   `IDesktopHubClient` side and the agent side to land together.
   Which option lets the .NET side make progress against a stable
   wire contract first? Slice **R7.g** (signalling DTOs and
   provider hooks, no WebRTC dependency) is designed to be the
   stable contract regardless of which option lands here, so
   answering this question is not blocking R7.g — but it *is*
   blocking the actual driver.

## Consequences

- **Until this ADR is decided:** the
  [`NotSupportedDesktopTransport`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
  stub stays the only registered provider; every desktop hub
  invocation continues to surface as a structured "not supported on
  `<host_os>`" failure that the operator UI already handles.
  The slice R7.b guards ensure that even in the stub state, hostile
  requests are refused with a precise message.
- **When this ADR is decided:** the maintainer who flips the bit
  updates the *Status* line above to *Accepted — Option <X>*,
  records the answers to the questions in the section above as
  in-line edits to this file, and opens the follow-up PR that
  changes [`agent-rs/deny.toml`](../../agent-rs/deny.toml) and adds
  the chosen WebRTC crate (or fork). That PR — not this one — is
  where the actual policy bit flips.

## References

- [W3C WebRTC 1.0 — peer connection IDL surface](https://www.w3.org/TR/webrtc/)
- [RFC 5763 — Framework for Establishing a Secure Real-time Transport Protocol (SRTP) Security Context Using DTLS](https://www.rfc-editor.org/rfc/rfc5763)
- [RFC 5764 — DTLS Extension to Establish Keys for SRTP](https://www.rfc-editor.org/rfc/rfc5764)
- [RFC 8261 — Datagram Transport Layer Security (DTLS) Encapsulation of SCTP Packets](https://www.rfc-editor.org/rfc/rfc8261)
- [`webrtc` crate (WebRTC.rs)](https://crates.io/crates/webrtc)
- [`aws-lc-rs` crate](https://crates.io/crates/aws-lc-rs)
- [CMRemote threat model](../threat-model.md)
- [CMRemote roadmap — slice R7 row](../../ROADMAP.md)
