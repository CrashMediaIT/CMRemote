# `cmremote-webrtc-crypto-spike`

**R7.f Option B feasibility-spike PoC — running-code evidence for
the `ring` → `aws-lc-rs` symbol mapping.**

This crate is the runnable counterpart to the spike's symbol-mapping
report. It ports the exact `ring` call sites that
[`webrtc-rs/dtls@v0.5.4 src/crypto/mod.rs`](https://github.com/webrtc-rs/dtls/blob/v0.5.4/src/crypto/mod.rs)
makes onto `aws-lc-rs`, and proves with `cargo test` that every
distinct symbol from the report round-trips end-to-end (key import,
sign, verify) with no shape change beyond the `use` lines at the
top of the file.

```bash
# From the agent-rs workspace root:
cargo test -p cmremote-webrtc-crypto-spike
# Expected: 11/11 passed.
```

## Why this exists

The parent ADR
([`docs/decisions/0001-webrtc-crypto-provider.md`](../../../docs/decisions/0001-webrtc-crypto-provider.md))
accepted **Option B (fork `webrtc` onto `aws-lc-rs`)**. The
feasibility spike approved under
[`docs/decisions/0001-spike-approval.md`](../../../docs/decisions/0001-spike-approval.md)
required three deliverables:

1. **Symbol-mapping report** —
   [`docs/decisions/0001-spike-report.md`](../../../docs/decisions/0001-spike-report.md).
   Done.
2. **PoC with green CI demonstrating the substitution works** —
   *this crate*. Done.
3. **Go / no-go recommendation** — recorded in the spike report
   (GO) and accepted at gate #2 in the parent ADR.

This crate is **not a WebRTC driver**, **not a fork of
`webrtc-dtls`**, and **does not modify
[`agent-rs/deny.toml`](../../deny.toml)**. The actual fork lives in
the to-be-created `CrashMediaIT/webrtc-cmremote` repository per
[`docs/decisions/0001-spike-fork-instructions.md`](../../../docs/decisions/0001-spike-fork-instructions.md).
Once that fork is wired into `agent-rs/Cargo.toml` via
`[patch.crates-io]`, this PoC crate is **deletable** — its purpose
is to preserve, in this repo's git history, the evidence that the
substitution compiles and the round-trips pass.

## Symbol coverage

Every `ring` symbol enumerated in the spike report is exercised by
at least one test in `tests/round_trip.rs`:

| `ring` symbol                                           | Test                                                       |
| ------------------------------------------------------- | ---------------------------------------------------------- |
| `ring::rand::SystemRandom`                              | `system_random_drives_ecdsa_p256_sign`                     |
| `ring::signature::Ed25519KeyPair::{from_pkcs8, sign}`   | `ed25519_sign_and_verify_round_trip`                       |
| `ring::signature::EcdsaKeyPair::{from_pkcs8, sign}`     | `ecdsa_p256_asn1_sign_and_verify_round_trip`               |
| `ring::signature::RsaKeyPair::{from_pkcs8, sign, public_modulus_len}` | `rsa_pkcs1_sha256_sign_and_verify_round_trip`    |
| `ECDSA_P256_SHA256_ASN1_SIGNING`                        | `ecdsa_p256_asn1_sign_and_verify_round_trip`               |
| `ECDSA_P256_SHA256_ASN1`                                | `ecdsa_p256_asn1_sign_and_verify_round_trip`               |
| `ECDSA_P384_SHA384_ASN1`                                | `ecdsa_p384_asn1_verify`                                   |
| `ED25519`                                               | `ed25519_sign_and_verify_round_trip`                       |
| `RSA_PKCS1_SHA256` (signing padding)                    | `rsa_pkcs1_sha256_sign_and_verify_round_trip`              |
| `RSA_PKCS1_2048_8192_SHA256`                            | `rsa_pkcs1_sha256_sign_and_verify_round_trip`              |
| `RSA_PKCS1_2048_8192_SHA384`                            | `rsa_pkcs1_sha384_verify`                                  |
| `RSA_PKCS1_2048_8192_SHA512`                            | `rsa_pkcs1_sha512_verify`                                  |
| `RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY`          | `rsa_pkcs1_sha1_legacy_verify`                             |
| `&dyn VerificationAlgorithm` + `UnparsedPublicKey`      | `verification_algorithm_trait_object_compiles_for_all_constants` |

Plus two negative-path tests that prove the verifier isn't a
no-op (`tampered_signature_is_rejected`,
`empty_public_key_yields_length_mismatch`).

## Test-only files

- `tests/rsa_test_key.hex` — a freshly-generated 2048-bit RSA test
  key in PKCS#8 DER (hex-encoded). Test-only; never used outside
  this crate. Regenerable with the openssl one-liner documented in
  `tests/round_trip.rs`.
- `tests/rsa_sha1_sig.hex` — pre-computed SHA-1 RSA signature over
  the message `"legacy sha1 cv"` using the test key above. `aws-lc-rs`
  intentionally does not expose SHA-1 RSA signing (only verification,
  for legacy compatibility — same posture `ring` documents), so the
  SHA-1 verify test takes its signature from openssl rather than
  generating one in-process.

## Note on `aws-lc-rs` reachability

`aws-lc-rs` is already in the workspace lock-file as the rustls
crypto provider since slice R2. Adding it as a direct dep here
adds zero new transitive surface beyond the
[`hex`](https://crates.io/crates/hex) dev-dep this crate uses to
decode the test fixtures. The `[bans].deny` entry on `ring` in
`agent-rs/deny.toml` is not affected: this crate uses `aws-lc-rs`
only.
