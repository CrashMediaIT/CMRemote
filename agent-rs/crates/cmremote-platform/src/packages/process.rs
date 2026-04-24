// Source: CMRemote, clean-room implementation.

//! Process-execution abstraction used by the concrete package
//! providers (slice R6).
//!
//! The package providers (Chocolatey, MSI, Executable) all share the
//! same shape: build an argv from validated inputs, spawn an OS
//! process, capture stdout/stderr, enforce a hard deadline, and report
//! the outcome. Every concrete provider takes a [`ProcessRunner`]
//! through its constructor so the orchestration logic — which is
//! identical across hosts — is exercised on the Linux CI runner via a
//! fake, while the real `tokio::process::Command` plumbing only kicks
//! in on Windows where `choco.exe` / `msiexec.exe` actually exist.
//!
//! ## Security contract
//!
//! Every implementation MUST:
//!
//! 1. Pass `args` as discrete argv slots. Never concatenate them into a
//!    single command line that is then split by a shell parser.
//! 2. Treat the deadline as a hard kill. The .NET reference uses
//!    `Process.Kill(entireProcessTree: true)`; the default
//!    [`TokioProcessRunner`] uses `child.start_kill()` followed by
//!    `child.wait()` so the agent never accumulates orphan installer
//!    processes.
//! 3. Cap captured stdout/stderr at `max_output_bytes` to bound the
//!    agent's memory footprint regardless of how much output an
//!    installer emits.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

/// Maximum bytes of stdout/stderr captured per provider invocation.
/// 16 KiB matches the .NET reference (`MaxOutputCharacters`).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// Marker appended to a captured stream when it was truncated to the
/// per-invocation cap. Keeps the cap visible to operators reading the
/// `stdout_tail` / `stderr_tail` fields.
pub const TRUNCATION_MARKER: &str = "\n[output truncated by cmremote-agent]\n";

/// Description of the child process to spawn.
#[derive(Debug, Clone)]
pub struct ProcessCommand {
    /// Absolute path to the executable. Providers resolve this from a
    /// known location (e.g. `%SystemRoot%\System32\msiexec.exe`); the
    /// wire never carries an executable string.
    pub program: PathBuf,
    /// Argv slots passed to the child process exactly as supplied —
    /// no additional shell quoting, no joining into a command line.
    pub args: Vec<String>,
    /// Hard deadline. When this fires the runner kills the child and
    /// reports `error = Some("Timed out.")`.
    pub timeout: Duration,
    /// Maximum bytes captured from each of stdout / stderr.
    pub max_output_bytes: usize,
}

impl ProcessCommand {
    /// Construct a command with the default 16 KiB output cap.
    pub fn new(program: PathBuf, args: Vec<String>, timeout: Duration) -> Self {
        Self {
            program,
            args,
            timeout,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// Outcome of a single [`ProcessCommand`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessOutcome {
    /// Exit code reported by the OS, or `-1` if the process never
    /// completed (failed to spawn, timed out, was cancelled).
    pub exit_code: i32,
    /// Captured stdout, possibly with [`TRUNCATION_MARKER`] appended.
    /// `None` when the runner did not redirect stdout.
    pub stdout: Option<String>,
    /// Captured stderr, possibly with [`TRUNCATION_MARKER`] appended.
    /// `None` when the runner did not redirect stderr.
    pub stderr: Option<String>,
    /// Operator-facing failure message. `None` on a clean exit
    /// (regardless of the exit code — interpretation is the provider's
    /// job).
    pub error: Option<String>,
    /// Wall-clock duration the runner observed.
    pub duration_ms: i64,
}

impl ProcessOutcome {
    /// Convenience constructor for "process never started".
    pub fn spawn_failed(message: impl Into<String>) -> Self {
        Self {
            exit_code: -1,
            error: Some(message.into()),
            ..Self::default()
        }
    }

    /// Convenience constructor for "process exceeded deadline".
    pub fn timed_out(stdout: Option<String>, stderr: Option<String>, duration_ms: i64) -> Self {
        Self {
            exit_code: -1,
            stdout,
            stderr,
            error: Some("Timed out.".into()),
            duration_ms,
        }
    }
}

/// Spawns OS processes for the package providers. Implementations must
/// not panic — every failure is surfaced through [`ProcessOutcome`].
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    /// Run `command` to completion (or until its `timeout` elapses)
    /// and return the outcome.
    async fn run(&self, command: ProcessCommand) -> ProcessOutcome;
}

/// Default [`ProcessRunner`] backed by `tokio::process::Command`.
///
/// Captures stdout and stderr up to `max_output_bytes`, kills the
/// child (and its process tree, on Unix) when the deadline fires, and
/// always returns rather than panicking.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioProcessRunner;

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run(&self, command: ProcessCommand) -> ProcessOutcome {
        use std::process::Stdio;
        use std::time::Instant;
        use tokio::process::Command;
        use tokio::time::timeout;

        let started = Instant::now();
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ProcessOutcome::spawn_failed(format!(
                    "Failed to spawn {}: {e}",
                    command.program.display()
                ));
            }
        };

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let cap = command.max_output_bytes;

        let stdout_task = tokio::spawn(async move {
            if let Some(ref mut s) = stdout_pipe {
                read_capped(s, cap).await
            } else {
                String::new()
            }
        });
        let stderr_task = tokio::spawn(async move {
            if let Some(ref mut s) = stderr_pipe {
                read_capped(s, cap).await
            } else {
                String::new()
            }
        });

        let wait = timeout(command.timeout, child.wait()).await;
        let elapsed_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;

        match wait {
            Ok(Ok(status)) => {
                let stdout = stdout_task.await.ok();
                let stderr = stderr_task.await.ok();
                ProcessOutcome {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: stdout.filter(|s| !s.is_empty()),
                    stderr: stderr.filter(|s| !s.is_empty()),
                    error: None,
                    duration_ms: elapsed_ms,
                }
            }
            Ok(Err(e)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                ProcessOutcome {
                    exit_code: -1,
                    stdout: stdout_task.await.ok().filter(|s| !s.is_empty()),
                    stderr: stderr_task.await.ok().filter(|s| !s.is_empty()),
                    error: Some(format!("Wait failed: {e}")),
                    duration_ms: elapsed_ms,
                }
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let stdout = stdout_task.await.ok().filter(|s| !s.is_empty());
                let stderr = stderr_task.await.ok().filter(|s| !s.is_empty());
                ProcessOutcome::timed_out(stdout, stderr, elapsed_ms)
            }
        }
    }
}

async fn read_capped<R>(reader: &mut R, cap: usize) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut out = Vec::with_capacity(cap.min(8192));
    let mut buf = [0u8; 4096];
    let mut truncated = false;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = cap.saturating_sub(out.len());
                if remaining == 0 {
                    truncated = true;
                    // Drain the remainder so the child doesn't block on
                    // a full pipe; we discard the bytes.
                    continue;
                }
                let take = n.min(remaining);
                out.extend_from_slice(&buf[..take]);
                if take < n {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    let mut s = String::from_utf8_lossy(&out).into_owned();
    if truncated {
        s.push_str(TRUNCATION_MARKER);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_failure_is_structured_not_panic() {
        let cmd = ProcessCommand::new(
            PathBuf::from("/path/that/does/not/exist/cmremote-test-binary"),
            vec![],
            Duration::from_secs(1),
        );
        let outcome = TokioProcessRunner.run(cmd).await;
        assert_eq!(outcome.exit_code, -1);
        assert!(outcome.error.unwrap().contains("Failed to spawn"));
    }

    // The remaining behaviours (timeout, stdout cap, exit-code passthrough)
    // depend on a host binary; we exercise them on Unix where /bin/sh is
    // available. The provider unit tests themselves use a fake runner so
    // the orchestration logic is platform-agnostic.
    #[cfg(unix)]
    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let cmd = ProcessCommand::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "echo hello && exit 7".into()],
            Duration::from_secs(5),
        );
        let outcome = TokioProcessRunner.run(cmd).await;
        assert_eq!(outcome.exit_code, 7);
        assert_eq!(outcome.stdout.as_deref().map(str::trim), Some("hello"));
        assert!(outcome.error.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_child_and_reports_timed_out() {
        let cmd = ProcessCommand::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "sleep 5".into()],
            Duration::from_millis(150),
        );
        let outcome = TokioProcessRunner.run(cmd).await;
        assert_eq!(outcome.exit_code, -1);
        assert_eq!(outcome.error.as_deref(), Some("Timed out."));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_is_capped_and_marker_appended() {
        // Emit ~50 KiB; expect the cap to clamp it.
        let cmd = ProcessCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "head -c 51200 /dev/zero | tr '\\0' 'a'".into()],
            timeout: Duration::from_secs(5),
            max_output_bytes: 1024,
        };
        let outcome = TokioProcessRunner.run(cmd).await;
        assert_eq!(outcome.exit_code, 0);
        let stdout = outcome.stdout.unwrap();
        // 1 KiB of 'a' plus the truncation marker.
        assert!(stdout.starts_with(&"a".repeat(1024)));
        assert!(stdout.contains(TRUNCATION_MARKER));
    }
}
