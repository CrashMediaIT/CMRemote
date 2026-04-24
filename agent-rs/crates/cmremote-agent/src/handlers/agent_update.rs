// Source: CMRemote, clean-room implementation.

//! Agent self-update hub handler (slice M3, gated on slice R6).
//!
//! Handles `InstallAgentUpdate(downloadUrl, version, sha256)`. The
//! handler downloads the new agent artifact via the same
//! [`ArtifactDownloader`] the package providers use, re-verifies its
//! SHA-256 against the value the server pushed (the server obtained
//! it from the publisher manifest), stages it to disk, and hands off
//! to the OS-appropriate [`AgentUpdateInstaller`] for the actual
//! binary swap.
//!
//! ## Why the install is behind a trait
//!
//! Replacing the running agent binary is intrinsically platform-
//! specific:
//!
//! * On Linux, the package manager (apt / rpm) owns the installed
//!   path; the agent only stages the .deb / .rpm and triggers the
//!   manager.
//! * On Windows, the SCM owns the binary lock; the swap goes through
//!   a small helper that stops the service, replaces the .exe, and
//!   restarts it.
//!
//! The handler is concerned only with the "fetch the right bytes"
//! half. The "swap the bits" half lives in [`AgentUpdateInstaller`]
//! implementations that the runtime registers later.
//!
//! Until a real installer is wired the default
//! [`StubAgentUpdateInstaller`] returns a structured "not configured"
//! failure — exactly the same shape as `RejectingDownloader` does for
//! package downloads, so the operator sees a clean error in the
//! manifest dispatcher's audit trail rather than an apparent silent
//! success.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cmremote_platform::packages::{
    compute_sha256_hex, ct_eq_hex, ArtifactDownloader, DownloadError, DownloadRequest,
};
use cmremote_wire::HubInvocation;
use tracing::{info, warn};

/// Hard wall-clock cap for the agent-update download. Same 15 minutes
/// as the package downloader.
pub const AGENT_UPDATE_DOWNLOAD_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(15 * 60);

/// Hard cap on the size of an agent self-update artifact (256 MiB).
/// The Rust agent itself is < 50 MiB stripped; 256 MiB is enough
/// headroom for the .msi/.deb/.rpm wrapper without being adversarial.
pub const MAX_AGENT_UPDATE_BYTES: u64 = 256 * 1024 * 1024;

/// Lowercase-hex SHA-256 string length.
const SHA256_HEX_LEN: usize = 64;

/// Performs the platform-specific binary swap once a verified update
/// artifact is on disk. Implementations must be side-effect-free in
/// the failure path: a failure here MUST leave the running agent
/// untouched.
#[async_trait]
pub trait AgentUpdateInstaller: Send + Sync {
    /// Apply the staged update at `artifact_path`. The implementation
    /// owns the file from this point and is responsible for deleting
    /// it when no longer needed.
    async fn install(
        &self,
        artifact_path: PathBuf,
        version: String,
    ) -> Result<(), AgentUpdateError>;
}

/// Failure modes for the agent self-update handler.
#[derive(Debug, thiserror::Error)]
pub enum AgentUpdateError {
    /// Wire arguments could not be decoded into the 3-string contract.
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
    /// Download URL did not start with `https://`.
    #[error("agent update URL must be https://")]
    InsecureUrl,
    /// SHA-256 was not 64 lowercase-hex characters.
    #[error("agent update SHA-256 is malformed")]
    BadSha256,
    /// Version string was empty or contained disallowed characters.
    #[error("agent update version is missing or malformed")]
    BadVersion,
    /// Download phase failed.
    #[error("agent update download failed: {0}")]
    Download(String),
    /// SHA-256 of the downloaded bytes did not match the wire value.
    #[error("agent update SHA-256 mismatch — refusing to install")]
    Sha256Mismatch,
    /// I/O error while staging or reading the artifact.
    #[error("agent update local I/O error: {0}")]
    Io(String),
    /// Installer rejected the staged artifact.
    #[error("agent update installer error: {0}")]
    Installer(String),
}

/// Default [`AgentUpdateInstaller`] — returns a structured failure for
/// every request. Wire a real installer to enable agent self-update.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubAgentUpdateInstaller;

#[async_trait]
impl AgentUpdateInstaller for StubAgentUpdateInstaller {
    async fn install(
        &self,
        _artifact_path: PathBuf,
        _version: String,
    ) -> Result<(), AgentUpdateError> {
        Err(AgentUpdateError::Installer(
            "this agent has no self-update installer registered".into(),
        ))
    }
}

/// The collaborators required to handle `InstallAgentUpdate`. A single
/// instance is shared between concurrent invocations.
#[derive(Clone)]
pub struct AgentUpdateContext {
    /// HTTPS client for fetching the artifact.
    pub downloader: Arc<dyn ArtifactDownloader>,
    /// Platform-specific installer.
    pub installer: Arc<dyn AgentUpdateInstaller>,
    /// Directory the artifact is staged into. Caller creates it.
    pub stage_dir: PathBuf,
}

/// Handle `InstallAgentUpdate(downloadUrl, version, sha256)`.
///
/// On success the completion payload is `null`. On failure it is the
/// stringified [`AgentUpdateError`] returned via the hub-completion
/// `error` field — the manifest dispatcher already understands this
/// shape (it polls heartbeats for the version bump as proof of
/// success and treats the absence of a bump as a failure).
pub async fn handle_install_agent_update(
    inv: &HubInvocation,
    ctx: &AgentUpdateContext,
) -> Result<serde_json::Value, String> {
    let (download_url, version, expected_sha) = match parse_args(inv) {
        Ok(t) => t,
        Err(e) => return Err(e.to_string()),
    };

    info!(
        version = %version,
        "agent self-update requested; fetching artifact"
    );

    // Phase 1 — stage. The leaf name is the bare version so a
    // partially-staged artifact from a previous attempt is overwritten
    // rather than accumulated.
    if let Err(e) = std::fs::create_dir_all(&ctx.stage_dir) {
        return Err(AgentUpdateError::Io(e.to_string()).to_string());
    }
    let leaf = format!("cmremote-agent-{version}.update");

    let dl_request = DownloadRequest {
        url: download_url,
        auth_header: None,
        max_bytes: MAX_AGENT_UPDATE_BYTES,
        timeout: AGENT_UPDATE_DOWNLOAD_TIMEOUT,
        destination_dir: ctx.stage_dir.clone(),
        file_name: leaf,
    };

    let downloaded = match ctx.downloader.download(dl_request).await {
        Ok(a) => a,
        Err(e) => return Err(translate_download_error(&e).to_string()),
    };

    // Phase 2 — verify.
    let bytes = match std::fs::read(&downloaded.path) {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&downloaded.path);
            return Err(AgentUpdateError::Io(e.to_string()).to_string());
        }
    };
    let actual = compute_sha256_hex(&bytes);
    if !ct_eq_hex(&expected_sha, &actual) {
        warn!(
            version = %version,
            "SHA-256 mismatch on staged agent update; refusing to install"
        );
        let _ = std::fs::remove_file(&downloaded.path);
        return Err(AgentUpdateError::Sha256Mismatch.to_string());
    }

    // Phase 3 — install.
    info!(
        version = %version,
        path = %downloaded.path.display(),
        "agent self-update verified; handing off to installer"
    );
    if let Err(e) = ctx
        .installer
        .install(downloaded.path.clone(), version.clone())
        .await
    {
        // Installer owns the file on success only. Best-effort
        // cleanup here so a `StubAgentUpdateInstaller` failure
        // doesn't leak the bytes.
        let _ = std::fs::remove_file(&downloaded.path);
        return Err(e.to_string());
    }

    Ok(serde_json::Value::Null)
}

fn parse_args(inv: &HubInvocation) -> Result<(String, String, String), AgentUpdateError> {
    if inv.arguments.len() < 3 {
        return Err(AgentUpdateError::InvalidArguments(format!(
            "expected 3 arguments, got {}",
            inv.arguments.len()
        )));
    }
    let url = inv.arguments[0]
        .as_str()
        .ok_or_else(|| AgentUpdateError::InvalidArguments("downloadUrl must be a string".into()))?;
    let version = inv.arguments[1]
        .as_str()
        .ok_or_else(|| AgentUpdateError::InvalidArguments("version must be a string".into()))?;
    let sha = inv.arguments[2]
        .as_str()
        .ok_or_else(|| AgentUpdateError::InvalidArguments("sha256 must be a string".into()))?;

    if url.len() < 8 || !url.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return Err(AgentUpdateError::InsecureUrl);
    }
    if !is_safe_version(version) {
        return Err(AgentUpdateError::BadVersion);
    }
    if !is_lower_hex_sha256(sha) {
        return Err(AgentUpdateError::BadSha256);
    }
    Ok((url.to_string(), version.to_string(), sha.to_string()))
}

fn is_lower_hex_sha256(s: &str) -> bool {
    s.len() == SHA256_HEX_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_safe_version(v: &str) -> bool {
    // SemVer 2.0 character set: ASCII alphanumerics, dot, hyphen, plus.
    // We deliberately do NOT allow underscores so this matches the
    // shape produced by the publisher manifest's signed-build pipeline
    // and stays consistent with `is_safe_chocolatey_version` over in
    // `cmremote-platform::packages`.
    if v.is_empty() || v.len() > 64 {
        return false;
    }
    v.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+')
}

fn translate_download_error(e: &DownloadError) -> AgentUpdateError {
    match e {
        DownloadError::NotConfigured => AgentUpdateError::Download(
            "this agent is not configured to download agent updates".into(),
        ),
        DownloadError::InsecureUrl(_) => AgentUpdateError::InsecureUrl,
        DownloadError::SizeLimitExceeded(cap) => {
            AgentUpdateError::Download(format!("artifact exceeded the {cap}-byte size cap"))
        }
        DownloadError::HttpStatus(s) => {
            AgentUpdateError::Download(format!("server returned HTTP {s}"))
        }
        DownloadError::Io(s) => AgentUpdateError::Io(s.clone()),
        DownloadError::Transport(s) => AgentUpdateError::Download(format!("transport error: {s}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use cmremote_platform::packages::{DownloadedArtifact, RejectingDownloader};
    use cmremote_wire::HubMessageKind;
    use tempfile::TempDir;

    use super::*;

    fn inv(args: Vec<serde_json::Value>) -> HubInvocation {
        HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("inv-1".into()),
            target: "InstallAgentUpdate".into(),
            arguments: args,
        }
    }

    /// Downloader that writes a fixed body to the requested path.
    struct FakeDl {
        bytes: Vec<u8>,
        last: Mutex<Option<DownloadRequest>>,
        canned: Mutex<Option<DownloadError>>,
    }
    impl FakeDl {
        fn ok(bytes: Vec<u8>) -> Arc<Self> {
            Arc::new(Self {
                bytes,
                last: Mutex::new(None),
                canned: Mutex::new(None),
            })
        }
        fn fail(e: DownloadError) -> Arc<Self> {
            Arc::new(Self {
                bytes: vec![],
                last: Mutex::new(None),
                canned: Mutex::new(Some(e)),
            })
        }
    }
    #[async_trait]
    impl ArtifactDownloader for FakeDl {
        async fn download(
            &self,
            request: DownloadRequest,
        ) -> Result<DownloadedArtifact, DownloadError> {
            *self.last.lock().unwrap() = Some(request.clone());
            if let Some(e) = self.canned.lock().unwrap().take() {
                return Err(e);
            }
            let dest = request.destination_dir.join(&request.file_name);
            fs::write(&dest, &self.bytes).map_err(|e| DownloadError::Io(e.to_string()))?;
            Ok(DownloadedArtifact {
                path: dest,
                bytes_len: self.bytes.len() as u64,
            })
        }
    }

    /// Installer that records the path and returns success.
    struct OkInstaller {
        last: Mutex<Option<(PathBuf, String)>>,
    }
    impl OkInstaller {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
            })
        }
    }
    #[async_trait]
    impl AgentUpdateInstaller for OkInstaller {
        async fn install(&self, p: PathBuf, v: String) -> Result<(), AgentUpdateError> {
            *self.last.lock().unwrap() = Some((p, v));
            Ok(())
        }
    }

    fn ctx(
        stage: &TempDir,
        dl: Arc<dyn ArtifactDownloader>,
        installer: Arc<dyn AgentUpdateInstaller>,
    ) -> AgentUpdateContext {
        AgentUpdateContext {
            downloader: dl,
            installer,
            stage_dir: stage.path().to_path_buf(),
        }
    }

    #[tokio::test]
    async fn missing_arguments_returns_invalid_arguments() {
        let tmp = TempDir::new().unwrap();
        let c = ctx(
            &tmp,
            Arc::new(RejectingDownloader),
            Arc::new(StubAgentUpdateInstaller),
        );
        let err = handle_install_agent_update(&inv(vec![]), &c)
            .await
            .unwrap_err();
        assert!(err.contains("invalid_arguments"));
    }

    #[tokio::test]
    async fn http_url_refused() {
        let tmp = TempDir::new().unwrap();
        let c = ctx(
            &tmp,
            Arc::new(RejectingDownloader),
            Arc::new(StubAgentUpdateInstaller),
        );
        let err = handle_install_agent_update(
            &inv(vec![
                "http://example.com/agent.msi".into(),
                "1.2.3".into(),
                "a".repeat(64).into(),
            ]),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("https"));
    }

    #[tokio::test]
    async fn malformed_sha256_refused() {
        let tmp = TempDir::new().unwrap();
        let c = ctx(
            &tmp,
            Arc::new(RejectingDownloader),
            Arc::new(StubAgentUpdateInstaller),
        );
        let err = handle_install_agent_update(
            &inv(vec![
                "https://example.com/x".into(),
                "1.2.3".into(),
                "not-a-real-sha".into(),
            ]),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("SHA-256"));
    }

    #[tokio::test]
    async fn malformed_version_refused() {
        let tmp = TempDir::new().unwrap();
        let c = ctx(
            &tmp,
            Arc::new(RejectingDownloader),
            Arc::new(StubAgentUpdateInstaller),
        );
        let err = handle_install_agent_update(
            &inv(vec![
                "https://example.com/x".into(),
                "1.2.3; rm -rf /".into(),
                "a".repeat(64).into(),
            ]),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("version"));
    }

    #[tokio::test]
    async fn download_failure_translates() {
        let tmp = TempDir::new().unwrap();
        let dl = FakeDl::fail(DownloadError::NotConfigured);
        let c = ctx(&tmp, dl, Arc::new(StubAgentUpdateInstaller));
        let err = handle_install_agent_update(
            &inv(vec![
                "https://example.com/x".into(),
                "1.2.3".into(),
                "a".repeat(64).into(),
            ]),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("not configured"));
    }

    #[tokio::test]
    async fn sha256_mismatch_refuses_install() {
        let tmp = TempDir::new().unwrap();
        let dl = FakeDl::ok(b"some bytes".to_vec());
        let installer = OkInstaller::new();
        let c = ctx(&tmp, dl, installer.clone());
        let err = handle_install_agent_update(
            &inv(vec![
                "https://example.com/x".into(),
                "1.2.3".into(),
                "0".repeat(64).into(),
            ]),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("SHA-256"));
        // Installer was not invoked.
        assert!(installer.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn happy_path_invokes_installer_with_verified_artifact() {
        let tmp = TempDir::new().unwrap();
        let bytes = b"agent update payload".to_vec();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDl::ok(bytes.clone());
        let installer = OkInstaller::new();
        let c = ctx(&tmp, dl, installer.clone());
        let result = handle_install_agent_update(
            &inv(vec![
                "https://example.com/x".into(),
                "1.2.3".into(),
                sha.into(),
            ]),
            &c,
        )
        .await
        .unwrap();
        assert_eq!(result, serde_json::Value::Null);
        let last = installer.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.1, "1.2.3");
        // The staged file's contents should match what we delivered.
        assert_eq!(fs::read(&last.0).unwrap(), bytes);
    }

    #[tokio::test]
    async fn stub_installer_returns_clean_failure_after_verify() {
        let tmp = TempDir::new().unwrap();
        let bytes = b"agent update payload".to_vec();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDl::ok(bytes);
        let c = ctx(&tmp, dl, Arc::new(StubAgentUpdateInstaller));
        let err = handle_install_agent_update(
            &inv(vec![
                "https://example.com/x".into(),
                "1.2.3".into(),
                sha.into(),
            ]),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("no self-update installer"));
        // No leftover staged file.
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "leftover files: {entries:?}");
    }

    #[test]
    fn lower_hex_sha256_validator() {
        assert!(is_lower_hex_sha256(&"a".repeat(64)));
        assert!(is_lower_hex_sha256(&"0123456789abcdef".repeat(4)));
        assert!(!is_lower_hex_sha256(&"A".repeat(64))); // uppercase
        assert!(!is_lower_hex_sha256(&"a".repeat(63))); // wrong length
        assert!(!is_lower_hex_sha256(&"a".repeat(65)));
        assert!(!is_lower_hex_sha256("zz"));
    }

    #[test]
    fn version_validator_accepts_semver_and_rejects_metachars() {
        assert!(is_safe_version("1.2.3"));
        assert!(is_safe_version("1.2.3-rc.1"));
        assert!(is_safe_version("1.2.3+build.4"));
        assert!(!is_safe_version(""));
        assert!(!is_safe_version("1.2.3 ; rm"));
        assert!(!is_safe_version("1.2.3$(whoami)"));
    }
}
