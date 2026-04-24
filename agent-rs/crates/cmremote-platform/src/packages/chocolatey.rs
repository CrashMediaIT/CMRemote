// Source: CMRemote, clean-room implementation.

//! Chocolatey-backed [`PackageProviderHandler`] implementation
//! (slice R6).
//!
//! Re-derived from the spec of the .NET `ChocolateyPackageProvider`:
//! resolve `choco.exe` from `%ChocolateyInstall%\bin\` or `PATH`,
//! build a `choco install` / `choco uninstall` argv with each token
//! in a discrete slot, and run it under a 30-minute hard deadline.
//!
//! No source from the .NET reference is copied; this is an
//! independent implementation written against the contract.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cmremote_wire::{
    PackageInstallAction, PackageInstallRequest, PackageInstallResult, PackageProvider,
};
use tracing::{info, warn};

use super::process::{ProcessCommand, ProcessRunner, TokioProcessRunner};
use super::{
    is_chocolatey_success_exit_code, is_safe_chocolatey_package_id, is_safe_chocolatey_version,
    PackageProviderHandler,
};

/// Hard wall-clock cap for a single `choco` invocation. Mirrors the
/// .NET reference (`ExecutionTimeout = 30 min`); large packages
/// (Office, VS Build Tools) can comfortably exceed 10 minutes on a
/// fresh install but should never legitimately exceed 30.
pub const CHOCOLATEY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Resolves the on-disk path of `choco.exe`. Implementations must not
/// spawn a child process — the lookup runs inside `can_handle` which
/// is on the dispatcher hot path.
pub trait ChocolateyEnvironment: Send + Sync {
    /// Returns `Some(path)` when `choco.exe` is installed on this host.
    fn resolve_choco(&self) -> Option<PathBuf>;
}

/// Default environment probe. On Windows it walks `%ChocolateyInstall%\bin\`
/// and then every entry on `PATH`; on every other OS it returns `None`
/// because `choco.exe` is Windows-only.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdChocolateyEnvironment;

impl ChocolateyEnvironment for StdChocolateyEnvironment {
    fn resolve_choco(&self) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            if let Ok(install_dir) = std::env::var("ChocolateyInstall") {
                let candidate = PathBuf::from(install_dir).join("bin").join("choco.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            if let Some(path_var) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path_var) {
                    let candidate = dir.join("choco.exe");
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
            None
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }
}

/// Concrete Chocolatey [`PackageProviderHandler`].
pub struct ChocolateyPackageProvider {
    env: Arc<dyn ChocolateyEnvironment>,
    runner: Arc<dyn ProcessRunner>,
    timeout: Duration,
}

impl ChocolateyPackageProvider {
    /// Construct a provider with the default OS-backed environment +
    /// process runner. Use [`Self::new_with`] in tests.
    pub fn new() -> Self {
        Self::new_with(
            Arc::new(StdChocolateyEnvironment),
            Arc::new(TokioProcessRunner),
            CHOCOLATEY_TIMEOUT,
        )
    }

    /// Construct a provider with explicit environment + runner. Used
    /// by unit tests to inject a fake `choco.exe` path and a fake
    /// process runner.
    pub fn new_with(
        env: Arc<dyn ChocolateyEnvironment>,
        runner: Arc<dyn ProcessRunner>,
        timeout: Duration,
    ) -> Self {
        Self {
            env,
            runner,
            timeout,
        }
    }

    fn build_argv(action: PackageInstallAction, request: &PackageInstallRequest) -> Vec<String> {
        let mut args: Vec<String> = Vec::with_capacity(16);
        args.push(
            match action {
                PackageInstallAction::Uninstall => "uninstall",
                PackageInstallAction::Install => "install",
            }
            .to_string(),
        );
        args.push(request.package_identifier.clone());
        args.push("--yes".into());
        args.push("--no-progress".into());
        args.push("--limit-output".into());
        args.push("--no-color".into());

        if let Some(v) = request.version.as_deref() {
            if !v.is_empty() && is_safe_chocolatey_version(v) {
                args.push("--version".into());
                args.push(v.to_string());
            }
        }

        if let Some(extra) = request.install_arguments.as_deref() {
            for part in extra.split_whitespace() {
                args.push(part.to_string());
            }
        }
        args
    }
}

impl Default for ChocolateyPackageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PackageProviderHandler for ChocolateyPackageProvider {
    fn can_handle(&self, request: &PackageInstallRequest) -> bool {
        request.provider == PackageProvider::Chocolatey && self.env.resolve_choco().is_some()
    }

    async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
        if request.provider != PackageProvider::Chocolatey {
            return PackageInstallResult::failed(request.job_id.clone(), "Provider mismatch.");
        }
        if request.package_identifier.is_empty() {
            return PackageInstallResult::failed(
                request.job_id.clone(),
                "Package identifier is required.",
            );
        }
        if !is_safe_chocolatey_package_id(&request.package_identifier) {
            return PackageInstallResult::failed(
                request.job_id.clone(),
                "Package identifier contains disallowed characters.",
            );
        }
        let choco = match self.env.resolve_choco() {
            Some(p) => p,
            None => {
                return PackageInstallResult::failed(
                    request.job_id.clone(),
                    "Chocolatey (choco.exe) is not installed on this device.",
                );
            }
        };

        let argv = Self::build_argv(request.action, request);

        info!(
            job_id = %request.job_id,
            action = ?request.action,
            package_id = %request.package_identifier,
            version = request.version.as_deref().unwrap_or(""),
            "chocolatey package job starting"
        );

        let outcome = self
            .runner
            .run(ProcessCommand::new(choco, argv, self.timeout))
            .await;

        let mut result = PackageInstallResult {
            job_id: request.job_id.clone(),
            success: false,
            exit_code: outcome.exit_code,
            duration_ms: outcome.duration_ms,
            stdout_tail: outcome.stdout,
            stderr_tail: outcome.stderr,
            error_message: outcome.error,
        };
        result.success =
            result.error_message.is_none() && is_chocolatey_success_exit_code(outcome.exit_code);
        if !result.success && result.error_message.is_none() {
            warn!(
                job_id = %request.job_id,
                exit_code = outcome.exit_code,
                "chocolatey reported a non-success exit code"
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records the last command it was asked to run and returns a
    /// canned outcome.
    struct FakeRunner {
        canned: ProcessOutcomeKind,
        last: Mutex<Option<ProcessCommand>>,
    }

    enum ProcessOutcomeKind {
        Success(i32),
        Failure { exit: i32, error: Option<String> },
    }

    impl FakeRunner {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                canned: ProcessOutcomeKind::Success(0),
                last: Mutex::new(None),
            })
        }

        fn with_exit(code: i32) -> Arc<Self> {
            Arc::new(Self {
                canned: ProcessOutcomeKind::Success(code),
                last: Mutex::new(None),
            })
        }

        fn err(message: &str) -> Arc<Self> {
            Arc::new(Self {
                canned: ProcessOutcomeKind::Failure {
                    exit: -1,
                    error: Some(message.to_string()),
                },
                last: Mutex::new(None),
            })
        }
    }

    use super::super::process::ProcessOutcome;

    #[async_trait]
    impl ProcessRunner for FakeRunner {
        async fn run(&self, command: ProcessCommand) -> ProcessOutcome {
            *self.last.lock().unwrap() = Some(command);
            match &self.canned {
                ProcessOutcomeKind::Success(code) => ProcessOutcome {
                    exit_code: *code,
                    stdout: Some("ok".into()),
                    stderr: None,
                    error: None,
                    duration_ms: 42,
                },
                ProcessOutcomeKind::Failure { exit, error } => ProcessOutcome {
                    exit_code: *exit,
                    stdout: None,
                    stderr: None,
                    error: error.clone(),
                    duration_ms: 11,
                },
            }
        }
    }

    struct FakeEnv(Option<PathBuf>);
    impl ChocolateyEnvironment for FakeEnv {
        fn resolve_choco(&self) -> Option<PathBuf> {
            self.0.clone()
        }
    }

    fn req(action: PackageInstallAction, id: &str) -> PackageInstallRequest {
        PackageInstallRequest {
            job_id: "j".into(),
            provider: PackageProvider::Chocolatey,
            action,
            package_identifier: id.into(),
            ..Default::default()
        }
    }

    fn provider(
        env: Arc<dyn ChocolateyEnvironment>,
        runner: Arc<dyn ProcessRunner>,
    ) -> ChocolateyPackageProvider {
        ChocolateyPackageProvider::new_with(env, runner, Duration::from_secs(60))
    }

    #[tokio::test]
    async fn missing_choco_short_circuits() {
        let env = Arc::new(FakeEnv(None));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let r = p
            .execute(&req(PackageInstallAction::Install, "googlechrome"))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("not installed"));
        assert!(
            runner.last.lock().unwrap().is_none(),
            "runner must not be invoked"
        );
    }

    #[tokio::test]
    async fn provider_mismatch_refused_without_runner_call() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let mut request = req(PackageInstallAction::Install, "googlechrome");
        request.provider = PackageProvider::UploadedMsi;
        let r = p.execute(&request).await;
        assert!(!r.success);
        assert_eq!(r.error_message.as_deref(), Some("Provider mismatch."));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn unsafe_package_id_refused_without_runner_call() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let r = p
            .execute(&req(PackageInstallAction::Install, "evil; rm -rf /"))
            .await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("disallowed"));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_package_id_refused_without_runner_call() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let r = p.execute(&req(PackageInstallAction::Install, "")).await;
        assert!(!r.success);
        assert!(r.error_message.unwrap().contains("required"));
        assert!(runner.last.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn install_argv_pins_install_subcommand_and_flags() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let r = p
            .execute(&req(PackageInstallAction::Install, "googlechrome"))
            .await;
        assert!(r.success);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.program, PathBuf::from("C:\\choco.exe"));
        assert_eq!(
            last.args,
            vec![
                "install".to_string(),
                "googlechrome".into(),
                "--yes".into(),
                "--no-progress".into(),
                "--limit-output".into(),
                "--no-color".into(),
            ]
        );
    }

    #[tokio::test]
    async fn uninstall_argv_uses_uninstall_subcommand() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let r = p
            .execute(&req(PackageInstallAction::Uninstall, "googlechrome"))
            .await;
        assert!(r.success);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert_eq!(last.args[0], "uninstall");
        assert_eq!(last.args[1], "googlechrome");
    }

    #[tokio::test]
    async fn safe_version_appended_after_flags() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let mut request = req(PackageInstallAction::Install, "vscode");
        request.version = Some("1.93.1".into());
        let r = p.execute(&request).await;
        assert!(r.success);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert!(last
            .args
            .windows(2)
            .any(|w| w[0] == "--version" && w[1] == "1.93.1"));
    }

    #[tokio::test]
    async fn unsafe_version_silently_dropped() {
        // The package id is fine; only the version is malformed. We
        // drop the version rather than failing the whole job, matching
        // the .NET reference's `IsSafeVersion` guard.
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let mut request = req(PackageInstallAction::Install, "vscode");
        request.version = Some("1.0; rm -rf /".into());
        let r = p.execute(&request).await;
        assert!(r.success);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert!(!last.args.iter().any(|a| a == "--version"));
    }

    #[tokio::test]
    async fn install_arguments_split_on_whitespace_into_argv_slots() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::ok();
        let p = provider(env, runner.clone());
        let mut request = req(PackageInstallAction::Install, "vscode");
        request.install_arguments = Some("--ignore-checksums --force".into());
        let r = p.execute(&request).await;
        assert!(r.success);
        let last = runner.last.lock().unwrap().clone().unwrap();
        assert!(last.args.contains(&"--ignore-checksums".to_string()));
        assert!(last.args.contains(&"--force".to_string()));
    }

    #[tokio::test]
    async fn reboot_required_exit_code_is_success() {
        // 3010 = ERROR_SUCCESS_REBOOT_REQUIRED. The Chocolatey output
        // parser counts this as a successful operation.
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::with_exit(3010);
        let p = provider(env, runner.clone());
        let r = p
            .execute(&req(PackageInstallAction::Install, "vscode"))
            .await;
        assert!(r.success);
        assert_eq!(r.exit_code, 3010);
    }

    #[tokio::test]
    async fn nonzero_unknown_exit_code_is_failure() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::with_exit(1);
        let p = provider(env, runner.clone());
        let r = p
            .execute(&req(PackageInstallAction::Install, "vscode"))
            .await;
        assert!(!r.success);
        assert_eq!(r.exit_code, 1);
    }

    #[tokio::test]
    async fn runner_error_propagates_into_error_message() {
        let env = Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe"))));
        let runner = FakeRunner::err("Timed out.");
        let p = provider(env, runner);
        let r = p
            .execute(&req(PackageInstallAction::Install, "vscode"))
            .await;
        assert!(!r.success);
        assert_eq!(r.error_message.as_deref(), Some("Timed out."));
    }

    #[tokio::test]
    async fn can_handle_requires_choco_resolution_and_provider_match() {
        let with_choco = provider(
            Arc::new(FakeEnv(Some(PathBuf::from("C:\\choco.exe")))),
            FakeRunner::ok(),
        );
        let without_choco = provider(Arc::new(FakeEnv(None)), FakeRunner::ok());

        let r = req(PackageInstallAction::Install, "vscode");
        assert!(with_choco.can_handle(&r));
        assert!(!without_choco.can_handle(&r));

        let mut wrong = r.clone();
        wrong.provider = PackageProvider::UploadedMsi;
        assert!(!with_choco.can_handle(&wrong));
    }
}
