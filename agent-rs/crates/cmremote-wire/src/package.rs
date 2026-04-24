// Source: CMRemote, clean-room implementation.

//! DTOs for the package-manager hub method (slice R6).
//!
//! Re-derived from `docs/wire-protocol.md` ➜ *Method surface* ➜
//! `InstallPackage`, mirroring the .NET `PackageInstallRequestDto` /
//! `PackageInstallResultDto` / `PackageProvider` / `PackageInstallAction`
//! shapes byte-for-byte so that a server speaking to either agent fleet
//! sees the same wire payload.
//!
//! No file copies: shapes are re-derived from the spec; serde
//! `rename_all = "PascalCase"` is the standard wire spelling for these
//! envelopes (see also `cmremote_platform::DeviceSnapshot`).

use serde::{Deserialize, Serialize};

/// Identifies which agent-side provider is responsible for installing
/// or uninstalling a package. Mirrors `Remotely.Shared.Enums.PackageProvider`.
///
/// `Unknown` is the default; the agent rejects requests with the
/// default value rather than guessing a provider — fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum PackageProvider {
    /// Unset — the request is malformed and must be rejected.
    #[default]
    Unknown,
    /// Chocolatey (`choco install -y --no-progress <id>`). Windows-only.
    Chocolatey,
    /// Org-uploaded MSI from the `UploadedMsis` library, installed via
    /// `msiexec /i <file> /qn /norestart`.
    UploadedMsi,
    /// Operator-defined executable + silent-install switches, fetched
    /// as a SharedFile and executed.
    Executable,
}

/// Whether the package operation is an install or an uninstall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum PackageInstallAction {
    /// Install or upgrade the package.
    #[default]
    Install,
    /// Uninstall the package.
    Uninstall,
}

/// Wire payload describing a single package install/uninstall request
/// sent from the server to an agent.
///
/// **Security contract.** The agent re-resolves the actual command
/// line locally (Chocolatey package id → `choco install …`, MSI shared
/// file id → `msiexec /i <path>`); it MUST NOT exec a string from the
/// wire as a shell command. `PackageIdentifier`, `Version`, and
/// `MsiFileName` are validated against narrow allow-lists before they
/// are passed as discrete argv slots — never via a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PackageInstallRequest {
    /// Server-side job id; echoed verbatim in the result.
    pub job_id: String,
    /// Provider that should service the request.
    pub provider: PackageProvider,
    /// Install or uninstall.
    pub action: PackageInstallAction,
    /// Provider-specific identifier (Chocolatey package id, MSI file
    /// id, etc.).
    pub package_identifier: String,
    /// Optional version pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Operator-supplied install arguments. Already validated server-side
    /// for shell metacharacters; the agent additionally splits them on
    /// whitespace into discrete argv slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_arguments: Option<String>,
    /// SharedFile id the agent fetches from
    /// `<server>/api/filesharing/{MsiSharedFileId}`. Populated only when
    /// `Provider == UploadedMsi`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msi_shared_file_id: Option<String>,
    /// Short-lived expiring auth token presented in the
    /// `X-Expiring-Token` header when fetching `MsiSharedFileId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msi_auth_token: Option<String>,
    /// Lowercase hex SHA-256 of the MSI bytes recorded at upload time.
    /// The agent re-hashes what it downloads and refuses to install on
    /// mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msi_sha256: Option<String>,
    /// Operator-uploaded filename, used only as the on-disk leaf name
    /// the agent writes to before invoking `msiexec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msi_file_name: Option<String>,
}

/// Wire payload reporting the outcome of a [`PackageInstallRequest`]
/// from agent to server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PackageInstallResult {
    /// Echoes [`PackageInstallRequest::job_id`].
    pub job_id: String,
    /// `true` when the package operation reached the desired post-state.
    pub success: bool,
    /// Underlying installer exit code (`-1` if the installer never ran).
    pub exit_code: i32,
    /// Wall-clock duration of the operation in milliseconds.
    pub duration_ms: i64,
    /// Tail of stdout from the installer (capped server-side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_tail: Option<String>,
    /// Tail of stderr from the installer (capped server-side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    /// Operator-facing failure message (provider-not-supported,
    /// SHA-256 mismatch, magic-byte rejection, timeout, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl PackageInstallResult {
    /// Build a failure result with `success=false`, `exit_code=-1`, and
    /// the supplied operator-facing message.
    pub fn failed(job_id: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            success: false,
            exit_code: -1,
            duration_ms: 0,
            stdout_tail: None,
            stderr_tail: None,
            error_message: Some(error_message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_round_trip_pascal_case() {
        let json = serde_json::to_string(&PackageProvider::UploadedMsi).unwrap();
        assert_eq!(json, "\"UploadedMsi\"");
        let back: PackageProvider = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PackageProvider::UploadedMsi);
    }

    #[test]
    fn provider_default_is_unknown_so_we_fail_closed() {
        assert_eq!(PackageProvider::default(), PackageProvider::Unknown);
    }

    #[test]
    fn action_round_trip_pascal_case() {
        let json = serde_json::to_string(&PackageInstallAction::Uninstall).unwrap();
        assert_eq!(json, "\"Uninstall\"");
    }

    #[test]
    fn request_round_trip_minimal() {
        let req = PackageInstallRequest {
            job_id: "job-1".into(),
            provider: PackageProvider::Chocolatey,
            action: PackageInstallAction::Install,
            package_identifier: "googlechrome".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        // Optional None fields must be omitted so the wire stays narrow.
        assert!(!json.contains("Version"));
        assert!(!json.contains("MsiSharedFileId"));
        let back: PackageInstallRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn request_round_trip_uploaded_msi_full() {
        let req = PackageInstallRequest {
            job_id: "job-2".into(),
            provider: PackageProvider::UploadedMsi,
            action: PackageInstallAction::Install,
            package_identifier: "msi-uuid".into(),
            version: None,
            install_arguments: Some("/quiet".into()),
            msi_shared_file_id: Some("shared-1".into()),
            msi_auth_token: Some("token".into()),
            msi_sha256: Some("a".repeat(64)),
            msi_file_name: Some("setup.msi".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"MsiSharedFileId\":\"shared-1\""));
        assert!(json.contains("\"MsiSha256\":"));
        let back: PackageInstallRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn result_failed_helper_sets_failure_invariants() {
        let r = PackageInstallResult::failed("job-3", "Provider mismatch.");
        assert_eq!(r.job_id, "job-3");
        assert!(!r.success);
        assert_eq!(r.exit_code, -1);
        assert_eq!(r.error_message.as_deref(), Some("Provider mismatch."));
    }

    #[test]
    fn result_round_trip_pascal_case() {
        let r = PackageInstallResult {
            job_id: "job-4".into(),
            success: true,
            exit_code: 3010,
            duration_ms: 12_345,
            stdout_tail: Some("done".into()),
            stderr_tail: None,
            error_message: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"JobId\":\"job-4\""));
        assert!(json.contains("\"ExitCode\":3010"));
        // Optional None fields must be omitted.
        assert!(!json.contains("StderrTail"));
        assert!(!json.contains("ErrorMessage"));
        let back: PackageInstallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }
}
