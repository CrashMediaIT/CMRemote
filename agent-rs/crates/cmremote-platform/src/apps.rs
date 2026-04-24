// Source: CMRemote, clean-room implementation.

//! Installed-applications provider trait and types (slice R5).
//!
//! Re-derived from `Agent/Interfaces/IInstalledApplicationsProvider.cs`
//! in the .NET agent and `Shared/Models/InstalledApplication.cs`.

use serde::{Deserialize, Serialize};

use crate::PlatformError;

/// A single installed application as reported by the package manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InstalledApp {
    /// Stable key uniquely identifying this application in a snapshot.
    /// For dpkg this is the package name; for rpm the NEVRA string.
    pub application_key: String,
    /// Human-readable display name.
    pub name: String,
    /// Version string (may be absent for metapackages).
    pub version: Option<String>,
    /// Publisher / maintainer string.
    pub publisher: Option<String>,
}

/// Enumerates installed applications and supports silent uninstall.
pub trait InstalledApplicationsProvider: Send + Sync {
    /// `true` on platforms where enumeration is supported.
    fn is_supported(&self) -> bool;

    /// Enumerate installed applications. Implementations must not panic;
    /// errors are surfaced via `PlatformError`.
    fn list(&self) -> Result<Vec<InstalledApp>, PlatformError>;

    /// Uninstall the application identified by `application_key`.
    /// Never accepts a raw uninstall command from the wire — the
    /// implementation re-resolves the key locally.
    fn uninstall(&self, application_key: &str) -> Result<i32, PlatformError>;
}
