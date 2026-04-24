// Source: CMRemote, clean-room implementation.

//! UploadedMsi-backed [`PackageProviderHandler`] implementation
//! (slice R6).
//!
//! Workflow re-derived from the spec of the .NET `MsiPackageInstaller`:
//!
//! 1. Validate the wire metadata (filename, SHA-256, host).
//! 2. Pull the bytes via the injected [`ArtifactDownloader`] into a
//!    cache directory under the agent's data path.
//! 3. Re-hash the downloaded bytes with SHA-256 and check the OLE2
//!    magic signature; refuse on either mismatch (both helpers live
//!    in [`crate::packages`]).
//! 4. Build a `msiexec /i <file> /qn /norestart /L*v <log>` argv with
//!    operator-supplied install arguments appended as discrete slots.
//! 5. Run via the injected [`ProcessRunner`]; on failure, attach the
//!    tail of the verbose log so the operator can see why msiexec
//!    refused the install.
//! 6. Best-effort delete the downloaded MSI + log file before
//!    returning so the agent never accumulates GiBs of cached
//!    installers.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cmremote_wire::{
    PackageInstallAction, PackageInstallRequest, PackageInstallResult, PackageProvider,
};
use tracing::{info, warn};

use super::download::{ArtifactDownloader, DownloadError, DownloadRequest, RejectingDownloader};
use super::process::{ProcessCommand, ProcessRunner, TokioProcessRunner};
use super::{
    compute_sha256_hex, ct_eq_hex, is_msi_magic_bytes, is_safe_msi_file_name,
    PackageProviderHandler,
};

/// Hard wall-clock cap for a single `msiexec` invocation. Mirrors the
/// .NET reference (`InstallTimeout = 60 min`).
pub const MSIEXEC_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Hard wall-clock cap for the artifact download. Mirrors the .NET
/// reference (`DownloadTimeout = 15 min`).
pub const MSI_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Hard cap on the size of a single MSI artifact (1 GiB). The .NET
/// reference has no explicit cap; we add one because our streaming
/// downloader needs to refuse adversarially-large bodies up front.
pub const MAX_MSI_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum number of bytes the operator-visible log tail will contain.
pub const MSI_LOG_TAIL_BYTES: usize = 16 * 1024;

/// Resolves the on-disk path of `msiexec.exe` and the cache directory
/// the MSI downloader will write to. Implementations must not spawn a
/// child process — this runs on the dispatcher hot path via
/// `can_handle`.
pub trait MsiEnvironment: Send + Sync {
    /// Returns `Some(path)` when `msiexec.exe` is usable on this host.
    fn resolve_msiexec(&self) -> Option<PathBuf>;
    /// Directory the downloader writes the MSI + verbose log into.
    /// The caller is responsible for creating it (and chmod'ing it on
    /// Unix).
    fn cache_dir(&self) -> PathBuf;
    /// Base server URL the agent should fetch the MSI from. The
    /// default implementation derives this from `ConnectionInfo`.
    fn server_host(&self) -> Option<String>;
}

/// Default environment probe. On Windows it pulls `msiexec.exe` from
/// `%SystemRoot%\System32\`; on every other OS it returns `None`.
#[derive(Debug, Clone)]
pub struct StdMsiEnvironment {
    cache_dir: PathBuf,
    server_host: Option<String>,
}

impl StdMsiEnvironment {
    /// Construct a probe with an explicit cache directory + server
    /// host. Wired by the runtime from `ConnectionInfo`.
    pub fn new(cache_dir: PathBuf, server_host: Option<String>) -> Self {
        Self {
            cache_dir,
            server_host,
        }
    }
}

impl MsiEnvironment for StdMsiEnvironment {
    fn resolve_msiexec(&self) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            if let Ok(system_root) = std::env::var("SystemRoot") {
                let candidate = PathBuf::from(system_root)
                    .join("System32")
                    .join("msiexec.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            None
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    fn cache_dir(&self) -> PathBuf {
        self.cache_dir.clone()
    }

    fn server_host(&self) -> Option<String> {
        self.server_host.clone()
    }
}

/// Concrete UploadedMsi [`PackageProviderHandler`].
pub struct UploadedMsiPackageProvider {
    env: Arc<dyn MsiEnvironment>,
    runner: Arc<dyn ProcessRunner>,
    downloader: Arc<dyn ArtifactDownloader>,
    download_timeout: Duration,
    install_timeout: Duration,
    max_bytes: u64,
}

impl UploadedMsiPackageProvider {
    /// Construct a provider with the supplied environment + the
    /// default OS-backed process runner and a *rejecting* downloader.
    /// Use [`Self::new_with`] to wire a real downloader.
    pub fn new(env: Arc<dyn MsiEnvironment>) -> Self {
        Self::new_with(
            env,
            Arc::new(TokioProcessRunner),
            Arc::new(RejectingDownloader),
            MSI_DOWNLOAD_TIMEOUT,
            MSIEXEC_TIMEOUT,
            MAX_MSI_BYTES,
        )
    }

    /// Construct a provider with explicit collaborators. Used by tests
    /// (and by the runtime once a real downloader has been wired).
    pub fn new_with(
        env: Arc<dyn MsiEnvironment>,
        runner: Arc<dyn ProcessRunner>,
        downloader: Arc<dyn ArtifactDownloader>,
        download_timeout: Duration,
        install_timeout: Duration,
        max_bytes: u64,
    ) -> Self {
        Self {
            env,
            runner,
            downloader,
            download_timeout,
            install_timeout,
            max_bytes,
        }
    }

    fn build_argv(
        action: PackageInstallAction,
        msi_path: &std::path::Path,
        log_path: &std::path::Path,
        extra: Option<&str>,
    ) -> Vec<String> {
        let mut args: Vec<String> = Vec::with_capacity(16);
        args.push(
            match action {
                PackageInstallAction::Uninstall => "/x",
                PackageInstallAction::Install => "/i",
            }
            .to_string(),
        );
        args.push(msi_path.to_string_lossy().into_owned());
        args.push("/qn".into());
        args.push("/norestart".into());
        args.push("/L*v".into());
        args.push(log_path.to_string_lossy().into_owned());
        if let Some(s) = extra {
            for part in s.split_whitespace() {
                args.push(part.to_string());
            }
        }
        args
    }

    fn classify_msiexec_exit(code: i32) -> bool {
        // 0    = success
        // 3010 = success but reboot required
        // 1641 = success and reboot was initiated
        // — Microsoft documented "successful" exit codes for msiexec.
        matches!(code, 0 | 3010 | 1641)
    }
}

#[async_trait]
impl PackageProviderHandler for UploadedMsiPackageProvider {
    fn can_handle(&self, request: &PackageInstallRequest) -> bool {
        request.provider == PackageProvider::UploadedMsi
            && self.env.resolve_msiexec().is_some()
            && request
                .msi_shared_file_id
                .as_deref()
                .is_some_and(|s| !s.is_empty())
            && request.msi_sha256.as_deref().is_some_and(|s| !s.is_empty())
    }

    async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
        let started = Instant::now();
        let mut result = PackageInstallResult::failed(request.job_id.clone(), "");

        if request.provider != PackageProvider::UploadedMsi {
            result.error_message = Some("Provider mismatch.".into());
            return result;
        }
        let shared_id = match request
            .msi_shared_file_id
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            Some(s) => s,
            None => {
                result.error_message = Some("MSI download metadata is missing.".into());
                return result;
            }
        };
        let expected_sha = match request.msi_sha256.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => {
                result.error_message = Some("MSI download metadata is missing.".into());
                return result;
            }
        };
        let auth_token = request.msi_auth_token.as_deref().unwrap_or("");
        let leaf = request.msi_file_name.as_deref().unwrap_or("setup.msi");
        if !is_safe_msi_file_name(leaf) {
            result.error_message = Some("MSI filename contains disallowed characters.".into());
            return result;
        }
        let msiexec = match self.env.resolve_msiexec() {
            Some(p) => p,
            None => {
                result.error_message = Some("msiexec.exe could not be located.".into());
                return result;
            }
        };
        let host = match self.env.server_host() {
            Some(h) if !h.is_empty() => h.trim_end_matches('/').to_string(),
            _ => {
                result.error_message = Some("Server host is not configured.".into());
                return result;
            }
        };

        let cache_dir = self.env.cache_dir();
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            result.error_message = Some(format!("Failed to prepare MSI cache: {e}"));
            return result;
        }

        let unique = uuid::Uuid::new_v4().simple().to_string();
        let local_file = cache_dir.join(format!("{unique}_{leaf}"));
        let log_file = cache_dir.join(format!("{unique}.msi.log"));

        let url = format!("{host}/API/FileSharing/{shared_id}");
        let dl_request = DownloadRequest {
            url,
            auth_header: if auth_token.is_empty() {
                None
            } else {
                Some(("X-Expiring-Token".into(), auth_token.to_string()))
            },
            max_bytes: self.max_bytes,
            timeout: self.download_timeout,
            destination_dir: cache_dir.clone(),
            file_name: format!("{unique}_{leaf}"),
        };

        info!(
            job_id = %request.job_id,
            shared_id = %shared_id,
            "MSI package job starting (download phase)"
        );

        // Phase 1 — download.
        let downloaded = match self.downloader.download(dl_request).await {
            Ok(a) => a,
            Err(e) => {
                result.error_message = Some(translate_download_error(&e));
                result.duration_ms = started.elapsed().as_millis() as i64;
                cleanup(&local_file, &log_file);
                return result;
            }
        };
        // The downloader is contractually obligated to write to the
        // path we gave it; tolerate variation by using whatever it
        // returns.
        let actual_path = downloaded.path;

        // Phase 2 — verify.
        let bytes = match fs::read(&actual_path) {
            Ok(b) => b,
            Err(e) => {
                result.error_message = Some(format!("Failed to re-read downloaded MSI: {e}"));
                cleanup(&actual_path, &log_file);
                result.duration_ms = started.elapsed().as_millis() as i64;
                return result;
            }
        };
        if !is_msi_magic_bytes(&bytes) {
            warn!(job_id = %request.job_id, "downloaded MSI failed OLE2 magic check");
            result.error_message =
                Some("Downloaded file is not a valid MSI (magic-byte check failed).".into());
            cleanup(&actual_path, &log_file);
            result.duration_ms = started.elapsed().as_millis() as i64;
            return result;
        }
        let actual_sha = compute_sha256_hex(&bytes);
        if !ct_eq_hex(expected_sha, &actual_sha) {
            warn!(
                job_id = %request.job_id,
                "SHA-256 mismatch on downloaded MSI; refusing to install"
            );
            result.error_message = Some("SHA-256 mismatch — refusing to install.".into());
            cleanup(&actual_path, &log_file);
            result.duration_ms = started.elapsed().as_millis() as i64;
            return result;
        }

        // Phase 3 — install.
        let argv = Self::build_argv(
            request.action,
            &actual_path,
            &log_file,
            request.install_arguments.as_deref(),
        );
        info!(
            job_id = %request.job_id,
            file = %actual_path.display(),
            "msiexec install starting"
        );
        let outcome = self
            .runner
            .run(ProcessCommand::new(msiexec, argv, self.install_timeout))
            .await;

        let success = outcome.error.is_none() && Self::classify_msiexec_exit(outcome.exit_code);
        let log_tail = if !success {
            read_log_tail(&log_file)
        } else {
            None
        };

        cleanup(&actual_path, &log_file);

        result.success = success;
        result.exit_code = outcome.exit_code;
        result.duration_ms = started.elapsed().as_millis() as i64;
        result.stdout_tail = log_tail;
        result.stderr_tail = None;
        result.error_message = outcome.error;
        result
    }
}

fn translate_download_error(e: &DownloadError) -> String {
    match e {
        DownloadError::NotConfigured => {
            "This agent is not configured to download package artifacts.".to_string()
        }
        DownloadError::InsecureUrl(_) => "Refusing to fetch MSI over an insecure URL.".to_string(),
        DownloadError::SizeLimitExceeded(cap) => {
            format!("Downloaded MSI exceeded the {cap}-byte size cap.")
        }
        DownloadError::HttpStatus(s) => format!("MSI download returned HTTP {s}."),
        DownloadError::Io(s) => format!("MSI download local I/O error: {s}"),
        // Note: the transport error is forwarded verbatim by contract
        // it MUST NOT contain secret material.
        DownloadError::Transport(s) => format!("MSI download transport error: {s}"),
    }
}

fn cleanup(file: &std::path::Path, log: &std::path::Path) {
    let _ = fs::remove_file(file);
    let _ = fs::remove_file(log);
}

fn read_log_tail(path: &std::path::Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let start = bytes.len().saturating_sub(MSI_LOG_TAIL_BYTES);
    Some(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::super::process::ProcessOutcome;
    use super::*;

    /// Records every command + canned outcome.
    struct FakeRunner {
        outcome: ProcessOutcome,
        last: Mutex<Option<ProcessCommand>>,
    }
    impl FakeRunner {
        fn new(outcome: ProcessOutcome) -> Arc<Self> {
            Arc::new(Self {
                outcome,
                last: Mutex::new(None),
            })
        }
    }
    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: ProcessCommand) -> ProcessOutcome {
            *self.last.lock().unwrap() = Some(command);
            self.outcome.clone()
        }
    }

    /// Writes `bytes` into the requested destination on `download`.
    struct FakeDownloader {
        bytes: Vec<u8>,
        last: Mutex<Option<DownloadRequest>>,
        canned: Mutex<Option<DownloadError>>,
    }
    impl FakeDownloader {
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
    impl ArtifactDownloader for FakeDownloader {
        async fn download(
            &self,
            request: DownloadRequest,
        ) -> Result<super::super::download::DownloadedArtifact, DownloadError> {
            *self.last.lock().unwrap() = Some(request.clone());
            if let Some(e) = self.canned.lock().unwrap().take() {
                return Err(e);
            }
            let dest = request.destination_dir.join(&request.file_name);
            fs::write(&dest, &self.bytes).map_err(|e| DownloadError::Io(e.to_string()))?;
            Ok(super::super::download::DownloadedArtifact {
                path: dest,
                bytes_len: self.bytes.len() as u64,
            })
        }
    }

    struct FakeEnv {
        msiexec: Option<PathBuf>,
        cache: PathBuf,
        host: Option<String>,
    }
    impl MsiEnvironment for FakeEnv {
        fn resolve_msiexec(&self) -> Option<PathBuf> {
            self.msiexec.clone()
        }
        fn cache_dir(&self) -> PathBuf {
            self.cache.clone()
        }
        fn server_host(&self) -> Option<String> {
            self.host.clone()
        }
    }

    fn ole2_bytes() -> Vec<u8> {
        let mut v = Vec::from(super::super::OLE2_MAGIC);
        v.extend_from_slice(&[0u8; 4096]);
        v
    }

    fn req(action: PackageInstallAction, sha: &str) -> PackageInstallRequest {
        PackageInstallRequest {
            job_id: "j".into(),
            provider: PackageProvider::UploadedMsi,
            action,
            package_identifier: "pkg".into(),
            msi_shared_file_id: Some("shared-1".into()),
            msi_auth_token: Some("tok".into()),
            msi_sha256: Some(sha.into()),
            msi_file_name: Some("setup.msi".into()),
            ..Default::default()
        }
    }

    fn provider(
        env: Arc<dyn MsiEnvironment>,
        runner: Arc<dyn ProcessRunner>,
        dl: Arc<dyn ArtifactDownloader>,
    ) -> UploadedMsiPackageProvider {
        UploadedMsiPackageProvider::new_with(
            env,
            runner,
            dl,
            Duration::from_secs(60),
            Duration::from_secs(60),
            MAX_MSI_BYTES,
        )
    }

    fn env_with(tmp: &TempDir, host: Option<&str>, msiexec: Option<PathBuf>) -> Arc<FakeEnv> {
        Arc::new(FakeEnv {
            msiexec,
            cache: tmp.path().to_path_buf(),
            host: host.map(|s| s.to_string()),
        })
    }

    #[tokio::test]
    async fn provider_mismatch_refused_without_download() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env, runner, dl.clone());
        let mut request = req(PackageInstallAction::Install, &"a".repeat(64));
        request.provider = PackageProvider::Chocolatey;
        let r = p.execute(&request).await;
        assert!(!r.success);
        assert_eq!(r.error_message.as_deref(), Some("Provider mismatch."));
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_metadata_refused() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env, runner, dl.clone());
        let mut request = req(PackageInstallAction::Install, &"a".repeat(64));
        request.msi_sha256 = None;
        let r = p.execute(&request).await;
        assert!(r.error_message.unwrap().contains("metadata"));
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn unsafe_filename_refused() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env, runner, dl.clone());
        let mut request = req(PackageInstallAction::Install, &"a".repeat(64));
        request.msi_file_name = Some("../../etc/passwd".into());
        let r = p.execute(&request).await;
        assert!(r.error_message.unwrap().contains("disallowed"));
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_msiexec_refused() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(&tmp, Some("https://srv"), None);
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env, runner, dl.clone());
        let r = p
            .execute(&req(PackageInstallAction::Install, &"a".repeat(64)))
            .await;
        assert!(r.error_message.unwrap().contains("msiexec"));
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_host_refused() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(&tmp, None, Some(PathBuf::from("X:\\msiexec.exe")));
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env, runner, dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &"a".repeat(64)))
            .await;
        assert!(r.error_message.unwrap().contains("host"));
    }

    #[tokio::test]
    async fn download_failure_translates_into_operator_message() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::fail(DownloadError::NotConfigured);
        let p = provider(env, runner.clone(), dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &"a".repeat(64)))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("not configured"));
        // msiexec must not have been invoked.
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn magic_byte_failure_refuses_install() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        // Bytes that are NOT an OLE2 file.
        let bytes = b"MZthis-looks-like-an-exe".to_vec();
        let actual_sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env, runner.clone(), dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &actual_sha))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("magic-byte"));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn sha256_mismatch_refuses_install() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(ole2_bytes());
        let p = provider(env, runner.clone(), dl);
        // Wrong expected SHA.
        let r = p
            .execute(&req(PackageInstallAction::Install, &"0".repeat(64)))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SHA-256 mismatch"));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn happy_path_invokes_msiexec_with_expected_argv() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 0,
            ..Default::default()
        });
        let bytes = ole2_bytes();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env, runner.clone(), dl.clone());
        let mut request = req(PackageInstallAction::Install, &sha);
        request.install_arguments = Some("PROP=1".into());
        let r = p.execute(&request).await;
        assert!(r.success, "msg={:?}", r.error_message);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.program, PathBuf::from("X:\\msiexec.exe"));
        assert_eq!(last.args[0], "/i");
        assert_eq!(last.args[2], "/qn");
        assert_eq!(last.args[3], "/norestart");
        assert_eq!(last.args[4], "/L*v");
        assert!(last.args.contains(&"PROP=1".to_string()));

        // Download was passed the host + auth header.
        let dl_req = dl.last.lock().unwrap().clone().unwrap();
        assert_eq!(dl_req.url, "https://srv/API/FileSharing/shared-1");
        assert_eq!(
            dl_req.auth_header.unwrap(),
            ("X-Expiring-Token".to_string(), "tok".to_string())
        );
    }

    #[tokio::test]
    async fn uninstall_uses_x_switch() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 0,
            ..Default::default()
        });
        let bytes = ole2_bytes();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env, runner.clone(), dl);
        let r = p.execute(&req(PackageInstallAction::Uninstall, &sha)).await;
        assert!(r.success);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.args[0], "/x");
    }

    #[tokio::test]
    async fn reboot_required_exit_code_is_success() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 3010,
            ..Default::default()
        });
        let bytes = ole2_bytes();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env, runner, dl);
        let r = p.execute(&req(PackageInstallAction::Install, &sha)).await;
        assert!(r.success);
        assert_eq!(r.exit_code, 3010);
    }

    #[tokio::test]
    async fn nonzero_unknown_exit_code_attaches_log_tail() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );

        // Custom runner that, in addition to returning exit=1603,
        // writes a fake msiexec verbose log to the path it was given
        // before returning. This exercises the log-tail capture path.
        struct LogWritingRunner;
        #[async_trait]
        impl ProcessRunner for LogWritingRunner {
            async fn run(&self, command: ProcessCommand) -> ProcessOutcome {
                // The /L*v arg is at args[4]; the path is args[5].
                let log_path = PathBuf::from(&command.args[5]);
                let _ = fs::write(
                    &log_path,
                    "MSI (s) (FF:00) [00:00:00:000]: Note: 1: 2729\nERROR_INSTALL_FAILURE\n",
                );
                ProcessOutcome {
                    exit_code: 1603,
                    error: None,
                    duration_ms: 7,
                    ..Default::default()
                }
            }
        }
        let runner: Arc<dyn ProcessRunner> = Arc::new(LogWritingRunner);
        let bytes = ole2_bytes();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env, runner, dl);
        let r = p.execute(&req(PackageInstallAction::Install, &sha)).await;
        assert!(!r.success);
        assert_eq!(r.exit_code, 1603);
        let tail = r.stdout_tail.unwrap();
        assert!(tail.contains("ERROR_INSTALL_FAILURE"));
    }

    #[tokio::test]
    async fn temp_files_cleaned_up_after_success() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 0,
            ..Default::default()
        });
        let bytes = ole2_bytes();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env, runner, dl);
        let r = p.execute(&req(PackageInstallAction::Install, &sha)).await;
        assert!(r.success);
        // Cache dir should have no .msi or .log left behind.
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "leftover files: {entries:?}");
    }

    #[tokio::test]
    async fn temp_files_cleaned_up_after_failure() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 0,
            ..Default::default()
        });
        let dl = FakeDownloader::ok(b"not-an-msi".to_vec());
        let p = provider(env, runner, dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &"0".repeat(64)))
            .await;
        assert!(!r.success);
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "leftover files: {entries:?}");
    }

    #[test]
    fn msiexec_exit_classification_matches_spec() {
        for ok in &[0, 3010, 1641] {
            assert!(UploadedMsiPackageProvider::classify_msiexec_exit(*ok));
        }
        for bad in &[1, 1603, 1612, 1625, -1] {
            assert!(!UploadedMsiPackageProvider::classify_msiexec_exit(*bad));
        }
    }

    #[test]
    fn translate_download_error_strips_token_information() {
        // The translation must never echo an auth token. We don't
        // pass one here; we just assert each variant returns a
        // human-readable string without exposing internal details.
        for e in [
            DownloadError::NotConfigured,
            DownloadError::InsecureUrl("http://x".into()),
            DownloadError::SizeLimitExceeded(123),
            DownloadError::HttpStatus(403),
            DownloadError::Io("oops".into()),
            DownloadError::Transport("dns".into()),
        ] {
            let msg = translate_download_error(&e);
            assert!(!msg.is_empty());
            assert!(!msg.contains("X-Expiring-Token"));
        }
    }

    #[test]
    fn can_handle_requires_msiexec_and_metadata() {
        let tmp = TempDir::new().unwrap();
        let env = env_with(
            &tmp,
            Some("https://srv"),
            Some(PathBuf::from("X:\\msiexec.exe")),
        );
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env, runner, dl);
        assert!(p.can_handle(&req(PackageInstallAction::Install, &"a".repeat(64))));

        let mut without_sha = req(PackageInstallAction::Install, &"a".repeat(64));
        without_sha.msi_sha256 = None;
        assert!(!p.can_handle(&without_sha));

        let mut wrong_provider = req(PackageInstallAction::Install, &"a".repeat(64));
        wrong_provider.provider = PackageProvider::Chocolatey;
        assert!(!p.can_handle(&wrong_provider));
    }
}
