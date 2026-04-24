// Source: CMRemote, clean-room implementation.

//! Handler registry for hub invocations (slices R3–R5).
//!
//! [`AgentHandlers`] is the concrete struct holding all per-OS
//! provider implementations and dispatching to them by
//! [`MethodName`].

pub mod apps;
pub mod device_info;
pub mod script;

use std::sync::Arc;

use cmremote_wire::HubInvocation;

use cmremote_platform::{apps::InstalledApplicationsProvider, DeviceInfoProvider};

use crate::dispatch::MethodName;
use cmremote_wire::ConnectionInfo;

/// Owns the per-OS providers and dispatches hub invocations.
pub struct AgentHandlers {
    pub(crate) connection_info: ConnectionInfo,
    pub(crate) device_info: Arc<dyn DeviceInfoProvider>,
    pub(crate) apps: Arc<dyn InstalledApplicationsProvider>,
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
                device_info::handle_trigger_heartbeat(&self.connection_info, &*self.device_info)
            }
            MethodName::ExecuteCommand => script::handle_execute_command(inv).await,
            MethodName::RequestInstalledApplications => {
                apps::handle_request_installed_applications(&*self.apps)
            }
            MethodName::UninstallApplication => {
                apps::handle_uninstall_application(inv, &*self.apps)
            }
            // R6–R8 stubs
            _ => Err("not_implemented".to_string()),
        }
    }
}
