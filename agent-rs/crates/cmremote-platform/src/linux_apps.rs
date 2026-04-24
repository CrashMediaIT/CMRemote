// Source: CMRemote, clean-room implementation.

//! Linux installed-applications provider (slice R5).
//!
//! Enumerates packages via `dpkg-query` on Debian/Ubuntu systems, with
//! an automatic fallback to `rpm -qa` on RPM-based distros.

use std::process::Command;

use crate::apps::{InstalledApp, InstalledApplicationsProvider};
use crate::PlatformError;

/// Enumerates and uninstalls packages on Linux using either dpkg or rpm.
pub struct DpkgProvider;

impl InstalledApplicationsProvider for DpkgProvider {
    fn is_supported(&self) -> bool {
        true
    }

    fn list(&self) -> Result<Vec<InstalledApp>, PlatformError> {
        if dpkg_available() {
            list_dpkg()
        } else if rpm_available() {
            list_rpm()
        } else {
            Err(PlatformError::Io(
                "neither dpkg nor rpm found on this system".into(),
            ))
        }
    }

    fn uninstall(&self, application_key: &str) -> Result<i32, PlatformError> {
        // Never accept a raw command from the wire; resolve only by key.
        if application_key.is_empty() || application_key.contains('\n') {
            return Err(PlatformError::Io("invalid application key".into()));
        }
        // Reject keys that try to look like additional options to the
        // package manager. `apt-get`, `rpm`, and most coreutils treat
        // anything starting with `-` as a flag even when it appears in
        // the trailing positional position; the leading `--` separator
        // we add below stops that, but a defence-in-depth check here
        // also rejects obviously-malicious payloads early.
        if application_key.starts_with('-') {
            return Err(PlatformError::Io(
                "application key may not start with '-'".into(),
            ));
        }

        // The leading `--` terminates option parsing for both apt-get
        // and rpm, so a key that slips a `-` past the check above (for
        // example via locale-specific characters) still cannot be
        // interpreted as a flag.
        let status = if dpkg_available() {
            Command::new("apt-get")
                .args(["remove", "-y", "--", application_key])
                .status()
        } else {
            Command::new("rpm")
                .args(["-e", "--", application_key])
                .status()
        };

        match status {
            Ok(s) => Ok(s.code().unwrap_or(-1)),
            Err(e) => Err(PlatformError::Io(e.to_string())),
        }
    }
}

fn dpkg_available() -> bool {
    Command::new("dpkg-query").arg("--version").output().is_ok()
}

fn rpm_available() -> bool {
    Command::new("rpm").arg("--version").output().is_ok()
}

fn list_dpkg() -> Result<Vec<InstalledApp>, PlatformError> {
    let output = Command::new("dpkg-query")
        .args([
            "--show",
            "--showformat=${Package}\t${Version}\t${Maintainer}\n",
        ])
        .output()
        .map_err(|e| PlatformError::Io(e.to_string()))?;

    if !output.status.success() {
        return Err(PlatformError::Io("dpkg-query failed".into()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_tsv_apps(&text))
}

fn list_rpm() -> Result<Vec<InstalledApp>, PlatformError> {
    let output = Command::new("rpm")
        .args(["-qa", "--queryformat", "%{NAME}\t%{VERSION}\t%{VENDOR}\n"])
        .output()
        .map_err(|e| PlatformError::Io(e.to_string()))?;

    if !output.status.success() {
        return Err(PlatformError::Io("rpm query failed".into()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_tsv_apps(&text))
}

/// Parse `name\tversion\tpublisher` lines.
pub(crate) fn parse_tsv_apps(text: &str) -> Vec<InstalledApp> {
    text.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let mut cols = line.splitn(3, '\t');
            let name = cols.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let version = cols.next().map(str::trim).filter(|v| !v.is_empty());
            let publisher = cols.next().map(str::trim).filter(|v| !v.is_empty());
            Some(InstalledApp {
                application_key: name.to_string(),
                name: name.to_string(),
                version: version.map(String::from),
                publisher: publisher.map(String::from),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dpkg_output() {
        let tsv = "bash\t5.1-6\tDebian\nzlib1g\t1:1.2.11\tDebian\n";
        let apps = parse_tsv_apps(tsv);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "bash");
        assert_eq!(apps[0].version.as_deref(), Some("5.1-6"));
        assert_eq!(apps[0].publisher.as_deref(), Some("Debian"));
        assert_eq!(apps[1].application_key, "zlib1g");
    }

    #[test]
    fn skips_empty_lines() {
        let tsv = "bash\t5.1\tDebian\n\ncurl\t7.68\tDebian\n";
        let apps = parse_tsv_apps(tsv);
        assert_eq!(apps.len(), 2);
    }

    #[test]
    fn uninstall_rejects_option_like_key() {
        let provider = DpkgProvider;
        // Any key starting with '-' must be rejected before we shell
        // out, regardless of whether dpkg/rpm exist on the host.
        let err = provider
            .uninstall("--reinstall")
            .expect_err("should reject option-like key");
        let msg = err.to_string();
        assert!(msg.contains("'-'"), "unexpected error: {msg}");
    }

    #[test]
    fn uninstall_rejects_empty_or_multiline_key() {
        let provider = DpkgProvider;
        assert!(provider.uninstall("").is_err());
        assert!(provider.uninstall("foo\nrm -rf /").is_err());
    }
}
