// Source: CMRemote, clean-room implementation.

//! Handler registry for hub invocations (slices R3–R6).
//!
//! [`AgentHandlers`] is the concrete struct holding all per-OS
//! provider implementations and dispatching to them by
//! [`MethodName`].

pub mod apps;
pub mod device_info;
pub mod packages;
pub mod script;

use std::sync::Arc;

use cmremote_wire::HubInvocation;

use cmremote_platform::{
    apps::InstalledApplicationsProvider, packages::PackageProviderHandler, DeviceInfoProvider,
};

use crate::dispatch::MethodName;
use cmremote_wire::ConnectionInfo;

/// Owns the per-OS providers and dispatches hub invocations.
pub struct AgentHandlers {
    pub(crate) connection_info: ConnectionInfo,
    pub(crate) device_info: Arc<dyn DeviceInfoProvider>,
    pub(crate) apps: Arc<dyn InstalledApplicationsProvider>,
    pub(crate) packages: Arc<dyn PackageProviderHandler>,
}

impl AgentHandlers {
    /// Dispatch a hub invocation to the appropriate handler.
    pub async fn dispatch(
        &self,
        method: MethodName,
        inv: &HubInvocation,
    ) -> Result<serde_json::Value, String> {
        match method {
            MethodName::TriggerHeartbeat => {
                device_info::handle_trigger_heartbeat(
                    &self.connection_info,
                    self.device_info.clone(),
                )
                .await
            }
            MethodName::ExecuteCommand => script::handle_execute_command(inv).await,
            MethodName::RequestInstalledApplications => {
                apps::handle_request_installed_applications(&*self.apps)
            }
            MethodName::UninstallApplication => {
                apps::handle_uninstall_application(inv, &*self.apps)
            }
            MethodName::InstallPackage => {
                packages::handle_install_package(inv, &*self.packages).await
            }
            // R7–R8 stubs
            _ => Err("not_implemented".to_string()),
        }
    }
}
