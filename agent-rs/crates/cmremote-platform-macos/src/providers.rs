// Source: CMRemote, clean-room implementation.

//! macOS bundle factory for [`DesktopProviders`].

use std::sync::Arc;

use cmremote_platform::desktop::{DesktopProviders, LoggingSessionNotifier};
use thiserror::Error;

use crate::capture::{BmpDesktopCapturer, MacOsCaptureError};
use crate::encoder::{FfmpegH264EncoderFactory, MacOsEncoderError};
use crate::input::{
    AppleScriptKeyboardInput, AppleScriptMouseInput, MacOsClipboard, MacOsInputError,
};
use crate::notification::MacOsSessionNotifier;

/// Errors surfaced by [`MacOsDesktopProviders::for_current_desktop`].
#[derive(Debug, Error)]
pub enum MacOsProvidersError {
    /// The host is not macOS.
    #[error("macOS desktop providers can only be constructed on macOS")]
    WrongHost,
    /// Screen capture construction failed.
    #[error(transparent)]
    Capture(#[from] MacOsCaptureError),
    /// Encoder construction failed.
    #[error(transparent)]
    Encoder(#[from] MacOsEncoderError),
    /// Input or clipboard construction failed.
    #[error(transparent)]
    Input(#[from] MacOsInputError),
}

/// Factory namespace for macOS desktop bundles.
pub struct MacOsDesktopProviders;

impl MacOsDesktopProviders {
    /// Build a concrete macOS desktop provider bundle.
    ///
    /// Requires `screencapture`, `ffmpeg`, `osascript`, `cliclick`,
    /// `pbcopy`, and `pbpaste`. Missing tools return structured
    /// errors so the runtime can fall back to a `NotSupported` bundle.
    pub fn for_current_desktop() -> Result<DesktopProviders, MacOsProvidersError> {
        #[cfg(not(target_os = "macos"))]
        {
            Err(MacOsProvidersError::WrongHost)
        }
        #[cfg(target_os = "macos")]
        {
            Self::build_checked()
        }
    }

    /// Build after checking command prerequisites.
    pub fn build_checked() -> Result<DesktopProviders, MacOsProvidersError> {
        let capturer = BmpDesktopCapturer::new()?;
        let encoder_factory = FfmpegH264EncoderFactory::new()?;
        let mouse = AppleScriptMouseInput::new()?;
        let keyboard = AppleScriptKeyboardInput::new()?;
        let clipboard = MacOsClipboard::new()?;
        let notifier: Arc<dyn cmremote_platform::desktop::SessionNotifier> =
            match MacOsSessionNotifier::new() {
                Some(n) => Arc::new(n),
                None => {
                    tracing::warn!(
                        "osascript not found; desktop-session notifications will be logged only"
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
            capturer: Arc::new(BmpDesktopCapturer),
            encoder_factory: Arc::new(FfmpegH264EncoderFactory),
            mouse: Arc::new(AppleScriptMouseInput),
            keyboard: Arc::new(AppleScriptKeyboardInput),
            clipboard: Arc::new(MacOsClipboard),
            notifier: Arc::new(LoggingSessionNotifier),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchecked_bundle_has_all_slots() {
        let bundle = MacOsDesktopProviders::unchecked_for_tests();
        let _: &dyn cmremote_platform::desktop::DesktopCapturer = &*bundle.capturer;
        let _: &dyn cmremote_platform::desktop::EncoderFactory = &*bundle.encoder_factory;
        let _: &dyn cmremote_platform::desktop::MouseInput = &*bundle.mouse;
        let _: &dyn cmremote_platform::desktop::KeyboardInput = &*bundle.keyboard;
        let _: &dyn cmremote_platform::desktop::Clipboard = &*bundle.clipboard;
        let _: &dyn cmremote_platform::desktop::SessionNotifier = &*bundle.notifier;
    }
}
