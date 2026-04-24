// Source: CMRemote, clean-room implementation.
//
// R7.f spike PoC tests. Each test exercises one of the symbol rows
// from `docs/decisions/0001-spike-report.md` end-to-end:
//
//   1. generate a fresh PKCS#8 key for the algorithm under test
//   2. construct the spike's `CryptoPrivateKey` from that PKCS#8 blob
//      (mirrors `webrtc-dtls`'s `from_pkcs8_*` site)
//   3. sign a message via `generate_key_signature` /
//      `generate_certificate_verify` (mirrors the upstream sign path)
//   4. verify it via `verify_signature` (mirrors the upstream verify
//      path), using the matching `VerificationAlgorithm` constant
//
// If the substitution from `ring` to `aws-lc-rs` had any shape gap
// for a given symbol, one of these tests would fail. They do not.

use cmremote_webrtc_crypto_spike::{
    generate_certificate_verify, generate_key_signature, public_key_bytes,
    verify_certificate_verify, verify_signature, CryptoPrivateKey, HashAlgorithm,
    SignatureAlgorithm, SignatureHashAlgorithm,
};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, Ed25519KeyPair, KeyPair, RsaKeyPair, UnparsedPublicKey, VerificationAlgorithm,
    ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P384_SHA384_ASN1,
    ECDSA_P384_SHA384_ASN1_SIGNING, ED25519, RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512,
};

/// Symbol exercised: `aws_lc_rs::rand::SystemRandom` (the
/// `ring::rand::SystemRandom` -> `aws_lc_rs::rand::SystemRandom`
/// row). If `SystemRandom` did not implement `SecureRandom` the way
/// the substitution claims, neither `EcdsaKeyPair::generate_pkcs8`
/// nor `EcdsaKeyPair::sign` below would compile.
#[test]
fn system_random_drives_ecdsa_p256_sign() {
    let rng = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).expect("generate");
    let key = CryptoPrivateKey::from_pkcs8_ecdsa_p256(pkcs8.as_ref().to_vec()).expect("import");
    let sig = generate_key_signature(b"hello", &key).expect("sign");
    assert!(!sig.is_empty(), "signature must not be empty");
}

/// Symbols exercised:
///   - `Ed25519KeyPair::from_pkcs8`
///   - `Ed25519KeyPair::sign(&msg)` (no rng — Ed25519 is deterministic)
///   - `ED25519` `VerificationAlgorithm` constant
///   - `UnparsedPublicKey::new(&ED25519, raw_pub)` + `verify`
#[test]
fn ed25519_sign_and_verify_round_trip() {
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate");
    let key = CryptoPrivateKey::from_pkcs8_ed25519(pkcs8.as_ref().to_vec()).expect("import");

    let msg = b"client_random || server_random || params";
    let sig = generate_key_signature(msg, &key).expect("sign");

    let pub_bytes = public_key_bytes(&key);
    verify_signature(
        msg,
        SignatureHashAlgorithm {
            // Ed25519 ignores the hash field upstream; the dispatch
            // is on `SignatureAlgorithm` alone.
            hash: HashAlgorithm::Sha256,
            signature: SignatureAlgorithm::Ed25519,
        },
        &sig,
        &pub_bytes,
    )
    .expect("verify");
}

/// Symbols exercised:
///   - `EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &der)`
///   - `EcdsaKeyPair::sign(&rng, &msg)` (returns ASN.1-DER ECDSA sig)
///   - `ECDSA_P256_SHA256_ASN1` `VerificationAlgorithm` constant
///   - `UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, uncompressed_pub)` + `verify`
#[test]
fn ecdsa_p256_asn1_sign_and_verify_round_trip() {
    let rng = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).expect("generate");
    let key = CryptoPrivateKey::from_pkcs8_ecdsa_p256(pkcs8.as_ref().to_vec()).expect("import");

    let msg = b"server_key_exchange.params";
    let sig = generate_key_signature(msg, &key).expect("sign");

    let pub_bytes = public_key_bytes(&key);
    verify_signature(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha256,
            signature: SignatureAlgorithm::Ecdsa,
        },
        &sig,
        &pub_bytes,
    )
    .expect("verify");
}

/// Symbol exercised: `ECDSA_P384_SHA384_ASN1`. The spike's
/// `CryptoPrivateKey` wrapper only models the algorithms
/// `webrtc-dtls` actually signs with (P-256, Ed25519, RSA), so this
/// test goes through `aws-lc-rs` directly to mint + sign with P-384,
/// then runs the verify path through the spike's `verify_signature`
/// which is the line `webrtc-dtls`'s `verify_signature` dispatches
/// on for the `Sha384 + Ecdsa` arm.
#[test]
fn ecdsa_p384_asn1_verify() {
    let rng = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, &rng).expect("generate");
    let kp =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, pkcs8.as_ref()).expect("import");

    let msg = b"server_key_exchange.params (p384)";
    let sig = kp.sign(&rng, msg).expect("sign").as_ref().to_vec();

    verify_signature(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha384,
            signature: SignatureAlgorithm::Ecdsa,
        },
        &sig,
        kp.public_key().as_ref(),
    )
    .expect("verify");
}

// -- RSA: shared test data ----------------------------------------
//
// PKCS#8 RSA key generation isn't exposed by `aws-lc-rs` directly
// across all 1.x versions, and the spike intentionally takes no
// extra feature dependency. A 2048-bit RSA test key in PKCS#8 DER
// is shipped as `rsa_test_key.hex` (test-only, regenerable from
// `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048`).

fn rsa_test_pkcs8() -> Vec<u8> {
    hex::decode(include_str!("rsa_test_key.hex").trim()).expect("valid hex")
}

/// Symbols exercised:
///   - `RsaKeyPair::from_pkcs8(&der)`
///   - `RsaKeyPair::public_modulus_len()`
///   - `RsaKeyPair::sign(&RSA_PKCS1_SHA256, &rng, msg, &mut sig)`
///   - `RSA_PKCS1_SHA256` (signing padding constant)
///   - `RSA_PKCS1_2048_8192_SHA256` (verification algorithm constant)
///   - `UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, rsa_pub_der)` + `verify`
#[test]
fn rsa_pkcs1_sha256_sign_and_verify_round_trip() {
    let key = CryptoPrivateKey::from_pkcs8_rsa(rsa_test_pkcs8()).expect("import");
    let msg = b"certificate_verify.handshake_messages";
    let sig = generate_certificate_verify(msg, &key).expect("sign");

    let pub_bytes = public_key_bytes(&key);
    verify_certificate_verify(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha256,
            signature: SignatureAlgorithm::Rsa,
        },
        &sig,
        &pub_bytes,
    )
    .expect("verify");
}

/// Symbol exercised: `RSA_PKCS1_2048_8192_SHA384`.
#[test]
fn rsa_pkcs1_sha384_verify() {
    use aws_lc_rs::signature::RSA_PKCS1_SHA384;
    let der = rsa_test_pkcs8();
    let kp = RsaKeyPair::from_pkcs8(&der).expect("import");
    let msg = b"sha384 cv";
    let mut sig = vec![0u8; kp.public_modulus_len()];
    let rng = SystemRandom::new();
    kp.sign(&RSA_PKCS1_SHA384, &rng, msg, &mut sig)
        .expect("sign");

    verify_signature(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha384,
            signature: SignatureAlgorithm::Rsa,
        },
        &sig,
        kp.public_key().as_ref(),
    )
    .expect("verify");
}

/// Symbol exercised: `RSA_PKCS1_2048_8192_SHA512`.
#[test]
fn rsa_pkcs1_sha512_verify() {
    use aws_lc_rs::signature::RSA_PKCS1_SHA512;
    let der = rsa_test_pkcs8();
    let kp = RsaKeyPair::from_pkcs8(&der).expect("import");
    let msg = b"sha512 cv";
    let mut sig = vec![0u8; kp.public_modulus_len()];
    let rng = SystemRandom::new();
    kp.sign(&RSA_PKCS1_SHA512, &rng, msg, &mut sig)
        .expect("sign");

    verify_signature(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha512,
            signature: SignatureAlgorithm::Rsa,
        },
        &sig,
        kp.public_key().as_ref(),
    )
    .expect("verify");
}

/// Symbol exercised:
/// `RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY`. `aws-lc-rs`
/// deliberately exposes legacy SHA-1 RSA only for *verification*
/// (not signing) — that's the same posture `ring` documents the
/// `_FOR_LEGACY_USE_ONLY` suffix for. Per the symbol-mapping
/// report, this is the verifier `webrtc-dtls`'s `verify_signature`
/// dispatches on for `Sha1 + Rsa` to handle SHA-1 RSA certificates
/// from older browsers.
///
/// To exercise the verifier without a sign path we use a
/// pre-generated SHA-1 RSA signature (produced with `openssl dgst
/// -sha1 -sign` against `tests/rsa_test_key.hex`) over a fixed
/// message, then check that the spike's `verify_signature` accepts
/// it via the legacy verifier constant.
#[test]
fn rsa_pkcs1_sha1_legacy_verify() {
    let der = rsa_test_pkcs8();
    let kp = RsaKeyPair::from_pkcs8(&der).expect("import");
    let msg = b"legacy sha1 cv";
    let sig = hex::decode(include_str!("rsa_sha1_sig.hex").trim()).expect("valid hex");

    verify_signature(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha1,
            signature: SignatureAlgorithm::Rsa,
        },
        &sig,
        kp.public_key().as_ref(),
    )
    .expect("verify");
}

/// Negative-path check: a flipped bit in the signature must fail
/// verification. Included so the round-trip tests can't pass
/// trivially (e.g. a verifier that always returns `Ok`).
#[test]
fn tampered_signature_is_rejected() {
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate");
    let key = CryptoPrivateKey::from_pkcs8_ed25519(pkcs8.as_ref().to_vec()).expect("import");

    let msg = b"detect tampering";
    let mut sig = generate_key_signature(msg, &key).expect("sign");
    sig[0] ^= 0x01;

    let pub_bytes = public_key_bytes(&key);
    let outcome = verify_signature(
        msg,
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha256,
            signature: SignatureAlgorithm::Ed25519,
        },
        &sig,
        &pub_bytes,
    );
    assert!(outcome.is_err(), "tampered signature must not verify");
}

/// `verify_signature` must reject an empty `subject_public_key` blob
/// with the same `LengthMismatch` error that `webrtc-dtls`'s
/// upstream `verify_signature` uses for an empty `raw_certificates`
/// slice. Keeps the spike's surface bug-compatible with upstream.
#[test]
fn empty_public_key_yields_length_mismatch() {
    let outcome = verify_signature(
        b"x",
        SignatureHashAlgorithm {
            hash: HashAlgorithm::Sha256,
            signature: SignatureAlgorithm::Ed25519,
        },
        b"sig",
        b"",
    );
    assert!(matches!(
        outcome,
        Err(cmremote_webrtc_crypto_spike::Error::LengthMismatch)
    ));
}

/// Type-level assertion: `&dyn aws_lc_rs::signature::VerificationAlgorithm`
/// is assignable from each of the algorithm constants the spike
/// substitutes for `ring`'s. If `aws-lc-rs` ever changed the trait
/// ergonomics this would stop compiling.
#[test]
fn verification_algorithm_trait_object_compiles_for_all_constants() {
    let _: &dyn VerificationAlgorithm = &ED25519;
    let _: &dyn VerificationAlgorithm = &ECDSA_P256_SHA256_ASN1;
    let _: &dyn VerificationAlgorithm = &ECDSA_P384_SHA384_ASN1;
    let _: &dyn VerificationAlgorithm = &RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY;
    let _: &dyn VerificationAlgorithm = &RSA_PKCS1_2048_8192_SHA256;
    let _: &dyn VerificationAlgorithm = &RSA_PKCS1_2048_8192_SHA384;
    let _: &dyn VerificationAlgorithm = &RSA_PKCS1_2048_8192_SHA512;

    // `UnparsedPublicKey::new(&dyn VerificationAlgorithm, &[u8])`
    // must accept any of them. Mirrors the upstream call site where
    // `verify_alg` is selected at runtime by `match`.
    let alg: &dyn VerificationAlgorithm = &ED25519;
    let _ = UnparsedPublicKey::new(alg, &[0u8; 32][..]);
}
