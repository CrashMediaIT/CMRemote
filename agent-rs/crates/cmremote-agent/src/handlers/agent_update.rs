// Source: CMRemote, clean-room implementation.

//! Agent self-update hub handler (slice M3, gated on slice R6).
//!
//! Handles `InstallAgentUpdate(downloadUrl, version, sha256,
//! signatureUrl, signedBy)`. The
//! handler downloads the new agent artifact via the same
//! [`ArtifactDownloader`] the package providers use, re-verifies its
//! SHA-256 against the value the server pushed (the server obtained
//! it from the publisher manifest), verifies the Sigstore cosign
//! bundle against the expected certificate identity, stages it to
//! disk, and hands off to the OS-appropriate [`AgentUpdateInstaller`]
//! for the actual binary swap.
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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cmremote_platform::packages::{
    compute_sha256_hex, ct_eq_hex, ArtifactDownloader, DownloadError, DownloadRequest,
    ProcessCommand, ProcessRunner, TokioProcessRunner,
};
use cmremote_platform::HostOs;
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

/// Hard wall-clock cap for the platform package installer. Package
/// manager invocations are allowed to restart services, but should not
/// hang the agent-upgrade worker indefinitely.
pub const AGENT_UPDATE_INSTALL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30 * 60);

/// Hard wall-clock cap for Sigstore cosign verification.
pub const AGENT_UPDATE_SIGNATURE_VERIFY_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);

/// Hard cap on the size of a cosign bundle (1 MiB).
pub const MAX_AGENT_UPDATE_SIGNATURE_BYTES: u64 = 1024 * 1024;

/// Lowercase-hex SHA-256 string length.
const SHA256_HEX_LEN: usize = 64;

/// GitHub Actions OIDC issuer expected for release-workflow keyless
/// signatures.
const GITHUB_ACTIONS_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

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

/// Verifies the Sigstore cosign bundle for a staged update artifact
/// before native package-manager handoff.
#[async_trait]
pub trait AgentUpdateSignatureVerifier: Send + Sync {
    /// Verify `artifact_path` against `bundle_path` and the expected
    /// certificate identity from the publisher manifest.
    async fn verify(
        &self,
        artifact_path: &Path,
        bundle_path: &Path,
        signed_by: &str,
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
    /// Signature URL did not start with `https://`.
    #[error("agent update signature URL must be https://")]
    InsecureSignatureUrl,
    /// Signature/certificate fields were missing or malformed.
    #[error("agent update signature metadata is missing or malformed")]
    BadSignatureMetadata,
    /// Cosign bundle verification failed.
    #[error("agent update cosign verification failed: {0}")]
    SignatureVerification(String),
    /// I/O error while staging or reading the artifact.
    #[error("agent update local I/O error: {0}")]
    Io(String),
    /// Installer rejected the staged artifact.
    #[error("agent update installer error: {0}")]
    Installer(String),
}

/// Process-backed Sigstore cosign verifier. The agent fails closed if
/// cosign is missing or returns non-zero.
pub struct CosignBundleVerifier {
    runner: Arc<dyn ProcessRunner>,
    cosign_path: PathBuf,
}

impl CosignBundleVerifier {
    /// Build a verifier using [`TokioProcessRunner`]. The executable can
    /// be overridden with `CMREMOTE_COSIGN`; otherwise `cosign` is
    /// resolved from PATH.
    pub fn from_env() -> Self {
        let cosign_path = std::env::var_os("CMREMOTE_COSIGN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cosign"));
        Self::new(cosign_path, Arc::new(TokioProcessRunner))
    }

    /// Build a verifier with an injected process runner.
    pub fn new(cosign_path: PathBuf, runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            runner,
            cosign_path,
        }
    }
}

#[async_trait]
impl AgentUpdateSignatureVerifier for CosignBundleVerifier {
    async fn verify(
        &self,
        artifact_path: &Path,
        bundle_path: &Path,
        signed_by: &str,
    ) -> Result<(), AgentUpdateError> {
        if !is_safe_signed_by(signed_by) {
            return Err(AgentUpdateError::BadSignatureMetadata);
        }
        let command = ProcessCommand::new(
            self.cosign_path.clone(),
            vec![
                "verify-blob".to_string(),
                "--bundle".to_string(),
                bundle_path.display().to_string(),
                "--certificate-identity".to_string(),
                signed_by.to_string(),
                "--certificate-oidc-issuer".to_string(),
                GITHUB_ACTIONS_OIDC_ISSUER.to_string(),
                artifact_path.display().to_string(),
            ],
            AGENT_UPDATE_SIGNATURE_VERIFY_TIMEOUT,
        );
        let outcome = self.runner.run(command).await;
        if let Some(error) = outcome.error {
            return Err(AgentUpdateError::SignatureVerification(error));
        }
        if outcome.exit_code != 0 {
            return Err(AgentUpdateError::SignatureVerification(format!(
                "cosign exited with code {}",
                outcome.exit_code
            )));
        }
        Ok(())
    }
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

/// Process-backed package installer for R8 self-updates.
///
/// The publisher manifest resolves to one of the package formats the
/// release workflow emits (`.deb`, `.rpm`, `.msi`, `.pkg`). This
/// installer maps the staged artifact extension to the native package
/// manager for the current OS and invokes it through the same injected
/// [`ProcessRunner`] abstraction used by R6 package installs, so tests
/// can pin argv construction without running a real package manager.
pub struct PackageAgentUpdateInstaller {
    host_os: HostOs,
    runner: Arc<dyn ProcessRunner>,
}

impl PackageAgentUpdateInstaller {
    /// Build an installer for the current host using
    /// [`TokioProcessRunner`].
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current(), Arc::new(TokioProcessRunner))
    }

    /// Build an installer for `host_os` with an injected runner.
    pub fn new(host_os: HostOs, runner: Arc<dyn ProcessRunner>) -> Self {
        Self { host_os, runner }
    }

    fn command_for_artifact(
        &self,
        artifact_path: &Path,
    ) -> Result<ProcessCommand, AgentUpdateError> {
        let ext = artifact_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let artifact = artifact_path.display().to_string();
        let (program, args) = match (self.host_os, ext.as_str()) {
            (HostOs::Linux, "deb") => (
                PathBuf::from("/usr/bin/dpkg"),
                vec!["-i".to_string(), artifact],
            ),
            (HostOs::Linux, "rpm") => (
                PathBuf::from("/usr/bin/rpm"),
                vec!["-Uvh".to_string(), artifact],
            ),
            (HostOs::Windows, "msi") => (
                windows_msiexec_path(),
                vec![
                    "/i".to_string(),
                    artifact,
                    "/qn".to_string(),
                    "/norestart".to_string(),
                ],
            ),
            (HostOs::MacOs, "pkg") => (
                PathBuf::from("/usr/sbin/installer"),
                vec![
                    "-pkg".to_string(),
                    artifact,
                    "-target".to_string(),
                    "/".to_string(),
                ],
            ),
            _ => {
                return Err(AgentUpdateError::Installer(format!(
                    "no self-update installer is available for {:?} artifact '.{}'",
                    self.host_os,
                    if ext.is_empty() {
                        "<none>"
                    } else {
                        ext.as_str()
                    }
                )));
            }
        };

        Ok(ProcessCommand::new(
            program,
            args,
            AGENT_UPDATE_INSTALL_TIMEOUT,
        ))
    }
}

#[async_trait]
impl AgentUpdateInstaller for PackageAgentUpdateInstaller {
    async fn install(
        &self,
        artifact_path: PathBuf,
        _version: String,
    ) -> Result<(), AgentUpdateError> {
        let command = self.command_for_artifact(&artifact_path)?;
        let outcome = self.runner.run(command).await;
        let _ = std::fs::remove_file(&artifact_path);
        if let Some(error) = outcome.error {
            return Err(AgentUpdateError::Installer(error));
        }
        if outcome.exit_code != 0 {
            return Err(AgentUpdateError::Installer(format!(
                "package installer exited with code {}",
                outcome.exit_code
            )));
        }
        Ok(())
    }
}

fn windows_msiexec_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join("msiexec.exe")
}

/// The collaborators required to handle `InstallAgentUpdate`. A single
/// instance is shared between concurrent invocations.
#[derive(Clone)]
pub struct AgentUpdateContext {
    /// HTTPS client for fetching the artifact.
    pub downloader: Arc<dyn ArtifactDownloader>,
    /// Platform-specific installer.
    pub installer: Arc<dyn AgentUpdateInstaller>,
    /// Sigstore cosign verifier.
    pub signature_verifier: Arc<dyn AgentUpdateSignatureVerifier>,
    /// Directory the artifact is staged into. Caller creates it.
    pub stage_dir: PathBuf,
}

/// Handle `InstallAgentUpdate(downloadUrl, version, sha256,
/// signatureUrl, signedBy)`.
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
    let (download_url, version, expected_sha, signature_url, signed_by) = match parse_args(inv) {
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
    let leaf = match artifact_extension_from_url(&download_url) {
        Some(ext) => format!("cmremote-agent-{version}.{ext}"),
        None => format!("cmremote-agent-{version}.update"),
    };

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

    // Phase 3 — verify the Sigstore cosign bundle.
    let sig_request = DownloadRequest {
        url: signature_url,
        auth_header: None,
        max_bytes: MAX_AGENT_UPDATE_SIGNATURE_BYTES,
        timeout: AGENT_UPDATE_SIGNATURE_VERIFY_TIMEOUT,
        destination_dir: ctx.stage_dir.clone(),
        file_name: format!("cmremote-agent-{version}.cosign.bundle"),
    };
    let bundle = match ctx.downloader.download(sig_request).await {
        Ok(a) => a,
        Err(e) => {
            let _ = std::fs::remove_file(&downloaded.path);
            return Err(translate_signature_download_error(&e).to_string());
        }
    };
    if let Err(e) = ctx
        .signature_verifier
        .verify(&downloaded.path, &bundle.path, &signed_by)
        .await
    {
        let _ = std::fs::remove_file(&downloaded.path);
        let _ = std::fs::remove_file(&bundle.path);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_file(&bundle.path);

    // Phase 4 — install.
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

fn parse_args(
    inv: &HubInvocation,
) -> Result<(String, String, String, String, String), AgentUpdateError> {
    if inv.arguments.len() < 5 {
        return Err(AgentUpdateError::InvalidArguments(format!(
            "expected 5 arguments, got {}",
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
    let signature_url = inv.arguments[3].as_str().ok_or_else(|| {
        AgentUpdateError::InvalidArguments("signatureUrl must be a string".into())
    })?;
    let signed_by = inv.arguments[4]
        .as_str()
        .ok_or_else(|| AgentUpdateError::InvalidArguments("signedBy must be a string".into()))?;

    if url.len() < 8 || !url.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return Err(AgentUpdateError::InsecureUrl);
    }
    if signature_url.len() < 8 || !signature_url.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return Err(AgentUpdateError::InsecureSignatureUrl);
    }
    if !is_safe_version(version) {
        return Err(AgentUpdateError::BadVersion);
    }
    if !is_lower_hex_sha256(sha) {
        return Err(AgentUpdateError::BadSha256);
    }
    if !is_safe_signed_by(signed_by) {
        return Err(AgentUpdateError::BadSignatureMetadata);
    }
    Ok((
        url.to_string(),
        version.to_string(),
        sha.to_string(),
        signature_url.to_string(),
        signed_by.to_string(),
    ))
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

fn is_safe_signed_by(v: &str) -> bool {
    if v.is_empty() || v.len() > 512 {
        return false;
    }
    v.bytes().all(|b| b.is_ascii_graphic())
        && v.starts_with("https://github.com/")
        && v.contains("/.github/workflows/")
}

fn artifact_extension_from_url(url: &str) -> Option<&'static str> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "deb" => Some("deb"),
        "rpm" => Some("rpm"),
        "msi" => Some("msi"),
        "pkg" => Some("pkg"),
        _ => None,
    }
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

fn translate_signature_download_error(e: &DownloadError) -> AgentUpdateError {
    match e {
        DownloadError::InsecureUrl(_) => AgentUpdateError::InsecureSignatureUrl,
        other => {
            AgentUpdateError::SignatureVerification(translate_download_error(other).to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use cmremote_platform::packages::{DownloadedArtifact, ProcessOutcome, RejectingDownloader};
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

    fn valid_args(url: &str, version: &str, sha: &str) -> Vec<serde_json::Value> {
        vec![
            url.into(),
            version.into(),
            sha.into(),
            "https://example.com/cmremote-agent.cosign.bundle".into(),
            "https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/v2.0.0"
                .into(),
        ]
    }

    /// Downloader that writes a fixed body to the requested path.
    struct FakeDl {
        bytes: Vec<u8>,
        last: Mutex<Option<DownloadRequest>>,
        canned: Mutex<Option<DownloadError>>,
    }

    struct FakeRunner {
        last: Mutex<Option<ProcessCommand>>,
        outcome: Mutex<ProcessOutcome>,
    }

    impl FakeRunner {
        fn with_exit(exit_code: i32) -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
                outcome: Mutex::new(ProcessOutcome {
                    exit_code,
                    ..ProcessOutcome::default()
                }),
            })
        }

        fn with_error(error: &str) -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
                outcome: Mutex::new(ProcessOutcome {
                    exit_code: -1,
                    error: Some(error.to_string()),
                    ..ProcessOutcome::default()
                }),
            })
        }
    }

    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: ProcessCommand) -> ProcessOutcome {
            *self.last.lock().unwrap() = Some(command);
            self.outcome.lock().unwrap().clone()
        }
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

    #[derive(Default)]
    struct OkSignatureVerifier {
        last: Mutex<Option<(PathBuf, PathBuf, String)>>,
    }

    #[async_trait]
    impl AgentUpdateSignatureVerifier for OkSignatureVerifier {
        async fn verify(
            &self,
            artifact_path: &Path,
            bundle_path: &Path,
            signed_by: &str,
        ) -> Result<(), AgentUpdateError> {
            *self.last.lock().unwrap() = Some((
                artifact_path.to_path_buf(),
                bundle_path.to_path_buf(),
                signed_by.to_string(),
            ));
            Ok(())
        }
    }

    struct FailingSignatureVerifier;

    #[async_trait]
    impl AgentUpdateSignatureVerifier for FailingSignatureVerifier {
        async fn verify(
            &self,
            _artifact_path: &Path,
            _bundle_path: &Path,
            _signed_by: &str,
        ) -> Result<(), AgentUpdateError> {
            Err(AgentUpdateError::SignatureVerification("bad bundle".into()))
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
            signature_verifier: Arc::new(OkSignatureVerifier::default()),
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
            &inv(valid_args(
                "http://example.com/agent.msi",
                "1.2.3",
                &"a".repeat(64),
            )),
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
            &inv(valid_args(
                "https://example.com/cmremote-agent.msi",
                "1.2.3",
                "not-a-real-sha",
            )),
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
            &inv(valid_args(
                "https://example.com/cmremote-agent.msi",
                "1.2.3; rm -rf /",
                &"a".repeat(64),
            )),
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
            &inv(valid_args(
                "https://example.com/x",
                "1.2.3",
                &"a".repeat(64),
            )),
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
            &inv(valid_args(
                "https://example.com/x",
                "1.2.3",
                &"0".repeat(64),
            )),
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
        let verifier = Arc::new(OkSignatureVerifier::default());
        let c = AgentUpdateContext {
            downloader: dl,
            installer: installer.clone(),
            signature_verifier: verifier.clone(),
            stage_dir: tmp.path().to_path_buf(),
        };
        let result = handle_install_agent_update(
            &inv(valid_args(
                "https://example.com/cmremote-agent.msi",
                "1.2.3",
                &sha,
            )),
            &c,
        )
        .await
        .unwrap();
        assert_eq!(result, serde_json::Value::Null);
        let last = installer.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.1, "1.2.3");
        // The staged file's contents should match what we delivered.
        assert_eq!(fs::read(&last.0).unwrap(), bytes);
        assert_eq!(last.0.extension().and_then(|e| e.to_str()), Some("msi"));
        let (_, bundle_path, signed_by) = verifier.last.lock().unwrap().clone().unwrap();
        assert_eq!(
            bundle_path.file_name().and_then(|n| n.to_str()),
            Some("cmremote-agent-1.2.3.cosign.bundle")
        );
        assert!(signed_by.contains("/.github/workflows/release.yml@refs/tags/v2.0.0"));
        assert!(
            !bundle_path.exists(),
            "cosign bundle should be removed after verification"
        );
    }

    #[tokio::test]
    async fn signature_verification_failure_refuses_install_and_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let bytes = b"agent update payload".to_vec();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDl::ok(bytes);
        let installer = OkInstaller::new();
        let c = AgentUpdateContext {
            downloader: dl,
            installer: installer.clone(),
            signature_verifier: Arc::new(FailingSignatureVerifier),
            stage_dir: tmp.path().to_path_buf(),
        };

        let err = handle_install_agent_update(
            &inv(valid_args(
                "https://example.com/cmremote-agent.msi",
                "1.2.3",
                &sha,
            )),
            &c,
        )
        .await
        .unwrap_err();

        assert!(err.contains("cosign verification failed"));
        assert!(installer.last.lock().unwrap().is_none());
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "leftover files: {entries:?}");
    }

    #[tokio::test]
    async fn stub_installer_returns_clean_failure_after_verify() {
        let tmp = TempDir::new().unwrap();
        let bytes = b"agent update payload".to_vec();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDl::ok(bytes);
        let c = ctx(&tmp, dl, Arc::new(StubAgentUpdateInstaller));
        let err = handle_install_agent_update(
            &inv(valid_args("https://example.com/x", "1.2.3", &sha)),
            &c,
        )
        .await
        .unwrap_err();
        assert!(err.contains("no self-update installer"));
        // No leftover staged file.
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "leftover files: {entries:?}");
    }

    #[tokio::test]
    async fn package_installer_invokes_dpkg_for_deb_and_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let artifact = tmp.path().join("cmremote-agent_2.0.0_amd64.deb");
        fs::write(&artifact, b"deb").unwrap();
        let runner = FakeRunner::with_exit(0);
        let installer = PackageAgentUpdateInstaller::new(HostOs::Linux, runner.clone());

        installer
            .install(artifact.clone(), "2.0.0".into())
            .await
            .unwrap();

        let cmd = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(cmd.program, PathBuf::from("/usr/bin/dpkg"));
        assert_eq!(
            cmd.args,
            vec!["-i".to_string(), artifact.display().to_string()]
        );
        assert_eq!(cmd.timeout, AGENT_UPDATE_INSTALL_TIMEOUT);
        assert!(
            !artifact.exists(),
            "installer owns and removes staged artifact"
        );
    }

    #[tokio::test]
    async fn cosign_verifier_invokes_verify_blob_with_bundle_and_identity() {
        let tmp = TempDir::new().unwrap();
        let artifact = tmp.path().join("cmremote-agent.deb");
        let bundle = tmp.path().join("cmremote-agent.deb.cosign.bundle");
        fs::write(&artifact, b"deb").unwrap();
        fs::write(&bundle, b"bundle").unwrap();
        let runner = FakeRunner::with_exit(0);
        let verifier = CosignBundleVerifier::new(PathBuf::from("/usr/bin/cosign"), runner.clone());
        let signed_by =
            "https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/v2.0.0";

        verifier
            .verify(&artifact, &bundle, signed_by)
            .await
            .unwrap();

        let cmd = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(cmd.program, PathBuf::from("/usr/bin/cosign"));
        assert_eq!(
            cmd.args,
            vec![
                "verify-blob".to_string(),
                "--bundle".to_string(),
                bundle.display().to_string(),
                "--certificate-identity".to_string(),
                signed_by.to_string(),
                "--certificate-oidc-issuer".to_string(),
                GITHUB_ACTIONS_OIDC_ISSUER.to_string(),
                artifact.display().to_string(),
            ]
        );
        assert_eq!(cmd.timeout, AGENT_UPDATE_SIGNATURE_VERIFY_TIMEOUT);
    }

    #[tokio::test]
    async fn package_installer_invokes_rpm_for_rpm() {
        let tmp = TempDir::new().unwrap();
        let artifact = tmp.path().join("cmremote-agent-2.0.0-1.x86_64.rpm");
        fs::write(&artifact, b"rpm").unwrap();
        let runner = FakeRunner::with_exit(0);
        let installer = PackageAgentUpdateInstaller::new(HostOs::Linux, runner.clone());

        installer
            .install(artifact.clone(), "2.0.0".into())
            .await
            .unwrap();

        let cmd = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(cmd.program, PathBuf::from("/usr/bin/rpm"));
        assert_eq!(
            cmd.args,
            vec!["-Uvh".to_string(), artifact.display().to_string()]
        );
    }

    #[test]
    fn package_installer_maps_windows_msi_and_macos_pkg_commands() {
        let runner = FakeRunner::with_exit(0);
        let windows = PackageAgentUpdateInstaller::new(HostOs::Windows, runner.clone());
        let cmd = windows
            .command_for_artifact(Path::new(r"C:\Temp\cmremote-agent.msi"))
            .unwrap();
        assert!(cmd
            .program
            .ends_with(Path::new("System32").join("msiexec.exe")));
        assert_eq!(
            cmd.args,
            vec![
                "/i".to_string(),
                r"C:\Temp\cmremote-agent.msi".to_string(),
                "/qn".to_string(),
                "/norestart".to_string(),
            ]
        );

        let mac = PackageAgentUpdateInstaller::new(HostOs::MacOs, runner);
        let cmd = mac
            .command_for_artifact(Path::new("/tmp/cmremote-agent.pkg"))
            .unwrap();
        assert_eq!(cmd.program, PathBuf::from("/usr/sbin/installer"));
        assert_eq!(
            cmd.args,
            vec![
                "-pkg".to_string(),
                "/tmp/cmremote-agent.pkg".to_string(),
                "-target".to_string(),
                "/".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn package_installer_refuses_wrong_artifact_for_host_without_runner_call() {
        let tmp = TempDir::new().unwrap();
        let artifact = tmp.path().join("cmremote-agent.msi");
        fs::write(&artifact, b"msi").unwrap();
        let runner = FakeRunner::with_exit(0);
        let installer = PackageAgentUpdateInstaller::new(HostOs::Linux, runner.clone());

        let err = installer
            .install(artifact.clone(), "2.0.0".into())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("no self-update installer"));
        assert!(runner.last.lock().unwrap().is_none());
        assert!(
            artifact.exists(),
            "handler cleanup owns pre-install refusal"
        );
    }

    #[tokio::test]
    async fn package_installer_reports_runner_error_and_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let artifact = tmp.path().join("cmremote-agent.deb");
        fs::write(&artifact, b"deb").unwrap();
        let runner = FakeRunner::with_error("Timed out.");
        let installer = PackageAgentUpdateInstaller::new(HostOs::Linux, runner);

        let err = installer
            .install(artifact.clone(), "2.0.0".into())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Timed out"));
        assert!(!artifact.exists());
    }

    #[tokio::test]
    async fn package_installer_reports_nonzero_exit_and_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let artifact = tmp.path().join("cmremote-agent.deb");
        fs::write(&artifact, b"deb").unwrap();
        let runner = FakeRunner::with_exit(42);
        let installer = PackageAgentUpdateInstaller::new(HostOs::Linux, runner);

        let err = installer
            .install(artifact.clone(), "2.0.0".into())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("42"));
        assert!(!artifact.exists());
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

    #[test]
    fn signed_by_validator_accepts_release_workflow_identity_only() {
        assert!(is_safe_signed_by(
            "https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml@refs/tags/v2.0.0"
        ));
        assert!(!is_safe_signed_by(""));
        assert!(!is_safe_signed_by("https://example.com/release.yml"));
        assert!(!is_safe_signed_by(
            "https://github.com/CrashMediaIT/CMRemote/actions/runs/1"
        ));
        assert!(!is_safe_signed_by(
            "https://github.com/CrashMediaIT/CMRemote/.github/workflows/release.yml\nmalicious"
        ));
    }

    #[test]
    fn artifact_extension_from_url_accepts_supported_package_extensions_only() {
        assert_eq!(
            artifact_extension_from_url("https://example.com/cmremote-agent.deb"),
            Some("deb")
        );
        assert_eq!(
            artifact_extension_from_url("https://example.com/cmremote-agent.RPM?sig=1"),
            Some("rpm")
        );
        assert_eq!(
            artifact_extension_from_url("https://example.com/cmremote-agent.pkg#fragment"),
            Some("pkg")
        );
        assert_eq!(
            artifact_extension_from_url("https://example.com/cmremote-agent.tar.gz"),
            None
        );
        assert_eq!(
            artifact_extension_from_url("https://example.com/noext"),
            None
        );
    }
}
