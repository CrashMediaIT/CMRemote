// Source: CMRemote, clean-room implementation.
//
// R7.f Option B feasibility-spike PoC.
//
// What this crate does, in one sentence: it takes every `ring` call
// site that `webrtc-rs/dtls@v0.5.4 src/crypto/mod.rs` makes, and
// re-points it at `aws-lc-rs` *with no other shape change*, so that
// `cargo test` proves each substitution round-trips key generation,
// signing, and verification.
//
// What this crate does NOT do: implement a WebRTC driver, ship a
// forked `webrtc-dtls`, or change `agent-rs/deny.toml`. The actual
// fork is created and consumed via `[patch.crates-io]` in a
// follow-up PR per `docs/decisions/0001-spike-fork-instructions.md`.
//
// Symbol coverage (every entry in the table in
// `docs/decisions/0001-spike-report.md` §"Direct symbols used by
// `webrtc-dtls` and their `aws-lc-rs` equivalents"):
//
//   ring::rand::SystemRandom                       -> aws_lc_rs::rand::SystemRandom
//   ring::signature::EcdsaKeyPair                  -> aws_lc_rs::signature::EcdsaKeyPair
//     ::from_pkcs8(alg, &der)                      -> identical
//     ::sign(&rng, &msg)                           -> identical
//   ring::signature::Ed25519KeyPair                -> aws_lc_rs::signature::Ed25519KeyPair
//     ::from_pkcs8(&der)                           -> identical
//     ::sign(&msg)                                 -> identical
//   ring::signature::RsaKeyPair                    -> aws_lc_rs::signature::RsaKeyPair
//     ::from_pkcs8(&der)                           -> identical
//     ::public_modulus_len()                       -> identical
//     ::sign(padding, &rng, &msg, &mut sig)        -> identical
//   ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING -> aws_lc_rs::signature::*
//   ring::signature::ECDSA_P256_SHA256_ASN1
//   ring::signature::ECDSA_P384_SHA384_ASN1
//   ring::signature::ED25519
//   ring::signature::RSA_PKCS1_SHA256
//   ring::signature::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY
//   ring::signature::RSA_PKCS1_2048_8192_SHA256
//   ring::signature::RSA_PKCS1_2048_8192_SHA384
//   ring::signature::RSA_PKCS1_2048_8192_SHA512
//   &dyn ring::signature::VerificationAlgorithm    -> &dyn aws_lc_rs::signature::VerificationAlgorithm
//   ring::signature::UnparsedPublicKey::new(..)    -> identical (verify(msg, sig))
//
// Each of the public functions below mirrors a free function /
// constructor / impl in `webrtc-dtls/src/crypto/mod.rs` line-for-line
// — the only difference is the `use` lines at the top of this file.
// That's the load-bearing claim of the spike.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, Ed25519KeyPair, KeyPair, RsaKeyPair, UnparsedPublicKey, VerificationAlgorithm,
    ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P384_SHA384_ASN1, ED25519,
    RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY, RSA_PKCS1_2048_8192_SHA256,
    RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512, RSA_PKCS1_SHA256,
};

/// Mirrors `webrtc-dtls`'s
/// [`SignatureHashAlgorithm::hash`](https://github.com/webrtc-rs/dtls/blob/v0.5.4/src/signature_hash_algorithm.rs)
/// enum, restricted to the variants `webrtc-dtls`'s `verify_signature`
/// dispatches on. Naming kept identical so the diff against upstream
/// is mechanical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

/// Same source as `HashAlgorithm` above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Ed25519,
    Ecdsa,
    Rsa,
}

/// Mirrors the `SignatureHashAlgorithm` struct used by
/// `webrtc-dtls::crypto::verify_signature`.
#[derive(Debug, Clone, Copy)]
pub struct SignatureHashAlgorithm {
    pub hash: HashAlgorithm,
    pub signature: SignatureAlgorithm,
}

/// Mirrors `webrtc-dtls`'s `Error` enum at the granularity the
/// `crypto` module needs: every `ring` call-site error is folded into
/// `Error::Other(String)` via `e.to_string()` upstream, and we keep
/// that contract here so the substitution stays mechanical.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported key_pair")]
    UnsupportedKeyPair,
    #[error("unimplemented signature algorithm / hash combination")]
    KeySignatureVerifyUnimplemented,
    #[error("input is empty")]
    LengthMismatch,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The set of private-key kinds `webrtc-dtls`'s
/// `CryptoPrivateKeyKind` uses. Same names, same shapes, same `ring`
/// types — except every type comes from `aws-lc-rs`.
pub enum CryptoPrivateKeyKind {
    Ed25519(Ed25519KeyPair),
    Ecdsa256(EcdsaKeyPair),
    Rsa256(RsaKeyPair),
}

/// Mirrors `webrtc-dtls`'s `CryptoPrivateKey`.
pub struct CryptoPrivateKey {
    pub kind: CryptoPrivateKeyKind,
    pub serialized_der: Vec<u8>,
}

impl CryptoPrivateKey {
    /// Reconstruct a private-key wrapper from a PKCS#8 DER blob.
    /// Mirrors the `from_key_pair` branches in
    /// `webrtc-dtls/src/crypto/mod.rs` — the only difference is that
    /// the caller already classified the algorithm (the upstream
    /// version uses `rcgen::KeyPair::is_compatible(...)` for that;
    /// in this PoC the caller is the test harness).
    pub fn from_pkcs8_ed25519(der: Vec<u8>) -> Result<Self> {
        let kp = Ed25519KeyPair::from_pkcs8(&der).map_err(|e| Error::Other(e.to_string()))?;
        Ok(Self {
            kind: CryptoPrivateKeyKind::Ed25519(kp),
            serialized_der: der,
        })
    }

    pub fn from_pkcs8_ecdsa_p256(der: Vec<u8>) -> Result<Self> {
        let kp = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &der)
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok(Self {
            kind: CryptoPrivateKeyKind::Ecdsa256(kp),
            serialized_der: der,
        })
    }

    pub fn from_pkcs8_rsa(der: Vec<u8>) -> Result<Self> {
        let kp = RsaKeyPair::from_pkcs8(&der).map_err(|e| Error::Other(e.to_string()))?;
        Ok(Self {
            kind: CryptoPrivateKeyKind::Rsa256(kp),
            serialized_der: der,
        })
    }
}

/// Line-for-line port of
/// `webrtc-dtls/src/crypto/mod.rs::generate_key_signature` (used to
/// sign the `ServerKeyExchange.params` blob). Every closure body is
/// identical to the upstream version with `ring::` swapped for
/// `aws_lc_rs::`. The function body below is what would land in the
/// fork's `crypto/mod.rs` after the substitution.
pub fn generate_key_signature(message: &[u8], private_key: &CryptoPrivateKey) -> Result<Vec<u8>> {
    let signature = match &private_key.kind {
        CryptoPrivateKeyKind::Ed25519(kp) => kp.sign(message).as_ref().to_vec(),
        CryptoPrivateKeyKind::Ecdsa256(kp) => {
            let system_random = SystemRandom::new();
            kp.sign(&system_random, message)
                .map_err(|e| Error::Other(e.to_string()))?
                .as_ref()
                .to_vec()
        }
        CryptoPrivateKeyKind::Rsa256(kp) => {
            let system_random = SystemRandom::new();
            let mut signature = vec![0; kp.public_modulus_len()];
            kp.sign(&RSA_PKCS1_SHA256, &system_random, message, &mut signature)
                .map_err(|e| Error::Other(e.to_string()))?;
            signature
        }
    };
    Ok(signature)
}

/// Line-for-line port of
/// `webrtc-dtls/src/crypto/mod.rs::generate_certificate_verify`.
/// Identical body to `generate_key_signature` upstream — kept
/// separate to preserve the mechanical-diff property.
pub fn generate_certificate_verify(
    handshake_bodies: &[u8],
    private_key: &CryptoPrivateKey,
) -> Result<Vec<u8>> {
    generate_key_signature(handshake_bodies, private_key)
}

/// Line-for-line port of
/// `webrtc-dtls/src/crypto/mod.rs::verify_signature`. Picks an
/// algorithm constant by `(hash, signature)` pair and runs
/// `UnparsedPublicKey::verify`. The `subject_public_key.data` blob
/// is supplied by the caller (upstream extracts it via `x509-parser`;
/// here it's an explicit argument so the spike doesn't drag in an
/// X.509 parser).
pub fn verify_signature(
    message: &[u8],
    hash_algorithm: SignatureHashAlgorithm,
    remote_key_signature: &[u8],
    subject_public_key: &[u8],
) -> Result<()> {
    if subject_public_key.is_empty() {
        return Err(Error::LengthMismatch);
    }

    let verify_alg: &dyn VerificationAlgorithm = match hash_algorithm.signature {
        SignatureAlgorithm::Ed25519 => &ED25519,
        SignatureAlgorithm::Ecdsa if hash_algorithm.hash == HashAlgorithm::Sha256 => {
            &ECDSA_P256_SHA256_ASN1
        }
        SignatureAlgorithm::Ecdsa if hash_algorithm.hash == HashAlgorithm::Sha384 => {
            &ECDSA_P384_SHA384_ASN1
        }
        SignatureAlgorithm::Rsa if hash_algorithm.hash == HashAlgorithm::Sha1 => {
            &RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY
        }
        SignatureAlgorithm::Rsa if hash_algorithm.hash == HashAlgorithm::Sha256 => {
            &RSA_PKCS1_2048_8192_SHA256
        }
        SignatureAlgorithm::Rsa if hash_algorithm.hash == HashAlgorithm::Sha384 => {
            &RSA_PKCS1_2048_8192_SHA384
        }
        SignatureAlgorithm::Rsa if hash_algorithm.hash == HashAlgorithm::Sha512 => {
            &RSA_PKCS1_2048_8192_SHA512
        }
        _ => return Err(Error::KeySignatureVerifyUnimplemented),
    };

    let public_key = UnparsedPublicKey::new(verify_alg, subject_public_key);
    public_key
        .verify(message, remote_key_signature)
        .map_err(|e| Error::Other(e.to_string()))?;
    Ok(())
}

/// Line-for-line port of
/// `webrtc-dtls/src/crypto/mod.rs::verify_certificate_verify`.
/// Forwards to `verify_signature` upstream, same here.
pub fn verify_certificate_verify(
    handshake_bodies: &[u8],
    hash_algorithm: SignatureHashAlgorithm,
    remote_key_signature: &[u8],
    subject_public_key: &[u8],
) -> Result<()> {
    verify_signature(
        handshake_bodies,
        hash_algorithm,
        remote_key_signature,
        subject_public_key,
    )
}

/// Re-export of the public-key bytes for a given private key. Upstream
/// `webrtc-dtls` reads this off the certificate; in the spike we pull
/// it directly from the key pair so the round-trip test does not need
/// an X.509 codec. The `aws_lc_rs::signature::KeyPair::public_key`
/// surface is identical to `ring`'s.
pub fn public_key_bytes(private_key: &CryptoPrivateKey) -> Vec<u8> {
    match &private_key.kind {
        CryptoPrivateKeyKind::Ed25519(kp) => kp.public_key().as_ref().to_vec(),
        CryptoPrivateKeyKind::Ecdsa256(kp) => kp.public_key().as_ref().to_vec(),
        CryptoPrivateKeyKind::Rsa256(kp) => {
            // Upstream `webrtc-dtls` does not call this branch — RSA
            // certs come from the cert chain, not from the key pair —
            // but we expose it for symmetry with the other two arms
            // so the round-trip test can verify what it just signed.
            kp.public_key().as_ref().to_vec()
        }
    }
}
