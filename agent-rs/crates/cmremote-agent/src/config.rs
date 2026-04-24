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
