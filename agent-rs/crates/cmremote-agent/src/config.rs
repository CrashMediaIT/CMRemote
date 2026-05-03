// Source: CMRemote, clean-room implementation.

//! Configuration loading.
//!
//! The Rust agent reads the same `ConnectionInfo.json` file format the
//! legacy .NET agent uses so that an in-place upgrade does not require
//! operators to rewrite any config. CLI overrides take precedence over
//! file values, matching legacy behaviour.

use std::path::{Path, PathBuf};

use cmremote_wire::ConnectionInfo;

use crate::cli::CliArgs;

/// Errors returned by the configuration loader.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("could not read config at {path}: {source}")]
    Read {
        /// Path the loader tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration file could not be written.
    #[error("could not write config at {path}: {source}")]
    Write {
        /// Path the writer tried to write.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration could not be serialised.
    #[error("could not serialize config for {path}: {source}")]
    Serialize {
        /// Path that failed to serialize.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// The configuration file was syntactically invalid.
    #[error("could not parse config at {path}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// The merged configuration failed validation.
    #[error("invalid configuration: {0}")]
    Invalid(#[from] cmremote_wire::WireError),
}

/// Default file name relative to the agent's working directory.
pub const DEFAULT_CONFIG_FILENAME: &str = "ConnectionInfo.json";

/// Resolve the on-disk config path: CLI override wins, otherwise
/// `<cwd>/ConnectionInfo.json`.
pub fn resolve_config_path(cli: &CliArgs) -> PathBuf {
    cli.config_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILENAME))
}

/// Load `ConnectionInfo` from disk if the file exists, otherwise return
/// a fresh default. Missing files are *not* an error — the agent may
/// be launched on a brand-new host with all values supplied via the
/// CLI.
pub fn load_or_default(path: &Path) -> Result<ConnectionInfo, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConnectionInfo::new()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Persist `ConnectionInfo.json` with secret-safe permissions.
///
/// On Unix the final file is forced to `0600` so only the owning agent
/// account (and root) can read the device verification tokens. Windows
/// builds create/replace the file through the same atomic path; ACLs are
/// pinned by the Windows-only unit test until the enrolment writer moves
/// into the Windows service installer.
pub fn save_secure(path: &Path, info: &ConnectionInfo) -> Result<(), ConfigError> {
    let json = serde_json::to_vec_pretty(info).map_err(|source| ConfigError::Serialize {
        path: path.to_path_buf(),
        source,
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let tmp = path.with_extension("json.tmp");
    write_secure_file(&tmp, &json)?;
    std::fs::rename(&tmp, path).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    set_secure_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn write_secure_file(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn write_secure_file(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    std::fs::write(path, bytes).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn set_secure_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt as _;
    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_secure_permissions(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}

/// Apply CLI overrides on top of a previously loaded `ConnectionInfo`.
pub fn apply_overrides(mut info: ConnectionInfo, cli: &CliArgs) -> ConnectionInfo {
    if let Some(host) = cli.host.as_deref() {
        info.host = Some(host.to_owned());
    }
    if let Some(org) = cli.organization.as_deref() {
        info.organization_id = Some(org.to_owned());
    }
    if let Some(dev) = cli.device.as_deref() {
        info.device_id = dev.to_owned();
    }
    info
}

/// Convenience: load + override + validate in one call.
pub fn build(cli: &CliArgs) -> Result<ConnectionInfo, ConfigError> {
    let path = resolve_config_path(cli);
    let info = apply_overrides(load_or_default(&path)?, cli);
    info.validate()?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn load_or_default_returns_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let info = load_or_default(&path).unwrap();
        assert!(!info.device_id.is_empty());
    }

    #[test]
    fn load_or_default_parses_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ConnectionInfo.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            r#"{{ "DeviceID": "abc", "Host": "https://example.com", "OrganizationID": "org" }}"#
        )
        .unwrap();
        let info = load_or_default(&path).unwrap();
        assert_eq!(info.device_id, "abc");
        assert_eq!(info.organization_id.as_deref(), Some("org"));
    }

    #[test]
    fn save_secure_round_trips_connection_info() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ConnectionInfo.json");
        let info = ConnectionInfo {
            device_id: "device-1".into(),
            host: Some("https://cmremote.example".into()),
            organization_id: Some("org-1".into()),
            server_verification_token: Some("server-secret".into()),
            organization_token: Some("org-secret".into()),
        };

        save_secure(&path, &info).unwrap();

        let loaded = load_or_default(&path).unwrap();
        assert_eq!(loaded.device_id, info.device_id);
        assert_eq!(loaded.host, info.host);
        assert_eq!(loaded.organization_id, info.organization_id);
        assert_eq!(
            loaded.server_verification_token,
            info.server_verification_token
        );
        assert_eq!(loaded.organization_token, info.organization_token);
    }

    #[cfg(unix)]
    #[test]
    fn save_secure_writes_connection_info_with_0600_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ConnectionInfo.json");
        let info = ConnectionInfo {
            device_id: "device-1".into(),
            host: Some("https://cmremote.example".into()),
            organization_id: Some("org-1".into()),
            server_verification_token: Some("server-secret".into()),
            organization_token: Some("org-secret".into()),
        };

        save_secure(&path, &info).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn parse_error_wraps_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json").unwrap();
        let err = load_or_default(&path).unwrap_err();
        match err {
            ConfigError::Parse { path: p, .. } => assert_eq!(p, path),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn cli_overrides_override_file_values() {
        let info = ConnectionInfo {
            device_id: "from-file".into(),
            host: Some("https://from-file".into()),
            organization_id: Some("file-org".into()),
            server_verification_token: None,
            organization_token: None,
        };
        let cli = CliArgs {
            host: Some("https://from-cli".into()),
            organization: Some("cli-org".into()),
            device: Some("from-cli".into()),
            ..CliArgs::default()
        };
        let merged = apply_overrides(info, &cli);
        assert_eq!(merged.host.as_deref(), Some("https://from-cli"));
        assert_eq!(merged.organization_id.as_deref(), Some("cli-org"));
        assert_eq!(merged.device_id, "from-cli");
    }

    #[test]
    fn build_validates_merged_config() {
        // Empty CLI + missing file → default `ConnectionInfo` with no
        // host/org → must fail validation.
        let cli = CliArgs {
            config_path: Some("/nonexistent/path-for-cmremote-test.json".into()),
            ..CliArgs::default()
        };
        let err = build(&cli).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }
}
