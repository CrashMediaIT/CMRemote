// Source: CMRemote, clean-room implementation.

//! Linux-specific device-information provider (slice R3).
//!
//! All reads are best-effort: any individual syscall / procfs failure
//! falls back to a zero or empty value rather than propagating. The
//! goal is to never panic and always return *something* useful.

use std::fs;

use crate::{DeviceInfoProvider, DeviceSnapshot, DriveInfo, HostOs, PlatformError};

/// Reads device metadata from Linux-specific procfs / sysfs paths.
pub struct LinuxDeviceInfoProvider;

impl DeviceInfoProvider for LinuxDeviceInfoProvider {
    fn snapshot(&self) -> Result<DeviceSnapshot, PlatformError> {
        Ok(DeviceSnapshot {
            device_id: String::new(),
            organization_id: String::new(),
            hostname: read_hostname(),
            os: HostOs::Linux,
            os_description: read_os_description(),
            architecture: std::env::consts::ARCH.to_string(),
            processor_count: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            is_64bit: cfg!(target_pointer_width = "64"),
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            current_user: read_current_user(),
            drives: read_drives(),
            total_memory_gb: memory_total_gb(),
            used_memory_gb: memory_used_gb(),
            cpu_utilization: cpu_utilization_percent(),
            mac_addresses: read_mac_addresses(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_hostname() -> String {
    // Try /etc/hostname first (most reliable), then $HOSTNAME env var.
    fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".into())
}

fn read_os_description() -> String {
    // Parse /etc/os-release for PRETTY_NAME.
    if let Ok(content) = fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
                return rest.trim_matches('"').to_string();
            }
        }
    }
    // Fall back to the generic OS string.
    std::env::consts::OS.to_string()
}

fn read_current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

/// Parse `/proc/meminfo` to extract total and available RAM in kB.
fn parse_meminfo() -> (u64, u64) {
    let content = match fs::read_to_string("/proc/meminfo") {
        Ok(c) => c,
        Err(_) => return (0, 0),
    };
    let mut total_kb = 0u64;
    let mut avail_kb = 0u64;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = parse_kb_value(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = parse_kb_value(rest);
        }
    }
    (total_kb, avail_kb)
}

fn parse_kb_value(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn memory_total_gb() -> f64 {
    let (total_kb, _) = parse_meminfo();
    round2(total_kb as f64 / 1024.0 / 1024.0)
}

fn memory_used_gb() -> f64 {
    let (total_kb, avail_kb) = parse_meminfo();
    let used_kb = total_kb.saturating_sub(avail_kb);
    round2(used_kb as f64 / 1024.0 / 1024.0)
}

/// Compute a 100 ms two-sample CPU utilisation from `/proc/stat`.
fn cpu_utilization_percent() -> f64 {
    fn read_idle_total() -> Option<(u64, u64)> {
        let line = fs::read_to_string("/proc/stat").ok()?;
        let cpu_line = line.lines().next()?; // "cpu  user nice system idle ..."
        let fields: Vec<u64> = cpu_line
            .split_whitespace()
            .skip(1) // skip "cpu"
            .filter_map(|v| v.parse().ok())
            .collect();
        if fields.len() < 4 {
            return None;
        }
        let idle = fields[3];
        let total: u64 = fields.iter().sum();
        Some((idle, total))
    }

    let Some((idle1, total1)) = read_idle_total() else {
        return 0.0;
    };
    std::thread::sleep(std::time::Duration::from_millis(100));
    let Some((idle2, total2)) = read_idle_total() else {
        return 0.0;
    };
    let d_total = total2.saturating_sub(total1) as f64;
    if d_total == 0.0 {
        return 0.0;
    }
    let d_idle = idle2.saturating_sub(idle1) as f64;
    round2((1.0 - d_idle / d_total) * 100.0)
}

/// Read drive info from `df -k --output=target,size,avail`.
fn read_drives() -> Vec<DriveInfo> {
    let output = match std::process::Command::new("df")
        .args(["-k", "--output=target,size,avail"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut drives = Vec::new();

    for line in text.lines().skip(1) {
        // line: "target  size  avail"
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }
        let name = cols[0].to_string();
        let total_kb: u64 = cols[1].parse().unwrap_or(0);
        let free_kb: u64 = cols[2].parse().unwrap_or(0);
        drives.push(DriveInfo {
            name,
            total_gb: round2(total_kb as f64 / 1024.0 / 1024.0),
            free_gb: round2(free_kb as f64 / 1024.0 / 1024.0),
        });
    }
    drives
}

/// Read MAC addresses from `/sys/class/net/<iface>/address`, skipping
/// loopback (`lo`) and virtual interfaces.
fn read_mac_addresses() -> Vec<String> {
    let entries = match fs::read_dir("/sys/class/net") {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut macs = Vec::new();
    for entry in entries.flatten() {
        let iface = entry.file_name();
        let iface_str = iface.to_string_lossy();
        if iface_str == "lo" {
            continue;
        }
        let addr_path = entry.path().join("address");
        if let Ok(mac) = fs::read_to_string(&addr_path) {
            let mac = mac.trim().to_string();
            // Skip all-zeros or empty MACs (virtual interfaces).
            if !mac.is_empty() && mac != "00:00:00:00:00:00" {
                macs.push(mac);
            }
        }
    }
    macs
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_provider_returns_non_empty_snapshot() {
        let snap = LinuxDeviceInfoProvider.snapshot().unwrap();
        assert_eq!(snap.os, HostOs::Linux);
        assert!(!snap.hostname.is_empty());
        assert!(!snap.architecture.is_empty());
        assert!(snap.processor_count > 0);
        assert!(!snap.agent_version.is_empty());
    }

    #[test]
    fn memory_values_are_non_negative() {
        let snap = LinuxDeviceInfoProvider.snapshot().unwrap();
        assert!(snap.total_memory_gb >= 0.0);
        assert!(snap.used_memory_gb >= 0.0);
        assert!(snap.used_memory_gb <= snap.total_memory_gb + 0.01);
    }

    #[test]
    fn hostname_is_non_empty() {
        assert!(!read_hostname().is_empty());
    }
}
