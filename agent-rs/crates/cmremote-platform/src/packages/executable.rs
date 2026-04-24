// Source: CMRemote, clean-room implementation.

//! Executable-package [`PackageProviderHandler`] implementation
//! (slice R6).
//!
//! Workflow:
//!
//! 1. Validate the wire metadata (filename, SHA-256, host).
//! 2. Pull the bytes via the injected [`ArtifactDownloader`] into a
//!    cache directory under the agent's data path.
//! 3. Re-hash the downloaded bytes with SHA-256 (no magic-byte check
//!    — this is a generic executable, not a CFB/OLE2 file).
//! 4. Run the executable with the operator-supplied silent-install
//!    arguments split on whitespace into discrete argv slots.
//! 5. Best-effort delete the downloaded executable before returning.
//!
//! Unlike the MSI provider this one does NOT verify a magic byte; the
//! `Executable` provider is the operator's "install whatever the
//! vendor ships" lane (vendor `.exe` installers, NSIS bundles, Inno
//! Setup wrappers, ...). The SHA-256 lock from the publisher manifest
//! is the integrity floor.

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
use super::{compute_sha256_hex, ct_eq_hex, is_safe_msi_file_name, PackageProviderHandler};

/// Hard wall-clock cap for a single executable-package install. Same
/// 60 minutes as `msiexec`; vendor installers can be slow.
pub const EXECUTABLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Hard wall-clock cap for the artifact download. 15 minutes.
pub const EXECUTABLE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Hard cap on the size of a single executable artifact (1 GiB).
pub const MAX_EXECUTABLE_BYTES: u64 = 1024 * 1024 * 1024;

/// Resolves the cache directory + server host the executable provider
/// needs. Mirrors [`super::msi::MsiEnvironment`] but for the
/// `.exe` lane.
pub trait ExecutableEnvironment: Send + Sync {
    /// Cache directory the provider stages downloads into.
    fn cache_dir(&self) -> PathBuf;
    /// Server base URL (no trailing slash). `None` ⇒ refuse the job.
    fn server_host(&self) -> Option<String>;
}

/// Default environment probe.
#[derive(Debug, Clone)]
pub struct StdExecutableEnvironment {
    cache_dir: PathBuf,
    server_host: Option<String>,
}

impl StdExecutableEnvironment {
    /// Construct a probe with explicit values; wired by the runtime.
    pub fn new(cache_dir: PathBuf, server_host: Option<String>) -> Self {
        Self {
            cache_dir,
            server_host,
        }
    }
}

impl ExecutableEnvironment for StdExecutableEnvironment {
    fn cache_dir(&self) -> PathBuf {
        self.cache_dir.clone()
    }
    fn server_host(&self) -> Option<String> {
        self.server_host.clone()
    }
}

/// Concrete Executable [`PackageProviderHandler`].
pub struct ExecutablePackageProvider {
    env: Arc<dyn ExecutableEnvironment>,
    runner: Arc<dyn ProcessRunner>,
    downloader: Arc<dyn ArtifactDownloader>,
    download_timeout: Duration,
    install_timeout: Duration,
    max_bytes: u64,
}

impl ExecutablePackageProvider {
    /// Default constructor — uses the OS-backed process runner and a
    /// rejecting downloader (replace with a real one once the HTTPS
    /// client lands).
    pub fn new(env: Arc<dyn ExecutableEnvironment>) -> Self {
        Self::new_with(
            env,
            Arc::new(TokioProcessRunner),
            Arc::new(RejectingDownloader),
            EXECUTABLE_DOWNLOAD_TIMEOUT,
            EXECUTABLE_TIMEOUT,
            MAX_EXECUTABLE_BYTES,
        )
    }

    /// Construct with explicit collaborators (used by tests + the
    /// runtime once a real downloader has been wired).
    pub fn new_with(
        env: Arc<dyn ExecutableEnvironment>,
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
}

#[async_trait]
impl PackageProviderHandler for ExecutablePackageProvider {
    fn can_handle(&self, request: &PackageInstallRequest) -> bool {
        request.provider == PackageProvider::Executable
            && request
                .msi_shared_file_id
                .as_deref()
                .is_some_and(|s| !s.is_empty())
            && request.msi_sha256.as_deref().is_some_and(|s| !s.is_empty())
    }

    async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
        let started = Instant::now();
        let mut result = PackageInstallResult::failed(request.job_id.clone(), "");

        if request.provider != PackageProvider::Executable {
            result.error_message = Some("Provider mismatch.".into());
            return result;
        }
        if request.action == PackageInstallAction::Uninstall {
            // Vendor `.exe` installers don't share a uninstall switch
            // surface; uninstall is the operator's lane via the
            // installed-applications path (slice R5).
            result.error_message =
                Some("Uninstall is not supported for the Executable provider.".into());
            return result;
        }

        let shared_id = match request
            .msi_shared_file_id
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            Some(s) => s,
            None => {
                result.error_message = Some("Executable download metadata is missing.".into());
                return result;
            }
        };
        let expected_sha = match request.msi_sha256.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => {
                result.error_message = Some("Executable download metadata is missing.".into());
                return result;
            }
        };
        let auth_token = request.msi_auth_token.as_deref().unwrap_or("");
        let leaf = request.msi_file_name.as_deref().unwrap_or("setup.exe");
        if !is_safe_msi_file_name(leaf) {
            result.error_message =
                Some("Executable filename contains disallowed characters.".into());
            return result;
        }
        let host = match self.env.server_host() {
            Some(h) if !h.is_empty() => h.trim_end_matches('/').to_string(),
            _ => {
                result.error_message = Some("Server host is not configured.".into());
                return result;
            }
        };

        let cache_dir = self.env.cache_dir();
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            result.error_message = Some(format!("Failed to prepare cache directory: {e}"));
            return result;
        }

        let unique = uuid::Uuid::new_v4().simple().to_string();
        let local_file = cache_dir.join(format!("{unique}_{leaf}"));

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
            "Executable package job starting (download phase)"
        );

        let downloaded = match self.downloader.download(dl_request).await {
            Ok(a) => a,
            Err(e) => {
                result.error_message = Some(translate(&e));
                result.duration_ms = started.elapsed().as_millis() as i64;
                let _ = fs::remove_file(&local_file);
                return result;
            }
        };
        let actual_path = downloaded.path;

        // SHA-256 verification (no magic-byte check).
        let bytes = match fs::read(&actual_path) {
            Ok(b) => b,
            Err(e) => {
                result.error_message =
                    Some(format!("Failed to re-read downloaded executable: {e}"));
                let _ = fs::remove_file(&actual_path);
                result.duration_ms = started.elapsed().as_millis() as i64;
                return result;
            }
        };
        let actual_sha = compute_sha256_hex(&bytes);
        if !ct_eq_hex(expected_sha, &actual_sha) {
            warn!(
                job_id = %request.job_id,
                "SHA-256 mismatch on downloaded executable; refusing to run"
            );
            result.error_message = Some("SHA-256 mismatch — refusing to run.".into());
            let _ = fs::remove_file(&actual_path);
            result.duration_ms = started.elapsed().as_millis() as i64;
            return result;
        }

        // Make the file executable on Unix so a sandboxed Linux test
        // host can exercise the path. On Windows the OS only requires
        // the .exe extension which the leaf already carries.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&actual_path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o700);
                let _ = fs::set_permissions(&actual_path, perms);
            }
        }

        // Build argv. The executable's argv[0] is the program path
        // itself (the OS supplies it); operator silent-install
        // arguments are split on whitespace into discrete slots.
        let mut argv: Vec<String> = Vec::with_capacity(8);
        if let Some(s) = request.install_arguments.as_deref() {
            for part in s.split_whitespace() {
                argv.push(part.to_string());
            }
        }

        info!(
            job_id = %request.job_id,
            file = %actual_path.display(),
            "executable package install starting"
        );
        let outcome = self
            .runner
            .run(ProcessCommand::new(
                actual_path.clone(),
                argv,
                self.install_timeout,
            ))
            .await;

        let _ = fs::remove_file(&actual_path);

        let success = outcome.error.is_none() && outcome.exit_code == 0;
        result.success = success;
        result.exit_code = outcome.exit_code;
        result.duration_ms = started.elapsed().as_millis() as i64;
        result.stdout_tail = outcome.stdout;
        result.stderr_tail = outcome.stderr;
        result.error_message = outcome.error;
        result
    }
}

fn translate(e: &DownloadError) -> String {
    match e {
        DownloadError::NotConfigured => {
            "This agent is not configured to download package artifacts.".to_string()
        }
        DownloadError::InsecureUrl(_) => {
            "Refusing to fetch executable over an insecure URL.".to_string()
        }
        DownloadError::SizeLimitExceeded(cap) => {
            format!("Downloaded executable exceeded the {cap}-byte size cap.")
        }
        DownloadError::HttpStatus(s) => format!("Executable download returned HTTP {s}."),
        DownloadError::Io(s) => format!("Executable download local I/O error: {s}"),
        DownloadError::Transport(s) => format!("Executable download transport error: {s}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::super::download::DownloadedArtifact;
    use super::super::process::ProcessOutcome;
    use super::*;

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

    struct FakeEnv {
        cache: PathBuf,
        host: Option<String>,
    }
    impl ExecutableEnvironment for FakeEnv {
        fn cache_dir(&self) -> PathBuf {
            self.cache.clone()
        }
        fn server_host(&self) -> Option<String> {
            self.host.clone()
        }
    }

    fn req(action: PackageInstallAction, sha: &str) -> PackageInstallRequest {
        PackageInstallRequest {
            job_id: "j".into(),
            provider: PackageProvider::Executable,
            action,
            package_identifier: "pkg".into(),
            msi_shared_file_id: Some("shared-1".into()),
            msi_auth_token: Some("tok".into()),
            msi_sha256: Some(sha.into()),
            msi_file_name: Some("setup.exe".into()),
            install_arguments: Some("/S /quiet".into()),
            ..Default::default()
        }
    }

    fn provider(
        env: Arc<dyn ExecutableEnvironment>,
        runner: Arc<dyn ProcessRunner>,
        dl: Arc<dyn ArtifactDownloader>,
    ) -> ExecutablePackageProvider {
        ExecutablePackageProvider::new_with(
            env,
            runner,
            dl,
            Duration::from_secs(60),
            Duration::from_secs(60),
            MAX_EXECUTABLE_BYTES,
        )
    }

    fn env(tmp: &TempDir, host: Option<&str>) -> Arc<FakeEnv> {
        Arc::new(FakeEnv {
            cache: tmp.path().to_path_buf(),
            host: host.map(|s| s.to_string()),
        })
    }

    #[tokio::test]
    async fn provider_mismatch_refused() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env(&tmp, Some("https://srv")), runner, dl.clone());
        let mut request = req(PackageInstallAction::Install, &"a".repeat(64));
        request.provider = PackageProvider::Chocolatey;
        let r = p.execute(&request).await;
        assert!(!r.success);
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn uninstall_refused() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env(&tmp, Some("https://srv")), runner, dl.clone());
        let r = p
            .execute(&req(PackageInstallAction::Uninstall, &"a".repeat(64)))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("Uninstall"));
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn unsafe_filename_refused() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env(&tmp, Some("https://srv")), runner, dl.clone());
        let mut request = req(PackageInstallAction::Install, &"a".repeat(64));
        request.msi_file_name = Some("..\\..\\evil.exe".into());
        let r = p.execute(&request).await;
        assert!(r.error_message.unwrap().contains("disallowed"));
        assert!(dl.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_host_refused() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env(&tmp, None), runner, dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &"a".repeat(64)))
            .await;
        assert!(r.error_message.unwrap().contains("host"));
    }

    #[tokio::test]
    async fn download_failure_translates() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::fail(DownloadError::NotConfigured);
        let p = provider(env(&tmp, Some("https://srv")), runner.clone(), dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &"a".repeat(64)))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("not configured"));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn sha256_mismatch_refuses_run() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(b"some bytes".to_vec());
        let p = provider(env(&tmp, Some("https://srv")), runner.clone(), dl);
        let r = p
            .execute(&req(PackageInstallAction::Install, &"0".repeat(64)))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("SHA-256 mismatch"));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn happy_path_runs_executable_with_split_argv_and_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 0,
            ..Default::default()
        });
        let bytes = b"#!/bin/sh\nexit 0\n".to_vec();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env(&tmp, Some("https://srv")), runner.clone(), dl.clone());
        let r = p.execute(&req(PackageInstallAction::Install, &sha)).await;
        assert!(r.success, "msg={:?}", r.error_message);
        let last = runner.last.lock().unwrap().clone().unwrap();
        // argv[0] = program path is supplied by the OS, not in args.
        assert_eq!(last.args, vec!["/S".to_string(), "/quiet".into()]);

        // Download URL was assembled correctly.
        let dl_req = dl.last.lock().unwrap().clone().unwrap();
        assert_eq!(dl_req.url, "https://srv/API/FileSharing/shared-1");

        // No leftover files in the cache dir.
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "leftover files: {entries:?}");
    }

    #[tokio::test]
    async fn nonzero_exit_code_is_failure() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome {
            exit_code: 1,
            ..Default::default()
        });
        let bytes = b"setup payload".to_vec();
        let sha = compute_sha256_hex(&bytes);
        let dl = FakeDownloader::ok(bytes);
        let p = provider(env(&tmp, Some("https://srv")), runner, dl);
        let r = p.execute(&req(PackageInstallAction::Install, &sha)).await;
        assert!(!r.success);
        assert_eq!(r.exit_code, 1);
    }

    #[tokio::test]
    async fn can_handle_requires_metadata_and_provider_match() {
        let tmp = TempDir::new().unwrap();
        let runner = FakeRunner::new(ProcessOutcome::default());
        let dl = FakeDownloader::ok(vec![]);
        let p = provider(env(&tmp, Some("https://srv")), runner, dl);
        assert!(p.can_handle(&req(PackageInstallAction::Install, &"a".repeat(64))));
        let mut without = req(PackageInstallAction::Install, &"a".repeat(64));
        without.msi_sha256 = None;
        assert!(!p.can_handle(&without));
        let mut wrong = req(PackageInstallAction::Install, &"a".repeat(64));
        wrong.provider = PackageProvider::UploadedMsi;
        assert!(!p.can_handle(&wrong));
    }
}
