// Source: CMRemote, clean-room implementation.

//! Top-level async runtime for the agent.
//!
//! Slice R0 set up structured logging, configuration, and a shutdown
//! signal. Slice R2 layers the WebSocket connection / heartbeat /
//! reconnect loop on top. Slice R2a wires in the hub dispatch layer
//! (R3/R4/R5 handlers follow).

use std::sync::Arc;

#[cfg(not(feature = "webrtc-driver"))]
use cmremote_platform::desktop::NotSupportedDesktopTransport;
#[cfg(feature = "webrtc-driver")]
use cmremote_platform::desktop::WebRtcDesktopTransport;
use cmremote_platform::desktop::{DesktopProviders, DesktopTransportProvider};
#[cfg(target_os = "linux")]
use cmremote_platform::linux_apps::DpkgProvider;
use cmremote_platform::packages::{
    ArtifactDownloader, CompositePackageProvider, RejectingDownloader, ReqwestArtifactDownloader,
};
#[cfg(not(target_os = "linux"))]
use cmremote_platform::stubs::NotSupportedAppsProvider;
use cmremote_platform::{DeviceInfoProvider, StdDeviceInfoProvider};
use cmremote_wire::ConnectionInfo;
use tokio::sync::watch;
use tracing::info;

#[cfg(target_os = "linux")]
use cmremote_platform_linux::LinuxDesktopProviders;
#[cfg(target_os = "macos")]
use cmremote_platform_macos::MacOsDesktopProviders;

use crate::cli::CliArgs;
use crate::handlers::AgentHandlers;
use crate::transport::signalling::HubBoundSignallingEgress;
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
    // is the real `ReqwestArtifactDownloader` (rustls + aws-lc-rs, no
    // `ring`, no `openssl-sys`); if its construction fails for any
    // reason — e.g. the rustls stack failed to initialise — we fall
    // back to the rejecting stub so MSI / Executable jobs surface a
    // clean "this agent is not configured to download package
    // artifacts" failure rather than panicking the whole agent on the
    // download path.
    let cache_dir = std::env::temp_dir().join("cmremote-package-cache");
    let stage_dir = std::env::temp_dir().join("cmremote-update-stage");
    let server_host = info.normalized_host();
    let downloader: Arc<dyn ArtifactDownloader> = match ReqwestArtifactDownloader::new() {
        Ok(d) => Arc::new(d),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to construct HTTPS artifact downloader (rustls / aws-lc-rs init failed — \
                 typically caused by a competing crypto provider already installed by another \
                 library); falling back to RejectingDownloader so package + agent-update jobs \
                 surface a clean structured failure instead of hanging",
            );
            Arc::new(RejectingDownloader)
        }
    };
    let mut composite = CompositePackageProvider::new();
    composite.register_default_handlers(cache_dir, server_host, downloader.clone());
    let packages = Arc::new(composite);

    // Slice R8: the agent self-update handler shares the same
    // downloader as the package providers, then hands the verified
    // artifact to the native package installer selected from the
    // staged file extension (.deb/.rpm/.msi/.pkg). Unsupported
    // host/artifact combinations still surface a clean structured
    // failure so the manifest dispatcher's audit trail is honest.
    let agent_update = Arc::new(crate::handlers::agent_update::AgentUpdateContext {
        downloader,
        installer: Arc::new(
            crate::handlers::agent_update::PackageAgentUpdateInstaller::for_current_host(),
        ),
        stage_dir,
    });

    // Slice R7.n.4/R7.o — per-host bundle of desktop capability providers
    // (capturer + encoder + mouse + keyboard + clipboard). On Windows we try
    // `WindowsDesktopProviders::for_primary_output`, which composes
    // the DXGI capturer with the three `SendInput` / `CF_UNICODETEXT`
    // drivers and gates construction on `WindowsSessionInfo` so an
    // agent running in Session 0 (services / `LocalSystem`) or in a
    // session that doesn't share the active console falls back to
    // the `NotSupported` bundle instead of `SendInput` silently
    // swallowing every event. Construction failure (no D3D11 device,
    // no primary output, query failure, non-interactive session) is
    // warn-logged and falls back to the `NotSupported` bundle so the
    // agent always starts; the desktop dispatch surface then
    // surfaces a structured "not supported on <OS>" failure to the
    // operator. Linux and macOS now follow the same pattern through
    // their own platform crates: provider construction verifies the
    // required host command surfaces before returning a concrete
    // bundle, and any missing prerequisite falls back to NotSupported.
    let desktop_providers = Arc::new(build_desktop_providers());

    // Slice R7 — desktop transport. By default, until the WebRTC
    // capture / encode driver lands, every request resolves to a
    // structured "not supported on <OS>" failure
    // (`NotSupportedDesktopTransport`). When the workspace is built
    // with `--features cmremote-agent/webrtc-driver` (slices R7.k +
    // R7.m), the runtime swaps in `WebRtcDesktopTransport` — a
    // concrete provider that owns a per-session state machine,
    // audit-logs every transition, and drives a real
    // `RTCPeerConnection` via the `webrtc` crate (resolved through
    // the workspace `[patch.crates-io]` pin to the CMRemote fork at
    // `v0.17.0-cmremote.1`). The dispatcher's
    // `Arc<dyn DesktopTransportProvider>` slot is unchanged either
    // way.
    //
    // Slice R7.n.5 — when `webrtc-driver` is on, inject the per-host
    // `desktop_providers` bundle and a `DiscardingCaptureSink` into
    // the transport via `with_providers`. Each `RemoteControl` then
    // spawns a per-session capture pump that drives the bundle's
    // capturer at `CapturePumpConfig::default().target_fps` (30 fps)
    // and pushes captured frames into the sink. Until slice R7.n.6
    // lands the Media Foundation H.264 encoder, the sink discards
    // every frame after counting it.
    // Slice R7.n.7 — hub-bound signalling egress. Built once for the
    // whole agent lifetime; the transport reconnect loop re-binds it
    // to each fresh per-connection outbound channel via
    // `bind` / `unbind`. When the workspace is built without the
    // `webrtc-driver` feature, the `WebRtcDesktopTransport` is not
    // constructed at all and the egress remains unbound — its impl
    // is a no-op in that build.
    let signalling_egress = Arc::new(HubBoundSignallingEgress::new());

    #[cfg(not(feature = "webrtc-driver"))]
    let desktop: Arc<dyn DesktopTransportProvider> = Arc::new(
        NotSupportedDesktopTransport::for_current_host(info.organization_id.clone()),
    );
    #[cfg(feature = "webrtc-driver")]
    let desktop: Arc<dyn DesktopTransportProvider> =
        Arc::new(WebRtcDesktopTransport::with_providers_and_egress(
            cmremote_platform::HostOs::current(),
            info.organization_id.clone(),
            desktop_providers.clone(),
            Arc::new(cmremote_platform::desktop::DiscardingCaptureSink::new()),
            cmremote_platform::desktop::CapturePumpConfig::default(),
            signalling_egress.clone(),
        ));

    let handlers = Arc::new(AgentHandlers {
        connection_info: info.clone(),
        device_info,
        apps,
        packages,
        agent_update,
        desktop,
        desktop_providers,
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

    let transport_result =
        transport::run_until_shutdown(info, handlers, signalling_egress, shutdown_rx).await;

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

/// Construct the per-host [`DesktopProviders`] bundle (slice R7.n.4/R7.o).
///
/// On Windows attempts the real DXGI capturer + `SendInput` /
/// `CF_UNICODETEXT` drivers via
/// [`cmremote_platform_windows::WindowsDesktopProviders::for_primary_output`].
/// On Linux attempts the XWD/ffmpeg/xdotool/clipboard-command bundle
/// via [`cmremote_platform_linux::LinuxDesktopProviders::for_current_desktop`].
/// On macOS attempts the screencapture/ffmpeg/AppleScript/cliclick
/// bundle via [`cmremote_platform_macos::MacOsDesktopProviders::for_current_desktop`].
/// Falls back to [`DesktopProviders::not_supported_for_current_host`]
/// (with a warn-log naming the failure mode) on any error so the
/// agent always starts.
fn build_desktop_providers() -> DesktopProviders {
    #[cfg(target_os = "windows")]
    {
        match cmremote_platform_windows::WindowsDesktopProviders::for_primary_output() {
            Ok(bundle) => {
                info!(
                    "desktop providers: Windows DXGI capturer + SendInput drivers + \
                     CF_UNICODETEXT clipboard"
                );
                bundle
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to construct Windows desktop providers \
                     (e.g. agent in Session 0, no interactive desktop attached, \
                     or no D3D11 device); falling back to NotSupported bundle so \
                     desktop-control requests surface a structured failure"
                );
                DesktopProviders::not_supported_for_current_host()
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        match LinuxDesktopProviders::for_current_desktop() {
            Ok(bundle) => {
                info!(
                    "desktop providers: Linux XWD capture + ffmpeg H.264 encoder + \
                     xdotool input + wl-clipboard/xclip clipboard"
                );
                bundle
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to construct Linux desktop providers \
                     (missing xwd/xdotool/ffmpeg/clipboard tools or no desktop session); \
                     falling back to NotSupported bundle"
                );
                DesktopProviders::not_supported_for_current_host()
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        match MacOsDesktopProviders::for_current_desktop() {
            Ok(bundle) => {
                info!(
                    "desktop providers: macOS screencapture + ffmpeg H.264 encoder + \
                     AppleScript/cliclick input + pbcopy/pbpaste clipboard"
                );
                bundle
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to construct macOS desktop providers \
                     (missing screencapture/ffmpeg/osascript/cliclick/pbcopy/pbpaste \
                     or no authorised desktop session); falling back to NotSupported bundle"
                );
                DesktopProviders::not_supported_for_current_host()
            }
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        info!(
            "desktop providers: NotSupported bundle (no concrete drivers available \
             for this host)"
        );
        DesktopProviders::not_supported_for_current_host()
    }
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

    #[tokio::test]
    async fn build_desktop_providers_returns_a_usable_bundle() {
        // The bundle must always be constructible — on non-Windows
        // hosts (including Linux CI) the fallback `NotSupported`
        // bundle is the only available bundle, and on Windows the
        // factory falls back to the same bundle when the live
        // session topology rules input injection out. Either way
        // the agent always starts with a non-null `desktop_providers`
        // slot in `AgentHandlers`.
        let bundle = build_desktop_providers();
        // Smoke-check every slot is wired (a `NotSupported` slot
        // surfaces a structured error on first use, which is the
        // contract — never panics).
        let _ = bundle.capturer.capture_next_frame().await;
        let _ = bundle.mouse.move_to(0, 0).await;
        let _ = bundle.keyboard.type_text("").await;
        let _ = bundle.clipboard.read_text().await;
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn build_desktop_providers_returns_not_supported_bundle_off_windows() {
        // On non-Windows hosts the bundle must surface
        // `NotSupported` for every operation — we have no concrete
        // drivers for those OSes today.
        let bundle = build_desktop_providers();
        let e = bundle.capturer.capture_next_frame().await.unwrap_err();
        assert!(
            e.to_string().to_lowercase().contains("not supported"),
            "{e}"
        );
        let e = bundle.mouse.move_to(0, 0).await.unwrap_err();
        assert!(
            e.to_string().to_lowercase().contains("not supported"),
            "{e}"
        );
        let e = bundle.keyboard.type_text("hi").await.unwrap_err();
        assert!(
            e.to_string().to_lowercase().contains("not supported"),
            "{e}"
        );
        let e = bundle.clipboard.read_text().await.unwrap_err();
        assert!(
            e.to_string().to_lowercase().contains("not supported"),
            "{e}"
        );
    }
}
