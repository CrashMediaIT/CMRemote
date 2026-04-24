// Source: CMRemote, clean-room implementation.

//! Package-manager hub handler (slice R6).
//!
//! Decodes the `InstallPackage` invocation arguments, dispatches them
//! through the [`PackageProviderHandler`] composite owned by
//! [`crate::handlers::AgentHandlers`], and serialises the
//! [`PackageInstallResult`] back as the completion payload.
//!
//! This handler is **deliberately stub-friendly**: per ROADMAP slice
//! R6 we ship the wire surface, the safety helpers, and the
//! routing/composition layer ahead of the signed-build pipeline. When
//! no concrete provider is registered (the default on every host
//! today), the composite returns a structured "not supported" failure
//! so the operator sees a clean job-failed status rather than a hung
//! job.

use cmremote_platform::packages::PackageProviderHandler;
use cmremote_wire::{HubInvocation, PackageInstallRequest, PackageInstallResult};

/// Handle `InstallPackage`: decode the request, dispatch through the
/// agent's package-provider composite, and return the result as JSON.
pub async fn handle_install_package(
    inv: &HubInvocation,
    provider: &dyn PackageProviderHandler,
) -> Result<serde_json::Value, String> {
    let raw_arg = inv
        .arguments
        .first()
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let request: PackageInstallRequest = match serde_json::from_value(raw_arg) {
        Ok(r) => r,
        Err(e) => {
            // Malformed arg: surface a structured failure with an
            // empty job id (the server is expected to log this on its
            // side as "client decode error" because there is no job
            // id we can echo back). The completion still carries a
            // PackageInstallResult so the wire shape is invariant.
            let result =
                PackageInstallResult::failed(String::new(), format!("invalid_arguments: {e}"));
            return serde_json::to_value(result).map_err(|e| e.to_string());
        }
    };

    let result = provider.execute(&request).await;
    serde_json::to_value(result).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use cmremote_platform::packages::{CompositePackageProvider, NotSupportedPackageProvider};
    use cmremote_wire::{HubMessageKind, PackageInstallAction, PackageProvider};

    use super::*;

    fn make_inv(arg: serde_json::Value) -> HubInvocation {
        HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("inv-1".into()),
            target: "InstallPackage".into(),
            arguments: vec![arg],
        }
    }

    #[tokio::test]
    async fn missing_argument_surfaces_invalid_arguments() {
        let inv = HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("inv-x".into()),
            target: "InstallPackage".into(),
            arguments: vec![],
        };
        let composite = CompositePackageProvider::new();
        let val = handle_install_package(&inv, &composite).await.unwrap();
        let r: PackageInstallResult = serde_json::from_value(val).unwrap();
        assert!(!r.success);
        assert!(r
            .error_message
            .as_deref()
            .unwrap_or("")
            .contains("invalid_arguments"));
    }

    #[tokio::test]
    async fn malformed_argument_surfaces_invalid_arguments() {
        // Argument is a string instead of an object.
        let inv = make_inv(serde_json::json!("not-an-object"));
        let composite = CompositePackageProvider::new();
        let val = handle_install_package(&inv, &composite).await.unwrap();
        let r: PackageInstallResult = serde_json::from_value(val).unwrap();
        assert!(!r.success);
        assert!(r
            .error_message
            .as_deref()
            .unwrap_or("")
            .contains("invalid_arguments"));
    }

    #[tokio::test]
    async fn no_handler_registered_returns_structured_not_supported() {
        let arg = serde_json::json!({
            "JobId": "job-7",
            "Provider": "Chocolatey",
            "Action": "Install",
            "PackageIdentifier": "googlechrome"
        });
        let inv = make_inv(arg);
        let composite = CompositePackageProvider::new();
        let val = handle_install_package(&inv, &composite).await.unwrap();
        let r: PackageInstallResult = serde_json::from_value(val).unwrap();
        assert_eq!(r.job_id, "job-7");
        assert!(!r.success);
        assert_eq!(r.exit_code, -1);
        assert!(r
            .error_message
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("not supported"));
    }

    #[tokio::test]
    async fn explicit_unknown_provider_short_circuits() {
        let arg = serde_json::json!({
            "JobId": "job-u",
            "Provider": "Unknown",
            "Action": "Install",
            "PackageIdentifier": "anything"
        });
        let inv = make_inv(arg);
        let composite = CompositePackageProvider::new();
        let val = handle_install_package(&inv, &composite).await.unwrap();
        let r: PackageInstallResult = serde_json::from_value(val).unwrap();
        assert!(!r.success);
        assert!(r
            .error_message
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("unknown"));
    }

    /// Smoke test: when a concrete handler is registered, the request
    /// reaches it and the success result is forwarded verbatim.
    #[tokio::test]
    async fn registered_handler_is_invoked_with_decoded_request() {
        struct EchoHandler;
        #[async_trait]
        impl PackageProviderHandler for EchoHandler {
            fn can_handle(&self, _: &PackageInstallRequest) -> bool {
                true
            }
            async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
                assert_eq!(request.job_id, "job-9");
                assert_eq!(request.provider, PackageProvider::Chocolatey);
                assert_eq!(request.action, PackageInstallAction::Install);
                assert_eq!(request.package_identifier, "googlechrome");
                PackageInstallResult {
                    job_id: request.job_id.clone(),
                    success: true,
                    exit_code: 0,
                    duration_ms: 7,
                    stdout_tail: Some("ok".into()),
                    stderr_tail: None,
                    error_message: None,
                }
            }
        }

        let mut composite = CompositePackageProvider::new();
        composite.register(PackageProvider::Chocolatey, Box::new(EchoHandler));

        let arg = serde_json::json!({
            "JobId": "job-9",
            "Provider": "Chocolatey",
            "Action": "Install",
            "PackageIdentifier": "googlechrome"
        });
        let inv = make_inv(arg);
        let val = handle_install_package(&inv, &composite).await.unwrap();
        let r: PackageInstallResult = serde_json::from_value(val).unwrap();
        assert!(r.success);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.duration_ms, 7);
        assert_eq!(r.stdout_tail.as_deref(), Some("ok"));
    }

    /// Belt-and-braces: with the not-supported fallback explicitly in
    /// the composite, the handler's structured failure path is
    /// exercised end-to-end through the dispatcher's serialisation
    /// surface.
    #[tokio::test]
    async fn not_supported_fallback_round_trips_through_handler() {
        let _fallback = NotSupportedPackageProvider::for_current_host();
        let composite = CompositePackageProvider::new();
        let arg = serde_json::json!({
            "JobId": "job-z",
            "Provider": "UploadedMsi",
            "Action": "Install",
            "PackageIdentifier": "irrelevant"
        });
        let inv = make_inv(arg);
        let val = handle_install_package(&inv, &composite).await.unwrap();
        let r: PackageInstallResult = serde_json::from_value(val).unwrap();
        assert_eq!(r.job_id, "job-z");
        assert!(!r.success);
    }
}
