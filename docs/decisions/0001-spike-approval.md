# ADR 0001 — Feasibility Spike Approval

**Parent ADR:** [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)  
**Status:** Approved  
**Date:** 2026-04-24  
**Approver:** CMRemote maintainers  

## Decision

The **Option B feasibility spike** described in ADR 0001 is **approved to proceed**.

### Scope of Work

The spike is time-boxed to **two engineer-weeks** and will:

1. **Enumerate every `ring` symbol** called from `webrtc-dtls` and `webrtc-srtp`
2. **Map each symbol** to an `aws-lc-rs` equivalent (or document required shim layer)
3. **Demonstrate green CI** on all five target triples:
   - `x86_64-pc-windows-msvc`
   - `x86_64-unknown-linux-gnu`
   - `aarch64-unknown-linux-gnu` *(highest-risk leg — C-toolchain dependency)*
   - `x86_64-apple-darwin`
   - `aarch64-apple-darwin`

### Exit Criteria

The spike succeeds if:
- All `ring` symbols have documented `aws-lc-rs` equivalents (or shimmed)
- A proof-of-concept build passes CI on all five target triples
- No cryptographic primitives require reimplementation

The spike **fails** and triggers ADR 0001 reopening if:
- Symbol gaps cannot be shimmed without reimplementing cryptographic primitives
- `aws-lc-rs` on `aarch64-unknown-linux-gnu` proves infeasible
- Build footprint is unacceptable on any target triple

### Deliverables

1. **Symbol mapping report** (to be appended to or linked from ADR 0001)
2. **Proof-of-concept fork** with green CI across all five triples
3. **Go/no-go recommendation** for proceeding to fork maintenance (maintainer gate #2)

## Next Steps

Upon spike completion:
- If **successful**: Proceed to maintainer gate #2 (sign off spike report)
- If **unsuccessful**: Reopen ADR 0001 and re-evaluate Option C

**Status (2026-04-24):** Deliverable #1 (symbol mapping) is **complete**
with a **GO** recommendation — see
[0001-spike-report.md](0001-spike-report.md). Deliverable #2 (PoC
demonstrating the substitution works) was **complete** as the
`cmremote-webrtc-crypto-spike` workspace crate (11/11 tests passed
against real `aws-lc-rs` 1.16.x; covered every distinct symbol from
the report) and has now been deleted by Step 8 of the runbook once
the fork was wired in via `[patch.crates-io]`; `cargo test` evidence
is preserved in git history. Deliverable #3 (go/no-go acceptance) is
**complete**: maintainer gate #2 is **ACCEPTED** as recorded in
[0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
§"Consequences" §"Status (2026-04-24)". The
external-repository creation runbook at
[0001-spike-fork-instructions.md](0001-spike-fork-instructions.md)
was executed by a maintainer with `CrashMediaIT` org admin rights,
and Step 8 of that runbook (wiring `[patch.crates-io]` to tag
`v0.5.4-cmremote.1` and adding the `[sources].allow-git` entry) has
landed against this repository.

## Authority

This approval constitutes maintainer gate #1 from ADR 0001 *Consequences* section.
