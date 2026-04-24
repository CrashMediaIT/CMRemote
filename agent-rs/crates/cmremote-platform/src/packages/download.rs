// Source: CMRemote, clean-room implementation.

//! Artifact-download abstraction used by the MSI / Executable package
//! providers and the agent self-update handler.
//!
//! Slice R6 ships the trait + a deliberately rejecting default
//! implementation; the concrete `reqwest`-based downloader (rustls
//! only, no `ring`, no `openssl-sys` per `deny.toml`) is the very next
//! follow-up so the runtime can wire a real client into the same
//! providers without further wire/contract churn.
//!
//! ## Security contract
//!
//! Every implementation MUST:
//!
//! 1. Refuse anything that is not `https://`. The signed-build
//!    pipeline (slice R8) only ever mints `https://` URLs; a
//!    `http://` URL on the wire is treated as adversarial.
//! 2. Cap the download at `max_bytes`. The agent has bounded RAM and
//!    unbounded fetches are how a malicious server fills the disk.
//! 3. Stream to disk under the supplied cache directory; never
//!    materialise the full artifact in RAM.
//! 4. Never echo the supplied `auth_header` value into logs or error
//!    messages. The header carries a short-lived token which must
//!    not leak via a stack trace.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

/// HTTPS-only artifact download request.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// Fully-qualified `https://…` URL produced by the publisher
    /// manifest or the server's signed-MSI URL minter.
    pub url: String,
    /// Optional header presented by the agent during the fetch (e.g.
    /// the `X-Expiring-Token` signed-MSI header). The value MUST NOT
    /// be logged.
    pub auth_header: Option<(String, String)>,
    /// Hard cap on the response body. Hit ⇒ download fails, partial
    /// file is deleted before returning.
    pub max_bytes: u64,
    /// Hard wall-clock deadline for the entire fetch.
    pub timeout: Duration,
    /// Directory under which the downloader writes the artifact. The
    /// caller is responsible for creating it (chmod 0700 on Unix).
    pub destination_dir: PathBuf,
    /// On-disk leaf name. Has already been validated by the caller via
    /// `is_safe_msi_file_name` or equivalent.
    pub file_name: String,
}

/// Outcome of a successful [`DownloadRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedArtifact {
    /// Absolute path the bytes were written to.
    pub path: PathBuf,
    /// Length of the downloaded body in bytes.
    pub bytes_len: u64,
}

/// Failure modes for [`ArtifactDownloader::download`].
///
/// The variants are deliberately coarse — operators only see the
/// resulting [`crate::packages::PackageInstallResult::error_message`]
/// and don't need a category-by-category breakdown — but they exist
/// as distinct enum variants so consumers can pattern-match (e.g. to
/// distinguish "URL was http://" from "the download exceeded its size
/// cap" in tests and metrics).
#[derive(Debug, Error)]
pub enum DownloadError {
    /// URL did not start with `https://`.
    #[error("only https:// URLs are accepted; got {0:?}")]
    InsecureUrl(String),
    /// Response body exceeded the configured cap.
    #[error("artifact exceeded the {0}-byte size cap")]
    SizeLimitExceeded(u64),
    /// The server returned a non-2xx status.
    #[error("server returned HTTP status {0}")]
    HttpStatus(u16),
    /// Underlying I/O failure while writing the artifact.
    #[error("local I/O error: {0}")]
    Io(String),
    /// Underlying transport failure (DNS, TLS, socket, …). The string
    /// MUST NOT contain any secret material from the request.
    #[error("transport error: {0}")]
    Transport(String),
    /// The default no-op downloader was invoked. Wire a real
    /// downloader to fix.
    #[error("no artifact downloader is registered for this agent")]
    NotConfigured,
}

/// Fetch-an-HTTPS-artifact-to-disk abstraction. Implementations must
/// be `Send + Sync` so a single instance can be shared across the
/// concurrent invocation tasks spawned by the dispatcher.
#[async_trait]
pub trait ArtifactDownloader: Send + Sync {
    /// Download `request` to disk and return the local path.
    async fn download(&self, request: DownloadRequest)
        -> Result<DownloadedArtifact, DownloadError>;
}

/// Default downloader the runtime wires when no concrete client has
/// been registered. Returns [`DownloadError::NotConfigured`] for every
/// request — the MSI / Executable providers translate that into a
/// clean operator-facing failure (`"This agent is not configured to
/// download package artifacts."`) so an install job is refused
/// loudly rather than hanging.
#[derive(Debug, Default, Clone, Copy)]
pub struct RejectingDownloader;

#[async_trait]
impl ArtifactDownloader for RejectingDownloader {
    async fn download(
        &self,
        _request: DownloadRequest,
    ) -> Result<DownloadedArtifact, DownloadError> {
        Err(DownloadError::NotConfigured)
    }
}

/// Validate that `url` is `https://` and short-circuit otherwise. Used
/// by every concrete downloader to satisfy the security contract.
pub fn require_https(url: &str) -> Result<(), DownloadError> {
    // Lowercase scheme compare; per RFC 3986 schemes are
    // case-insensitive. We don't accept `HTTPS://` either uppercase
    // because rustls will accept the URL and we want the rejection to
    // be in this single place.
    if url.len() < 8 || !url.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return Err(DownloadError::InsecureUrl(url.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> DownloadRequest {
        DownloadRequest {
            url: "https://example.invalid/x".into(),
            auth_header: None,
            max_bytes: 1024,
            timeout: Duration::from_secs(1),
            destination_dir: PathBuf::from("/tmp"),
            file_name: "x".into(),
        }
    }

    #[tokio::test]
    async fn rejecting_downloader_returns_not_configured() {
        let err = RejectingDownloader.download(req()).await.unwrap_err();
        assert!(matches!(err, DownloadError::NotConfigured));
    }

    #[test]
    fn require_https_accepts_lowercase_and_uppercase_scheme() {
        assert!(require_https("https://example.com/x").is_ok());
        assert!(require_https("HTTPS://example.com/x").is_ok());
        assert!(require_https("Https://example.com/x").is_ok());
    }

    #[test]
    fn require_https_rejects_http_and_other_schemes() {
        for bad in &[
            "",
            "http://example.com/x",
            "ftp://example.com/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "//example.com/x",
            "example.com/x",
        ] {
            let err = require_https(bad).unwrap_err();
            assert!(matches!(err, DownloadError::InsecureUrl(_)));
        }
    }
}
