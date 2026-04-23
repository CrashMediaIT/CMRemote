// Source: CMRemote, clean-room implementation.

//! Top-level async runtime for the agent.
//!
//! In R0 this only sets up structured logging, loads configuration,
//! prints a startup banner, and waits for a shutdown signal. The
//! connection / heartbeat loop is added in slice R2.

use cmremote_platform::{DeviceInfoProvider, StdDeviceInfoProvider};
use cmremote_wire::ConnectionInfo;
use tracing::{info, warn};

use crate::cli::CliArgs;

/// Errors surfaced from the agent runtime.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Configuration could not be loaded or validated.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    /// Failure querying OS state.
    #[error(transparent)]
    Platform(#[from] cmremote_platform::PlatformError),
}

/// Run the agent until a shutdown signal is received.
///
/// Returns `Ok(())` on a clean shutdown.
pub async fn run(cli: CliArgs) -> Result<(), RuntimeError> {
    let info = crate::config::build(&cli)?;
    let host = StdDeviceInfoProvider.snapshot()?;

    log_startup_banner(&info, &host);

    wait_for_shutdown().await;

    info!("shutdown signal received; stopping agent");
    Ok(())
}

fn log_startup_banner(info: &ConnectionInfo, host: &cmremote_platform::HostDescriptor) {
    info!(
        device_id = %info.device_id,
        organization_id = info.organization_id.as_deref().unwrap_or(""),
        host = info.normalized_host().as_deref().unwrap_or(""),
        os = host.os.as_str(),
        os_description = %host.os_description,
        architecture = %host.architecture,
        "cmremote-agent starting (R0 scaffold; no network I/O yet)"
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
        };
        let host = cmremote_platform::HostDescriptor::from_std();
        log_startup_banner(&info, &host);
    }
}
