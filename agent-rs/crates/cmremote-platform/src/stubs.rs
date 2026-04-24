// Source: CMRemote, clean-room implementation.

//! Stub providers for platforms that don't yet have a native
//! implementation.

use crate::apps::{InstalledApp, InstalledApplicationsProvider};
use crate::{HostOs, PlatformError};

/// Returns `PlatformError::NotSupported` for every operation.
pub struct NotSupportedAppsProvider;

impl InstalledApplicationsProvider for NotSupportedAppsProvider {
    fn is_supported(&self) -> bool {
        false
    }

    fn list(&self) -> Result<Vec<InstalledApp>, PlatformError> {
        Err(PlatformError::NotSupported(HostOs::current()))
    }

    fn uninstall(&self, _application_key: &str) -> Result<i32, PlatformError> {
        Err(PlatformError::NotSupported(HostOs::current()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_supported_is_not_supported() {
        let p = NotSupportedAppsProvider;
        assert!(!p.is_supported());
        assert!(p.list().is_err());
        assert!(p.uninstall("bash").is_err());
    }
}
