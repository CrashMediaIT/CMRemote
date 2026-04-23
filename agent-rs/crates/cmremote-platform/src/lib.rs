// Source: CMRemote, clean-room implementation.
//
// `cmremote-platform` defines the per-OS provider traits used by the
// agent. The R0 scaffold only declares the trait surface and a generic
// host descriptor; per-OS implementations land slice-by-slice (see
// ROADMAP.md → "Rust agent slice-by-slice delivery plan").

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

//! Operating-system abstractions for the CMRemote agent.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Identifies the operating-system family the agent is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostOs {
    /// Microsoft Windows (any supported edition).
    Windows,
    /// Any GNU/Linux distribution.
    Linux,
    /// Apple macOS.
    MacOs,
    /// Any other Unix-like system; treated as best-effort.
    OtherUnix,
}

impl HostOs {
    /// Detect the current host's OS family at compile time.
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::OtherUnix
        }
    }

    /// Short stable string used in logs and the wire protocol.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::OtherUnix => "unix",
        }
    }
}

/// Errors produced by platform providers.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// The requested capability is not supported on this OS.
    #[error("not supported on {0:?}")]
    NotSupported(HostOs),

    /// An I/O error occurred while talking to the OS.
    #[error("platform I/O error: {0}")]
    Io(String),
}

/// Reports static information about the host (hostname, OS version,
/// architecture, …). Implementations land in slice R3.
pub trait DeviceInfoProvider: Send + Sync {
    /// Return a snapshot of the current host description.
    fn snapshot(&self) -> Result<HostDescriptor, PlatformError>;
}

/// Static host description reported back to the server.
///
/// Field set is deliberately small for R0; slice R3 expands it to match
/// the .NET agent's `Device` DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDescriptor {
    /// Operating-system family.
    pub os: HostOs,
    /// Free-form OS description (e.g. `"Windows 11 23H2"`).
    pub os_description: String,
    /// Reported hostname.
    pub hostname: String,
    /// CPU architecture (`x86_64`, `aarch64`, …).
    pub architecture: String,
}

impl HostDescriptor {
    /// Construct a `HostDescriptor` from the values the Rust standard
    /// library exposes without any OS-specific dependencies. Used by
    /// the R0 stub provider and by tests.
    pub fn from_std() -> Self {
        Self {
            os: HostOs::current(),
            os_description: std::env::consts::OS.to_string(),
            hostname: std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "unknown".into()),
            architecture: std::env::consts::ARCH.to_string(),
        }
    }
}

/// R0 stub provider that returns the standard-library snapshot.
/// Replaced per-OS in slice R3.
#[derive(Debug, Default)]
pub struct StdDeviceInfoProvider;

impl DeviceInfoProvider for StdDeviceInfoProvider {
    fn snapshot(&self) -> Result<HostDescriptor, PlatformError> {
        Ok(HostDescriptor::from_std())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_os_current_matches_cfg() {
        let os = HostOs::current();
        if cfg!(target_os = "linux") {
            assert_eq!(os, HostOs::Linux);
        }
        assert!(!os.as_str().is_empty());
    }

    #[test]
    fn std_provider_returns_some_arch() {
        let snap = StdDeviceInfoProvider.snapshot().unwrap();
        assert!(!snap.architecture.is_empty());
        assert!(!snap.os_description.is_empty());
    }
}
