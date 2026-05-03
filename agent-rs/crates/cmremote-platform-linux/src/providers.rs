// Source: CMRemote, clean-room implementation.

//! Linux bundle factory for [`DesktopProviders`].

use std::sync::Arc;

use cmremote_platform::desktop::{DesktopProviders, LoggingSessionNotifier};
use thiserror::Error;

use crate::capture::{LinuxCaptureError, XwdDesktopCapturer};
use crate::encoder::{FfmpegH264EncoderFactory, LinuxEncoderError};
use crate::input::{LinuxClipboard, LinuxInputError, XdotoolKeyboardInput, XdotoolMouseInput};
use crate::notification::NotifySendSessionNotifier;

/// Errors surfaced by [`LinuxDesktopProviders::for_current_desktop`].
#[derive(Debug, Error)]
pub enum LinuxProvidersError {
    /// The host is not Linux.
    #[error("Linux desktop providers can only be constructed on Linux")]
    WrongHost,
    /// Screen capture driver construction failed.
    #[error(transparent)]
    Capture(#[from] LinuxCaptureError),
    /// No X11 desktop display is available.
    #[error("DISPLAY is not set; no X11 desktop session is available")]
    NoDisplay,
    /// Encoder construction failed.
    #[error(transparent)]
    Encoder(#[from] LinuxEncoderError),
    /// Input or clipboard construction failed.
    #[error(transparent)]
    Input(#[from] LinuxInputError),
}

/// Factory namespace for Linux desktop bundles.
pub struct LinuxDesktopProviders;

impl LinuxDesktopProviders {
    /// Build a concrete Linux desktop provider bundle.
    ///
    /// This requires `xwd`, `xdotool`, `ffmpeg`, and either
    /// `wl-copy`/`wl-paste` or `xclip` on PATH. Missing tools are
    /// surfaced as structured errors so the runtime can fall back to
    /// `DesktopProviders::not_supported_for_current_host()`.
    pub fn for_current_desktop() -> Result<DesktopProviders, LinuxProvidersError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(LinuxProvidersError::WrongHost)
        }
        #[cfg(target_os = "linux")]
        {
            Self::build_checked()
        }
    }

    /// Build after checking command prerequisites.
    pub fn build_checked() -> Result<DesktopProviders, LinuxProvidersError> {
        if std::env::var_os("DISPLAY").is_none() {
            return Err(LinuxProvidersError::NoDisplay);
        }
        let capturer = XwdDesktopCapturer::new()?;
        let encoder_factory = FfmpegH264EncoderFactory::new()?;
        let mouse = XdotoolMouseInput::new()?;
        let keyboard = XdotoolKeyboardInput::new()?;
        let clipboard = LinuxClipboard::new()?;
        let notifier: Arc<dyn cmremote_platform::desktop::SessionNotifier> =
            match NotifySendSessionNotifier::new() {
                Some(n) => Arc::new(n),
                None => {
                    tracing::warn!(
                        "notify-send not found; desktop-session notifications will be logged only"
                    );
                    Arc::new(LoggingSessionNotifier)
                }
            };
        Ok(DesktopProviders {
            capturer: Arc::new(capturer),
            encoder_factory: Arc::new(encoder_factory),
            mouse: Arc::new(mouse),
            keyboard: Arc::new(keyboard),
            clipboard: Arc::new(clipboard),
            notifier,
        })
    }

    /// Build a bundle without checking host executables. Intended for
    /// unit tests that only need trait-object shape.
    pub fn unchecked_for_tests() -> DesktopProviders {
        DesktopProviders {
            capturer: Arc::new(XwdDesktopCapturer),
            encoder_factory: Arc::new(FfmpegH264EncoderFactory),
            mouse: Arc::new(XdotoolMouseInput),
            keyboard: Arc::new(XdotoolKeyboardInput),
            clipboard: Arc::new(LinuxClipboard::WlClipboard),
            notifier: Arc::new(LoggingSessionNotifier),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmremote_platform::desktop::SessionNotification;

    #[test]
    fn unchecked_bundle_has_all_slots() {
        let bundle = LinuxDesktopProviders::unchecked_for_tests();
        let _: &dyn cmremote_platform::desktop::DesktopCapturer = &*bundle.capturer;
        let _: &dyn cmremote_platform::desktop::EncoderFactory = &*bundle.encoder_factory;
        let _: &dyn cmremote_platform::desktop::MouseInput = &*bundle.mouse;
        let _: &dyn cmremote_platform::desktop::KeyboardInput = &*bundle.keyboard;
        let _: &dyn cmremote_platform::desktop::Clipboard = &*bundle.clipboard;
        let _: &dyn cmremote_platform::desktop::SessionNotifier = &*bundle.notifier;
    }

    /// Hosted-CI/lab validation for the Linux desktop stack. Run under
    /// Xvfb with `xwd`, `xdotool`, `xclip`, `ffmpeg`, and `notify-send`
    /// installed:
    ///
    /// `xvfb-run -a cargo test -p cmremote-platform-linux desktop_lab -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "requires Xvfb and native Linux desktop helper binaries"]
    async fn desktop_lab_captures_encodes_and_notifies_without_prompting() {
        let bundle = LinuxDesktopProviders::build_checked().expect("linux providers");

        let notification = SessionNotification::sanitised(
            "11111111-2222-3333-4444-555555555555",
            "Lab Viewer",
            "CMRemote Lab",
            "viewer-conn",
        )
        .expect("valid notification");
        bundle.notifier.session_connected(&notification).await;

        let frame = bundle
            .capturer
            .capture_next_frame()
            .await
            .expect("xwd frame");
        assert!(frame.width > 0);
        assert!(frame.height > 0);
        assert_eq!(frame.stride, frame.width * 4);
        assert_eq!(frame.bgra.len(), (frame.stride * frame.height) as usize);

        let encoder = bundle.encoder_factory.build().expect("ffmpeg encoder");
        encoder.request_keyframe();
        let encoded = encoder.encode(&frame).await.expect("h264 frame");
        assert!(!encoded.bytes.is_empty());
        assert_eq!(encoded.timestamp_micros, frame.timestamp_micros);
        assert!(encoded.is_keyframe);

        bundle
            .notifier
            .session_disconnected(&notification, "desktop-lab-complete")
            .await;
    }
}
