// Source: CMRemote, clean-room implementation.

//! Installed-applications hub handlers (slice R5).

use cmremote_platform::apps::InstalledApplicationsProvider;
use cmremote_wire::HubInvocation;

/// Handle `RequestInstalledApplications`: list all installed apps and
/// return them as a JSON array.
pub fn handle_request_installed_applications(
    provider: &dyn InstalledApplicationsProvider,
) -> Result<serde_json::Value, String> {
    let apps = provider.list().map_err(|e| e.to_string())?;
    serde_json::to_value(apps).map_err(|e| e.to_string())
}

/// Handle `UninstallApplication`: extract the `applicationKey` from the
/// first argument and uninstall by key (never by raw command string).
pub fn handle_uninstall_application(
    inv: &HubInvocation,
    provider: &dyn InstalledApplicationsProvider,
) -> Result<serde_json::Value, String> {
    let key = inv
        .arguments
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| "invalid_arguments: expected applicationKey string".to_string())?;

    let exit_code = provider.uninstall(key).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ExitCode": exit_code }))
}
