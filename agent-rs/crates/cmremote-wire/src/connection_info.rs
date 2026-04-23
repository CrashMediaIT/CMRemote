// Source: CMRemote, clean-room implementation.

//! Bootstrap connection configuration for the agent.
//!
//! Mirrors the on-disk `ConnectionInfo.json` consumed by the legacy .NET
//! agent so that an upgrade-in-place from the .NET agent to the Rust
//! agent does not require operators to rewrite their config.

use serde::{Deserialize, Serialize};

use crate::error::WireError;

/// Persistent bootstrap configuration written to `ConnectionInfo.json`.
///
/// Field names match the legacy on-disk format intentionally (note the
/// all-caps `ID` suffix) — see the "Bootstrap configuration" section of
/// `docs/wire-protocol.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Stable per-device identifier. Generated on first run if absent.
    #[serde(rename = "DeviceID", default = "ConnectionInfo::new_device_id")]
    pub device_id: String,

    /// Base URL of the CMRemote server, e.g. `https://cmremote.example.com`.
    /// Trailing slashes are stripped on load.
    #[serde(rename = "Host", default)]
    pub host: Option<String>,

    /// Tenant the device belongs to.
    #[serde(rename = "OrganizationID", default)]
    pub organization_id: Option<String>,

    /// Server-issued token used to verify the server identity on
    /// reconnect. Optional on first run.
    #[serde(rename = "ServerVerificationToken", default)]
    pub server_verification_token: Option<String>,
}

impl ConnectionInfo {
    /// Build a fresh `ConnectionInfo` with a generated device id.
    pub fn new() -> Self {
        Self {
            device_id: Self::new_device_id(),
            host: None,
            organization_id: None,
            server_verification_token: None,
        }
    }

    /// Returns `host` with any trailing `/` stripped, matching the
    /// legacy .NET agent's normalization rules.
    pub fn normalized_host(&self) -> Option<String> {
        self.host
            .as_deref()
            .map(str::trim)
            .map(|s| s.trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty())
    }

    /// Validate the minimum set of fields required to attempt a
    /// connection.
    pub fn validate(&self) -> Result<(), WireError> {
        if self.device_id.trim().is_empty() {
            return Err(WireError::InvalidConfig("DeviceID is empty"));
        }
        if self.normalized_host().is_none() {
            return Err(WireError::InvalidConfig("Host is missing"));
        }
        if self
            .organization_id
            .as_deref()
            .map(str::trim)
            .map(str::is_empty)
            .unwrap_or(true)
        {
            return Err(WireError::InvalidConfig("OrganizationID is missing"));
        }
        Ok(())
    }

    fn new_device_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_generates_device_id() {
        let info = ConnectionInfo::new();
        assert!(!info.device_id.is_empty());
        // Must parse as a UUID.
        uuid::Uuid::parse_str(&info.device_id).expect("device id must be a UUID");
    }

    #[test]
    fn normalized_host_strips_trailing_slash_and_whitespace() {
        let info = ConnectionInfo {
            device_id: "d".into(),
            host: Some("  https://example.com/  ".into()),
            organization_id: Some("o".into()),
            server_verification_token: None,
        };
        assert_eq!(
            info.normalized_host().as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn validate_requires_host_and_org() {
        let mut info = ConnectionInfo::new();
        assert!(info.validate().is_err());
        info.host = Some("https://example.com".into());
        assert!(info.validate().is_err());
        info.organization_id = Some("org".into());
        info.validate().expect("should be valid");
    }

    #[test]
    fn validate_rejects_blank_device_id() {
        let info = ConnectionInfo {
            device_id: "   ".into(),
            host: Some("https://example.com".into()),
            organization_id: Some("org".into()),
            server_verification_token: None,
        };
        assert!(matches!(info.validate(), Err(WireError::InvalidConfig(_))));
    }

    #[test]
    fn round_trips_pascal_case_json() {
        // The on-disk format used by the legacy .NET agent is PascalCase.
        let json = r#"{
            "DeviceID": "f2b0a595-5ea8-471b-975f-12e70e0f3497",
            "Host": "https://cmremote.example.com",
            "OrganizationID": "org-1",
            "ServerVerificationToken": "tok"
        }"#;
        let info: ConnectionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.device_id, "f2b0a595-5ea8-471b-975f-12e70e0f3497");
        assert_eq!(info.organization_id.as_deref(), Some("org-1"));
        let re = serde_json::to_string(&info).unwrap();
        assert!(re.contains("\"DeviceID\""));
        assert!(re.contains("\"OrganizationID\""));
    }
}
