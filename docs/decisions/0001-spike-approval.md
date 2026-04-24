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

## Authority

This approval constitutes maintainer gate #1 from ADR 0001 *Consequences* section.
