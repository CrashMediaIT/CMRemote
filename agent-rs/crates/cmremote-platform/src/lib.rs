// Source: CMRemote, clean-room implementation.
//
// `cmremote-platform` defines the per-OS provider traits used by the
// agent. Slice R3 expands the device-info snapshot to the full
// `DeviceSnapshot` DTO matching the .NET `DeviceClientDto`, adds the
// `InstalledApplicationsProvider` trait (R5), and provides a
// `LinuxDeviceInfoProvider` backed by `/proc` and `/sys`.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

//! Operating-system abstractions for the CMRemote agent.

pub mod apps;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub mod linux_apps;
pub mod stubs;

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

// ---------------------------------------------------------------------------
// Device information — slice R3
// ---------------------------------------------------------------------------

/// A single filesystem / block-device reported in the device snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DriveInfo {
    /// Mount point or drive letter (e.g. `/` or `C:`).
    pub name: String,
    /// Total capacity in gibibytes.
    pub total_gb: f64,
    /// Free space in gibibytes.
    pub free_gb: f64,
}

/// Full device snapshot sent to the server via the `TriggerHeartbeat`
/// hub invocation. Mirrors `Remotely.Shared.Dtos.DeviceClientDto` field
/// for field.
///
/// Fields `device_id` and `organization_id` are filled in by the caller
/// (from `ConnectionInfo`) after the provider returns the snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceSnapshot {
    /// Device UUID — populated from `ConnectionInfo.DeviceID`.
    pub device_id: String,
    /// Org UUID — populated from `ConnectionInfo.OrganizationID`.
    pub organization_id: String,
    /// Machine / DNS hostname.
    pub hostname: String,
    /// OS family.
    pub os: HostOs,
    /// Detailed OS string (e.g. `"Linux 6.1.0 #1 SMP"`).
    pub os_description: String,
    /// CPU architecture string (`x86_64`, `aarch64`, …).
    pub architecture: String,
    /// Logical processor count.
    pub processor_count: usize,
    /// `true` when the process is 64-bit.
    pub is_64bit: bool,
    /// Agent build version from `CARGO_PKG_VERSION`.
    pub agent_version: String,
    /// Logged-in user running the agent process.
    pub current_user: String,
    /// Mounted drives / volumes.
    pub drives: Vec<DriveInfo>,
    /// Total physical memory in GB.
    pub total_memory_gb: f64,
    /// Used physical memory in GB.
    pub used_memory_gb: f64,
    /// Instantaneous CPU utilisation 0–100.
    pub cpu_utilization: f64,
    /// MAC addresses of active network interfaces.
    pub mac_addresses: Vec<String>,
}

impl Default for HostOs {
    fn default() -> Self {
        HostOs::current()
    }
}

/// Reports static + dynamic information about the host.
pub trait DeviceInfoProvider: Send + Sync {
    /// Return a snapshot of the current host. The `device_id` and
    /// `organization_id` fields default to empty strings; the caller
    /// must fill them from `ConnectionInfo` before sending.
    fn snapshot(&self) -> Result<DeviceSnapshot, PlatformError>;
}

/// Backward-compat alias used by R0-era code.
pub type HostDescriptor = DeviceSnapshot;

// ---------------------------------------------------------------------------
// Std / stub provider
// ---------------------------------------------------------------------------

/// Provider that reads everything available through the Rust standard
/// library. On Linux this delegates to `LinuxDeviceInfoProvider` for
/// the richer `/proc` data.
#[derive(Debug, Default)]
pub struct StdDeviceInfoProvider;

impl DeviceInfoProvider for StdDeviceInfoProvider {
    fn snapshot(&self) -> Result<DeviceSnapshot, PlatformError> {
        #[cfg(target_os = "linux")]
        {
            linux::LinuxDeviceInfoProvider.snapshot()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(DeviceSnapshot {
                os: HostOs::current(),
                os_description: std::env::consts::OS.to_string(),
                hostname: std::env::var("HOSTNAME")
                    .or_else(|_| std::env::var("COMPUTERNAME"))
                    .unwrap_or_else(|_| "unknown".into()),
                architecture: std::env::consts::ARCH.to_string(),
                processor_count: std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1),
                is_64bit: cfg!(target_pointer_width = "64"),
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                ..DeviceSnapshot::default()
            })
        }
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
        assert!(snap.processor_count > 0);
    }
}
