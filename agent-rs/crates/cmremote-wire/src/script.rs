// Source: CMRemote, clean-room implementation.

//! DTOs for the script / command-execution hub methods.
//!
//! Re-derived from `docs/wire-protocol.md` ➜ *Method surface* ➜
//! `ExecuteCommand`. The R4 slice adds the actual executor; these types
//! exist here so both the wire layer and the handler layer share the
//! same definitions without a crate-level circular dependency.

use serde::{Deserialize, Serialize};

/// Scripting shell the server may request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ScriptingShell {
    /// `/bin/bash`
    Bash,
    /// `/bin/sh`
    Sh,
    /// `/bin/zsh`
    Zsh,
    /// PowerShell Core (`pwsh`)
    Pwsh,
    /// `cmd.exe` — Windows-only
    Cmd,
}

impl ScriptingShell {
    /// Returns the binary name for this shell on POSIX, or `None` if
    /// the shell is not supported on the current platform.
    pub fn binary_name(self) -> Option<&'static str> {
        match self {
            Self::Bash => Some("bash"),
            Self::Sh => Some("sh"),
            Self::Zsh => Some("zsh"),
            Self::Pwsh => Some("pwsh"),
            // `cmd.exe` is Windows-only; on non-Windows platforms we
            // signal "not supported" by returning `None`.
            Self::Cmd => {
                if cfg!(target_os = "windows") {
                    Some("cmd")
                } else {
                    None
                }
            }
        }
    }
}

/// Arguments for the `ExecuteCommand` hub invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExecuteCommandArgs {
    /// Scripting shell to invoke.
    pub shell: ScriptingShell,
    /// Command content passed to the shell's `-c` flag.
    pub command: String,
    /// Connection ID of the browser/viewer that issued the command.
    pub connection_id: String,
    /// Hub connection ID of the sender.
    pub sender_connection_id: String,
    /// Human-readable sender username for audit logs.
    pub sender_user_name: String,
}

/// Result of a completed command execution, returned as the
/// `HubCompletion.result` payload for `ExecuteCommand`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ScriptResult {
    /// Shell that was used.
    pub shell: Option<ScriptingShell>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Process exit code. `None` when the process was killed or timed out.
    pub exit_code: Option<i32>,
    /// Non-`None` when the executor could not even launch the process.
    pub error_message: Option<String>,
    /// Set when the 5-minute execution deadline fired.
    pub timed_out: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_binary_name() {
        assert_eq!(ScriptingShell::Bash.binary_name(), Some("bash"));
    }

    #[test]
    fn cmd_unsupported_on_non_windows() {
        // This test runs on Linux CI; cmd should return None.
        if cfg!(not(target_os = "windows")) {
            assert_eq!(ScriptingShell::Cmd.binary_name(), None);
        }
    }

    #[test]
    fn script_result_round_trips() {
        let r = ScriptResult {
            shell: Some(ScriptingShell::Bash),
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: Some(0),
            error_message: None,
            timed_out: false,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ScriptResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
