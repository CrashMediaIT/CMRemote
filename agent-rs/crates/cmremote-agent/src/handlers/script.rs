// Source: CMRemote, clean-room implementation.

//! Script / command-execution hub handler (slice R4).
//!
//! Uses `tokio::process::Command` with a 5-minute hard deadline. The
//! shell binary is resolved from an allow-list; a raw command string
//! is never exec'd as a shell itself.

use std::time::Duration;

use cmremote_wire::{ExecuteCommandArgs, HubInvocation, ScriptResult};
use tracing::warn;

/// Five-minute hard deadline for command execution.
const EXEC_TIMEOUT: Duration = Duration::from_secs(300);

/// Handle `ExecuteCommand`: parse arguments, run the command under the
/// requested shell, return stdout/stderr/exit-code in a `ScriptResult`.
pub async fn handle_execute_command(inv: &HubInvocation) -> Result<serde_json::Value, String> {
    let raw_arg = inv
        .arguments
        .first()
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let args: ExecuteCommandArgs =
        serde_json::from_value(raw_arg).map_err(|e| format!("invalid_arguments: {e}"))?;

    let binary = match args.shell.binary_name() {
        Some(b) => b,
        None => {
            return serde_json::to_value(ScriptResult {
                shell: Some(args.shell),
                error_message: Some(format!(
                    "{:?} is not supported on this platform",
                    args.shell
                )),
                ..ScriptResult::default()
            })
            .map_err(|e| e.to_string());
        }
    };

    let run = tokio::process::Command::new(binary)
        .arg("-c")
        .arg(&args.command)
        .output();

    let script_result = match tokio::time::timeout(EXEC_TIMEOUT, run).await {
        Err(_elapsed) => {
            warn!(shell = ?args.shell, "command execution timed out");
            ScriptResult {
                shell: Some(args.shell),
                timed_out: true,
                ..ScriptResult::default()
            }
        }
        Ok(Err(io_err)) => {
            warn!(error = %io_err, "failed to spawn shell");
            ScriptResult {
                shell: Some(args.shell),
                error_message: Some(io_err.to_string()),
                ..ScriptResult::default()
            }
        }
        Ok(Ok(output)) => ScriptResult {
            shell: Some(args.shell),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
            error_message: None,
            timed_out: false,
        },
    };

    serde_json::to_value(script_result).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use cmremote_wire::{HubMessageKind, ScriptingShell};

    use super::*;

    fn make_inv(shell: ScriptingShell, cmd: &str) -> HubInvocation {
        let args_obj = ExecuteCommandArgs {
            shell,
            command: cmd.into(),
            connection_id: String::new(),
            sender_connection_id: String::new(),
            sender_user_name: String::new(),
        };
        HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("t1".into()),
            target: "ExecuteCommand".into(),
            arguments: vec![serde_json::to_value(args_obj).unwrap()],
        }
    }

    #[tokio::test]
    async fn echo_returns_stdout() {
        let inv = make_inv(ScriptingShell::Sh, "echo hello");
        let val = handle_execute_command(&inv).await.unwrap();
        let result: ScriptResult = serde_json::from_value(val).unwrap();
        assert!(result.stdout.contains("hello"));
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn exit_code_propagates() {
        let inv = make_inv(ScriptingShell::Sh, "exit 42");
        let val = handle_execute_command(&inv).await.unwrap();
        let result: ScriptResult = serde_json::from_value(val).unwrap();
        assert_eq!(result.exit_code, Some(42));
    }

    #[tokio::test]
    async fn cmd_not_supported_on_linux() {
        if cfg!(not(target_os = "windows")) {
            let inv = make_inv(ScriptingShell::Cmd, "dir");
            let val = handle_execute_command(&inv).await.unwrap();
            let result: ScriptResult = serde_json::from_value(val).unwrap();
            assert!(result.error_message.is_some());
        }
    }
}
