// Source: CMRemote, clean-room implementation.

//! Top-level async runtime for the agent.
//!
//! Slice R0 set up structured logging, configuration, and a shutdown
//! signal. Slice R2 layers the WebSocket connection / heartbeat /
//! reconnect loop on top. Slice R2a wires in the hub dispatch layer
//! (R3/R4/R5 handlers follow).

use std::sync::Arc;

#[cfg(target_os = "linux")]
use cmremote_platform::linux_apps::DpkgProvider;
use cmremote_platform::packages::{CompositePackageProvider, RejectingDownloader};
#[cfg(not(target_os = "linux"))]
use cmremote_platform::stubs::NotSupportedAppsProvider;
use cmremote_platform::{DeviceInfoProvider, StdDeviceInfoProvider};
use cmremote_wire::ConnectionInfo;
use tokio::sync::watch;
use tracing::info;

use crate::cli::CliArgs;
use crate::handlers::AgentHandlers;
use crate::transport::{self, TransportError};

/// Errors surfaced from the agent runtime.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Configuration could not be loaded or validated.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    /// Failure querying OS state.
    #[error(transparent)]
    Platform(#[from] cmremote_platform::PlatformError),

    /// Transport-layer failure that the reconnect loop could not
    /// recover from on its own.
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Run the agent until a shutdown signal is received.
///
/// Returns `Ok(())` on a clean shutdown.
pub async fn run(cli: CliArgs) -> Result<(), RuntimeError> {
    let info = crate::config::build(&cli)?;
    let host = StdDeviceInfoProvider.snapshot()?;

    log_startup_banner(&info, &host);

    let device_info = Arc::new(StdDeviceInfoProvider);

    #[cfg(target_os = "linux")]
    let apps = Arc::new(DpkgProvider);
    #[cfg(not(target_os = "linux"))]
    let apps = Arc::new(NotSupportedAppsProvider);

    // Slice R6: composite package provider with the per-OS default
    // handler set registered. On Windows the Chocolatey / UploadedMsi /
    // Executable providers are wired in; on every other OS each
    // provider's `can_handle` returns `false` so the composite still
    // surfaces a structured "not supported" failure (the providers
    // themselves return the same shape from `execute`). The downloader
    // is the rejecting stub for now — the runtime will swap in the
    // real reqwest-based client in a follow-up PR; until then MSI /
    // Executable jobs fail loudly with "this agent is not configured
    // to download package artifacts" rather than hanging.
    let cache_dir = std::env::temp_dir().join("cmremote-package-cache");
    let stage_dir = std::env::temp_dir().join("cmremote-update-stage");
    let server_host = info.normalized_host();
    let mut composite = CompositePackageProvider::new();
    composite.register_default_handlers(cache_dir, server_host, Arc::new(RejectingDownloader));
    let packages = Arc::new(composite);

    // Slice M3 (gated on R6): the agent self-update handler shares
    // the same downloader as the package providers. The installer is
    // platform-specific and not yet implemented; until then the stub
    // installer surfaces a clean structured failure so the manifest
    // dispatcher's audit trail is honest about the missing capability.
    let agent_update = Arc::new(crate::handlers::agent_update::AgentUpdateContext {
        downloader: Arc::new(RejectingDownloader),
        installer: Arc::new(crate::handlers::agent_update::StubAgentUpdateInstaller),
        stage_dir,
    });

    let handlers = Arc::new(AgentHandlers {
        connection_info: info.clone(),
        device_info,
        apps,
        packages,
        agent_update,
    });

    // The shutdown channel is a single-producer / multi-consumer
    // boolean: the OS-signal task flips it from `false` to `true`,
    // and every owner of a `watch::Receiver` (currently: the
    // transport loop) bails out cooperatively.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let signal_handle = tokio::spawn(async move {
        wait_for_shutdown().await;
        info!("shutdown signal received; stopping agent");
        let _ = shutdown_tx.send(true);
    });

    let transport_result = transport::run_until_shutdown(info, handlers, shutdown_rx).await;

    // Make sure the signal task has finished — otherwise we leak it
    // for the (vanishingly small) window between transport exit and
    // process shutdown.
    let _ = signal_handle.await;

    transport_result?;
    Ok(())
}

fn log_startup_banner(info: &ConnectionInfo, host: &cmremote_platform::DeviceSnapshot) {
    info!(
        device_id = %info.device_id,
        organization_id = info.organization_id.as_deref().unwrap_or(""),
        host = info.normalized_host().as_deref().unwrap_or(""),
        os = host.os.as_str(),
        os_description = %host.os_description,
        architecture = %host.architecture,
        "cmremote-agent starting (slice R2a: hub dispatch surface)"
    );
}

/// Resolve when the OS asks us to shut down.
///
/// Listens for SIGINT on every platform, plus SIGTERM on Unix. On
/// Windows `ctrl_c` covers both `Ctrl+C` and the SCM stop signal.
async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        use tracing::warn;
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGTERM handler; falling back to SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_does_not_panic_with_minimal_config() {
        let info = ConnectionInfo {
            device_id: "d".into(),
            host: Some("https://example.com".into()),
            organization_id: Some("o".into()),
            server_verification_token: None,
            organization_token: None,
        };
        let host = StdDeviceInfoProvider.snapshot().unwrap();
        log_startup_banner(&info, &host);
    }
}
