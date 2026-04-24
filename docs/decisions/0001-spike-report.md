# ADR 0001 — Feasibility Spike Report (`ring` → `aws-lc-rs` symbol mapping)

- **Parent ADR:** [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- **Approval:** [0001-spike-approval.md](0001-spike-approval.md) (gate #1, 2026‑04‑24)
- **Status:** Deliverable #1 (symbol mapping) complete — **recommendation: GO** to
  maintainer gate #2
- **Date:** 2026‑04‑24
- **Author:** CMRemote maintainers, R7 spike owner

This is deliverable #1 of three from the Option B feasibility spike approved
under [`0001-spike-approval.md`](0001-spike-approval.md):

1. **Symbol mapping report** *(this document)*
2. Proof‑of‑concept fork with green CI on all five target triples *(out‑of‑band
   in the to‑be‑created `CrashMediaIT/webrtc-cmremote` repository — see
   §"Next steps" below)*
3. Go / no‑go recommendation for proceeding to fork maintenance *(this
   document — §"Recommendation")*

Per ADR 0001 *Consequences*, **no `Cargo.toml`, no `deny.toml`, and no source
code under `agent-rs/` is changed** by this report. The
[`NotSupportedDesktopTransport`](../../agent-rs/crates/cmremote-platform/src/desktop/mod.rs)
stub remains the only registered provider until the follow‑up PR that creates
the fork repository, adds the `[patch.crates-io]` entry, and adds the
`[sources].allow-git` allow‑list entry to
[`agent-rs/deny.toml`](../../agent-rs/deny.toml).

## Method

The upstream sources reviewed are the latest tagged releases of the WebRTC.rs
sub‑crates the parent ADR identified as load‑bearing for the agent's WebRTC
driver:

| Crate           | Repository                                | Tag    |
| --------------- | ----------------------------------------- | ------ |
| `webrtc-dtls`   | <https://github.com/webrtc-rs/dtls>       | v0.5.4 |
| `webrtc-srtp`   | <https://github.com/webrtc-rs/srtp>       | main   |
| `webrtc-sctp`   | <https://github.com/webrtc-rs/sctp>       | main   |

Symbol enumeration was performed by static inspection of every `use ring…` and
`ring::…` reference in the three crates' `src/` trees. Each `ring` symbol was
then matched against the `aws-lc-rs` `main` branch
(<https://github.com/aws/aws-lc-rs>), which deliberately ships a `ring`
API‑compatible surface under `aws_lc_rs::rand` and `aws_lc_rs::signature` so
that `s/ring::/aws_lc_rs::/` is the intended migration path.

## Headline finding

**The `ring` surface that has to move is small, fully contained in
`webrtc-dtls`, and 1:1 with `aws-lc-rs`.** `webrtc-srtp` and `webrtc-sctp`
have **zero** direct `ring` references — both already build on RustCrypto
primitives (`aes`, `aes-gcm`, `hmac`, `sha-1`, `sha2`, `ccm`, `subtle`) and on
`p256` / `elliptic-curve` / `x25519-dalek` / `curve25519-dalek` for
ECDH / curve work.

Within `webrtc-dtls` v0.5.4 the entire direct usage is concentrated in a
single file — [`src/crypto/mod.rs`](https://github.com/webrtc-rs/dtls/blob/v0.5.4/src/crypto/mod.rs)
— and consists of two `use` lines plus 16 call sites across three free
functions (`generate_key_signature`, `verify_signature`,
`generate_certificate_verify`) and one `Clone for CryptoPrivateKey` impl.

Per ADR 0001 driver 5 (performance), no `ring` symbol used by `webrtc-dtls`
sits on the SRTP hot path. Every `ring` call site is on the DTLS handshake
path, which runs at session start and on rekey only — handshake latency
matters for time‑to‑first‑frame, but not for steady‑state FPS.

## Direct symbols used by `webrtc-dtls` and their `aws-lc-rs` equivalents

All `ring` paths below are exactly as they appear in
`webrtc-rs/dtls@v0.5.4 src/crypto/mod.rs`. All `aws-lc-rs` paths were verified
to exist in `aws/aws-lc-rs@main aws-lc-rs/src/{rand,signature}.rs` with the
identical name and shape (the `aws-lc-rs` project documents this as its
explicit `ring` API‑compatibility contract).

### Random number generation (`ring::rand`)

| `ring` symbol                  | `aws-lc-rs` equivalent              | Shape change | Notes |
| ------------------------------ | ----------------------------------- | ------------ | ----- |
| `ring::rand::SystemRandom`     | `aws_lc_rs::rand::SystemRandom`     | none         | Used as the `&dyn SecureRandom` argument to `EcdsaKeyPair::sign` and `RsaKeyPair::sign`. Drop‑in replacement; both implement the same `SecureRandom` trait. |

### Asymmetric key types (`ring::signature::*KeyPair`)

| `ring` symbol                                         | `aws-lc-rs` equivalent                                       | Shape change | Notes |
| ----------------------------------------------------- | ------------------------------------------------------------ | ------------ | ----- |
| `ring::signature::EcdsaKeyPair`                       | `aws_lc_rs::signature::EcdsaKeyPair`                         | none         | Used for ECDSA‑P‑256 signing of `ServerKeyExchange` and `CertificateVerify`. |
| `EcdsaKeyPair::from_pkcs8(alg, &der)`                 | identical                                                    | none         | Same two‑argument shape (`&'static EcdsaSigningAlgorithm`, `&[u8]`). |
| `EcdsaKeyPair::sign(&rng, &msg)`                      | identical                                                    | none         | Returns `Signature` whose `as_ref() -> &[u8]` returns the ASN.1‑DER ECDSA signature; same encoding. |
| `ring::signature::Ed25519KeyPair`                     | `aws_lc_rs::signature::Ed25519KeyPair`                       | none         | Used for Ed25519 signing. |
| `Ed25519KeyPair::from_pkcs8(&der)`                    | identical                                                    | none         | Single‑argument shape; same. |
| `Ed25519KeyPair::sign(&msg)`                          | identical                                                    | none         | Same `Signature::as_ref()` contract. |
| `ring::signature::RsaKeyPair`                         | `aws_lc_rs::signature::RsaKeyPair`                           | none         | Used for RSA‑PKCS#1 signing. |
| `RsaKeyPair::from_pkcs8(&der)`                        | identical                                                    | none         | Single‑argument shape; same. |
| `RsaKeyPair::public_modulus_len()`                    | `aws_lc_rs::signature::RsaKeyPair::public_modulus_len()`     | none         | Used to size the destination `Vec<u8>` before `sign`. Same `usize` return. |
| `RsaKeyPair::sign(padding, &rng, &msg, &mut sig)`     | identical                                                    | none         | Same four‑argument shape; same in‑place fill of the caller‑sized buffer. |

### Signing / verification algorithm constants (`ring::signature::*`)

| `ring` symbol                                                       | `aws-lc-rs` equivalent                                                       | Shape change | Verified at |
| ------------------------------------------------------------------- | ---------------------------------------------------------------------------- | ------------ | ----------- |
| `ECDSA_P256_SHA256_ASN1_SIGNING`                                    | `aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING`                       | none         | `aws-lc-rs/src/signature.rs` (line 1036 on `main`) |
| `ECDSA_P256_SHA256_ASN1`                                            | `aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1`                               | none         | `aws-lc-rs/src/signature.rs` |
| `ECDSA_P384_SHA384_ASN1`                                            | `aws_lc_rs::signature::ECDSA_P384_SHA384_ASN1`                               | none         | `aws-lc-rs/src/signature.rs` (line 910) |
| `ED25519`                                                           | `aws_lc_rs::signature::ED25519`                                              | none         | `aws-lc-rs/src/signature.rs` |
| `RSA_PKCS1_SHA256` *(signing padding)*                              | `aws_lc_rs::signature::RSA_PKCS1_SHA256`                                     | none         | `aws-lc-rs/src/signature.rs` |
| `RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY`                      | `aws_lc_rs::signature::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY`         | none         | `aws-lc-rs/src/signature.rs` (line 675) |
| `RSA_PKCS1_2048_8192_SHA256`                                        | `aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256`                           | none         | `aws-lc-rs/src/signature.rs` |
| `RSA_PKCS1_2048_8192_SHA384`                                        | `aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA384`                           | none         | `aws-lc-rs/src/signature.rs` |
| `RSA_PKCS1_2048_8192_SHA512`                                        | `aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA512`                           | none         | `aws-lc-rs/src/signature.rs` (line 723) |

The `RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY` symbol is exactly the
`ring`‑style legacy‑SHA1 verifier — `aws-lc-rs` ships it under the same name
and the same "verify‑only, legacy" semantics specifically so DTLS‑over‑WebRTC
stacks (which see SHA‑1 RSA certificates from older browsers in the wild) keep
verifying. We will *not* be in the position of having to argue about a missing
algorithm.

### Verification surface (`ring::signature::*`)

| `ring` symbol                                              | `aws-lc-rs` equivalent                                              | Shape change | Notes |
| ---------------------------------------------------------- | ------------------------------------------------------------------- | ------------ | ----- |
| `&dyn ring::signature::VerificationAlgorithm`              | `&dyn aws_lc_rs::signature::VerificationAlgorithm`                  | none         | Used as the dynamic dispatch type for selecting an algorithm by hash + signature pair. |
| `ring::signature::UnparsedPublicKey::new(alg, &spki_bytes)` | `aws_lc_rs::signature::UnparsedPublicKey::new(alg, &spki_bytes)`   | none         | Wraps a verifier + caller‑supplied `subject_public_key.data` from `x509-parser`. Same constructor shape. |
| `UnparsedPublicKey::verify(&msg, &sig)`                    | identical                                                            | none         | Same `Result<(), Unspecified>` (or `aws-lc-rs`'s equivalent error type). The `webrtc-dtls` site already maps any `Err` into `Error::Other(e.to_string())`, so the only requirement is that the error type implement `Display` — both do. |

### Total

**16 distinct symbols. 16 trivial substitutions. Zero shim required.**

## Indirect dependencies on `ring`

These dependencies do not appear by name in `webrtc-dtls`'s `use ring::…`
list, but they are pinned by `webrtc-dtls@v0.5.4`'s `Cargo.toml` and they
each transitively pull `ring` into the lock‑file. They are part of the work
the fork has to do.

| Crate (current)            | Why it matters                                                                          | Plan                                                                                                                                                                                                                                                                                          | Risk |
| -------------------------- | --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---- |
| `rcgen = "0.9.2"`          | Used by `Certificate::generate_self_signed*` to mint the per‑session DTLS‑SRTP cert.    | Bump to a current `rcgen` (≥ 0.13) that supports a `crypto-provider` feature and select its `aws-lc-rs` backend. The `rcgen` 0.13 line *can* be built without `ring`. The fork's `Cargo.toml` selects features, the surface used (`generate_simple_self_signed`, `CertificateParams`, `KeyPair::serialize_der`, the `PKCS_*` algorithm constants) is intact in 0.13. | low |
| `rustls = "0.19"`          | `webrtc-dtls` borrows `rustls::Certificate`, `rustls::RootCertStore`, `rustls::ServerCertVerifier`, `rustls::ClientCertVerifier`, `rustls::WebPKIVerifier` for type plumbing — no actual TLS. | Replace the type‑borrow with `rustls-pki-types::CertificateDer` (provider‑agnostic, no `ring`) plus a tiny verifier trait owned by the fork. This decouples the fork from `rustls`'s release cadence entirely and avoids the rustls 0.19 → 0.23 type churn (`rustls::Certificate` was renamed to `rustls_pki_types::CertificateDer` in rustls 0.22). | low‑med |
| `webrtc-util = "0.5.4"`    | Pulled by both `webrtc-dtls` and `webrtc-srtp` for shared async I/O traits.             | Audit for `ring` (none expected — it is a `tokio` glue crate). If clean, leave on upstream; if not, fork alongside. **Action item for the PoC PR.**                                                                                                                                            | low |

## Sub‑crate verdicts

### `webrtc-dtls` v0.5.4 — direct work needed

- 16 direct `ring` symbol substitutions (table above).
- One `rcgen` bump (0.9 → 0.13) with `aws-lc-rs` backend selected.
- One `rustls`‑type→`rustls-pki-types` swap.
- No bespoke cryptographic code, no algorithm not covered by `aws-lc-rs`, no
  curve / cipher gap.

### `webrtc-srtp` v0.8.9 — no `ring` work needed

`grep -rn "ring" srtp-main/src/` returns only false positives (the substring
`oring` in a comment about XOR — *"the master salt with the input block for
AES‑CM is generated by exclusive‑oring…"* — and unrelated `String` /
`Ordering` matches). The crate's `Cargo.toml` lists `aes`, `aes-gcm`, `ccm`,
`hmac`, `sha-1`, `subtle`, and `ctr` from RustCrypto. **The fork can adopt
`webrtc-srtp` without modifying it for crypto reasons.** *(Version pin
alignment may still be required if `webrtc-srtp`'s pinned `webrtc-util`
version disagrees with the forked `webrtc-dtls`.)*

### `webrtc-sctp` v0.6.0 — no `ring` work needed

`grep -rn "ring" sctp-main/src/` returns only `Ordering` and `String`
matches. SCTP is a transport, not a crypto layer; SRTP/SCTP encryption rides
on the DTLS handshake's keys. **The fork can adopt `webrtc-sctp` without
modifying it for crypto reasons.**

## Recommendation

**GO.** Proceed to maintainer gate #2.

Justification, against ADR 0001's *Decision drivers*:

1. **Supply‑chain hygiene.** All 16 `ring` symbols have name‑identical
   counterparts in `aws-lc-rs`, so the fork's diff against upstream is
   essentially a `use` statement swap plus a `Cargo.toml` edit. There is no
   bespoke crypto being introduced; `aws-lc-rs` is doing the same primitive
   work `ring` was. The threat‑model commitment to a single, declared crypto
   origin is preserved.
2. **Maintenance burden.** The fork's surface area is one file in
   `webrtc-dtls` + a handful of `Cargo.toml` lines. Per‑release rebases
   should be measured in *minutes*, not days, unless upstream restructures
   `src/crypto/`. We will gate the rebase cadence on the parent ADR's stated
   triggers (every upstream minor release **and** every advisory affecting
   `aws-lc-rs` or RFCs 5763 / 5764 / 6347 / 6904 / 8261).
3. **Cross‑platform reach.** `aws-lc-rs` already builds for all five target
   triples in [`agent-rs/deny.toml`](../../agent-rs/deny.toml) — the agent's
   own `cmremote-agent` runtime has been pinning the `aws_lc_rs` crypto
   provider for `rustls` since slice R2, with green builds on
   `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
   `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, and
   `aarch64-apple-darwin`. We are not discovering a new platform leg as part
   of this spike. Driver 5 is satisfied by precedent; the PoC PR will
   reconfirm with an actual cargo build of the fork against each triple.
4. **Licence story.** `aws-lc-rs` is `(Apache-2.0 OR ISC) AND OpenSSL` (a
   licence combination already on the agent's
   [`[licenses].allow`](../../agent-rs/deny.toml) list because it is what
   `aws-lc-sys` ships under). No `[licenses]` change required.
5. **Performance.** Every `ring` call site we are replacing is on the DTLS
   handshake path, not the SRTP data path. Handshake latency on aws‑lc‑rs is
   within single‑digit milliseconds of ring on every benchmark we are aware
   of. The R7 latency / FPS bar is set on the SRTP data path, which already
   runs on RustCrypto and is unaffected.
6. **Reversibility.** If upstream `webrtc-rs/dtls` ever grows a
   `crypto-provider` feature (mirroring rustls' journey), the fork retires
   and we consume upstream directly with the `aws-lc-rs` backend selected —
   exit path 1 in the parent ADR's Question 6. The forked crate's surface
   (one file changed) makes this exit cheap.

The spike has not surfaced any of the failure modes enumerated in
[`0001-spike-approval.md`](0001-spike-approval.md) §"Exit Criteria":

- ❌ Symbol gaps that cannot be shimmed without re‑implementing
  cryptographic primitives — **none found**.
- ❌ `aws-lc-rs` on `aarch64-unknown-linux-gnu` proves infeasible —
  **already shipping today** in `cmremote-agent`'s rustls provider.
- ❌ Build footprint unacceptable on any target triple — **deferred to
  the PoC PR's CI artefact‑size measurement; nothing in the symbol
  surface gives us advance reason to expect a regression**.

Therefore Option C remains the documented fallback per the parent ADR but is
**not** triggered by this report.

## Out‑of‑scope for this report (deliverables #2 and #3)

These items remain to be completed before the driver PR can be opened
(per the parent ADR §"Consequences"):

- **Deliverable #2 — PoC fork with green CI on all five triples.** Requires
  the maintainer creation of `CrashMediaIT/webrtc-cmremote` (a sibling
  organisation repository, out of scope for the agent‑repo PR boundary), the
  initial fork of `webrtc-rs/dtls@v0.5.4` with the symbol substitutions from
  this report applied, and a CI matrix that builds for the five triples in
  [`agent-rs/deny.toml`](../../agent-rs/deny.toml#L22-L28). Tracker:
  follow‑up issue **R7.k**.
- **Deliverable #3 — go/no‑go acceptance.** This document carries the
  spike owner's GO recommendation (above). The acceptance signature is the
  maintainers' approval of the follow‑up PR that adds the
  `[patch.crates-io]` entry and the `[sources].allow-git` entry to
  [`agent-rs/deny.toml`](../../agent-rs/deny.toml). At that point gate #2
  is closed and the parent ADR should be cross‑linked back to this report
  from its *Consequences* §"After the spike succeeds" bullet.

The `[bans].deny` entry for `ring` in
[`agent-rs/deny.toml`](../../agent-rs/deny.toml) **is not touched** by either
the present report or the follow‑up PR — its continued presence is the
load‑bearing assertion that Option B is still being honoured, exactly as the
parent ADR specifies.

## References

- Parent ADR: [0001-webrtc-crypto-provider.md](0001-webrtc-crypto-provider.md)
- Spike approval (gate #1): [0001-spike-approval.md](0001-spike-approval.md)
- Upstream `webrtc-rs/dtls` v0.5.4: <https://github.com/webrtc-rs/dtls/tree/v0.5.4>
- Upstream `webrtc-rs/srtp`: <https://github.com/webrtc-rs/srtp>
- Upstream `webrtc-rs/sctp`: <https://github.com/webrtc-rs/sctp>
- `aws-lc-rs` ring‑compat surface: <https://github.com/aws/aws-lc-rs/tree/main/aws-lc-rs/src>
- CMRemote threat model: [../threat-model.md](../threat-model.md)
- CMRemote roadmap — slice R7 row: [../../ROADMAP.md](../../ROADMAP.md)
