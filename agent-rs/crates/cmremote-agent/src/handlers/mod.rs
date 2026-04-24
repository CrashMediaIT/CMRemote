// Source: CMRemote, clean-room implementation.

//! Handler registry for hub invocations (slices R3–R6).
//!
//! [`AgentHandlers`] is the concrete struct holding all per-OS
//! provider implementations and dispatching to them by
//! [`MethodName`].

pub mod agent_update;
pub mod apps;
pub mod desktop;
pub mod device_info;
pub mod packages;
pub mod script;

use std::sync::Arc;

use cmremote_wire::HubInvocation;

use cmremote_platform::{
    apps::InstalledApplicationsProvider, desktop::DesktopTransportProvider,
    packages::PackageProviderHandler, DeviceInfoProvider,
};

use crate::dispatch::MethodName;
use cmremote_wire::ConnectionInfo;

/// Owns the per-OS providers and dispatches hub invocations.
pub struct AgentHandlers {
    pub(crate) connection_info: ConnectionInfo,
    pub(crate) device_info: Arc<dyn DeviceInfoProvider>,
    pub(crate) apps: Arc<dyn InstalledApplicationsProvider>,
    pub(crate) packages: Arc<dyn PackageProviderHandler>,
    pub(crate) agent_update: Arc<agent_update::AgentUpdateContext>,
    pub(crate) desktop: Arc<dyn DesktopTransportProvider>,
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
            MethodName::InstallAgentUpdate => {
                agent_update::handle_install_agent_update(inv, &self.agent_update).await
            }
            // Slice R7 — desktop transport. The default provider
            // (`NotSupportedDesktopTransport`) returns a structured
            // failure so the operator sees a clear "not supported on
            // <OS>" message instead of `not_implemented`. Concrete
            // WebRTC-backed drivers register here without any further
            // dispatch-layer changes.
            MethodName::RemoteControl => desktop::handle_remote_control(inv, &*self.desktop).await,
            MethodName::RestartScreenCaster => {
                desktop::handle_restart_screen_caster(inv, &*self.desktop).await
            }
            MethodName::ChangeWindowsSession => {
                desktop::handle_change_windows_session(inv, &*self.desktop).await
            }
            MethodName::InvokeCtrlAltDel => {
                desktop::handle_invoke_ctrl_alt_del(inv, &*self.desktop).await
            }
            // Slice R7.g — desktop signalling. Same dispatch shape as
            // the four method-surface methods above; until the
            // crypto-provider ADR is decided and a real WebRTC
            // driver lands, every call resolves to a structured
            // "not supported on <OS>" failure.
            MethodName::SendSdpOffer => desktop::handle_send_sdp_offer(inv, &*self.desktop).await,
            MethodName::SendSdpAnswer => desktop::handle_send_sdp_answer(inv, &*self.desktop).await,
            MethodName::SendIceCandidate => {
                desktop::handle_send_ice_candidate(inv, &*self.desktop).await
            }
            // Slice R7.j — per-session ICE / TURN configuration.
            // Same dispatch shape as the signalling family; the
            // stub provider runs the slice R7.b envelope guards
            // plus the slice R7.i config guards before returning a
            // structured "not supported on <OS>" failure.
            MethodName::ProvideIceServers => {
                desktop::handle_provide_ice_servers(inv, &*self.desktop).await
            }
            // R8 stubs (RunScript / DeleteLogs / GetLogs /
            // ReinstallAgent / UninstallAgent / WakeDevice /
            // TransferFileFromBrowserToAgent) — fall through to the
            // generic `not_implemented` completion until their
            // respective slices land.
            _ => Err("not_implemented".to_string()),
        }
    }
}
