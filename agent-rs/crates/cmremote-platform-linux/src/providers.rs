// Source: CMRemote, clean-room implementation.

//! Linux bundle factory for [`DesktopProviders`].

use std::sync::Arc;

use cmremote_platform::desktop::DesktopProviders;
use thiserror::Error;

use crate::capture::{LinuxCaptureError, XwdDesktopCapturer};
use crate::encoder::{FfmpegH264EncoderFactory, LinuxEncoderError};
use crate::input::{LinuxClipboard, LinuxInputError, XdotoolKeyboardInput, XdotoolMouseInput};

/// Errors surfaced by [`LinuxDesktopProviders::for_current_desktop`].
#[derive(Debug, Error)]
pub enum LinuxProvidersError {
    /// The host is not Linux.
    #[error("Linux desktop providers can only be constructed on Linux")]
    WrongHost,
    /// Screen capture driver construction failed.
    #[error(transparent)]
    Capture(#[from] LinuxCaptureError),
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
            return Err(LinuxProvidersError::WrongHost);
        }
        #[cfg(target_os = "linux")]
        {
            Self::build_checked()
        }
    }

    /// Build after checking command prerequisites.
    pub fn build_checked() -> Result<DesktopProviders, LinuxProvidersError> {
        let capturer = XwdDesktopCapturer::new()?;
        let encoder_factory = FfmpegH264EncoderFactory::new()?;
        let mouse = XdotoolMouseInput::new()?;
        let keyboard = XdotoolKeyboardInput::new()?;
        let clipboard = LinuxClipboard::new()?;
        Ok(DesktopProviders {
            capturer: Arc::new(capturer),
            encoder_factory: Arc::new(encoder_factory),
            mouse: Arc::new(mouse),
            keyboard: Arc::new(keyboard),
            clipboard: Arc::new(clipboard),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchecked_bundle_has_all_slots() {
        let bundle = LinuxDesktopProviders::unchecked_for_tests();
        let _: &dyn cmremote_platform::desktop::DesktopCapturer = &*bundle.capturer;
        let _: &dyn cmremote_platform::desktop::EncoderFactory = &*bundle.encoder_factory;
        let _: &dyn cmremote_platform::desktop::MouseInput = &*bundle.mouse;
        let _: &dyn cmremote_platform::desktop::KeyboardInput = &*bundle.keyboard;
        let _: &dyn cmremote_platform::desktop::Clipboard = &*bundle.clipboard;
    }
}
