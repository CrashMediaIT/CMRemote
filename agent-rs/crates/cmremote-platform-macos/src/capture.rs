// Source: CMRemote, clean-room implementation.

//! macOS desktop capture driver backed by `screencapture` BMP output.

use async_trait::async_trait;
use cmremote_platform::desktop::{CapturedFrame, DesktopCapturer, DesktopMediaError};
use thiserror::Error;
use tokio::process::Command;

/// Errors surfaced by the macOS capture driver.
#[derive(Debug, Error)]
pub enum MacOsCaptureError {
    /// The `screencapture` executable is not available.
    #[error("screencapture is not available on PATH")]
    MissingScreenCapture,
    /// The capture command failed.
    #[error("screencapture failed: {0}")]
    Process(String),
    /// The BMP output was malformed or unsupported.
    #[error("invalid BMP frame: {0}")]
    InvalidBmp(String),
}

impl From<MacOsCaptureError> for DesktopMediaError {
    fn from(value: MacOsCaptureError) -> Self {
        match value {
            MacOsCaptureError::MissingScreenCapture => {
                DesktopMediaError::NotSupported(cmremote_platform::HostOs::MacOs)
            }
            MacOsCaptureError::Process(e) | MacOsCaptureError::InvalidBmp(e) => {
                DesktopMediaError::Io(e)
            }
        }
    }
}

/// macOS capturer using `screencapture -x -t bmp` and a BMP parser.
#[derive(Debug, Default)]
pub struct BmpDesktopCapturer;

impl BmpDesktopCapturer {
    /// Construct a capturer after checking for `screencapture`.
    pub fn new() -> Result<Self, MacOsCaptureError> {
        if !crate::command_exists("screencapture") {
            return Err(MacOsCaptureError::MissingScreenCapture);
        }
        Ok(Self)
    }

    /// Parse uncompressed 24-bit or 32-bit BMP data into BGRA.
    pub fn parse_bmp(
        bytes: &[u8],
        timestamp_micros: u64,
    ) -> Result<CapturedFrame, MacOsCaptureError> {
        parse_bmp(bytes, timestamp_micros)
    }
}

#[async_trait]
impl DesktopCapturer for BmpDesktopCapturer {
    async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
        let path = std::env::temp_dir().join(format!(
            "cmremote-screencapture-{}-{}.bmp",
            std::process::id(),
            current_timestamp_micros()
        ));
        let status = Command::new("screencapture")
            .args(["-x", "-t", "bmp"])
            .arg(&path)
            .status()
            .await
            .map_err(|e| DesktopMediaError::Io(format!("screencapture spawn failed: {e}")))?;
        if !status.success() {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(DesktopMediaError::Io(format!(
                "screencapture exited {:?}",
                status.code()
            )));
        }
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| DesktopMediaError::Io(format!("failed to read screenshot: {e}")))?;
        let _ = tokio::fs::remove_file(&path).await;
        parse_bmp(&bytes, current_timestamp_micros()).map_err(Into::into)
    }
}

fn current_timestamp_micros() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn parse_bmp(bytes: &[u8], timestamp_micros: u64) -> Result<CapturedFrame, MacOsCaptureError> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err(MacOsCaptureError::InvalidBmp("missing BMP header".into()));
    }
    let pixel_offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    let dib_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    if dib_size < 40 {
        return Err(MacOsCaptureError::InvalidBmp(
            "unsupported DIB header".into(),
        ));
    }
    let width_i = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let height_i = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
    let planes = u16::from_le_bytes([bytes[26], bytes[27]]);
    let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
    let compression = u32::from_le_bytes([bytes[30], bytes[31], bytes[32], bytes[33]]);
    if planes != 1 || compression != 0 {
        return Err(MacOsCaptureError::InvalidBmp(
            "compressed BMP is unsupported".into(),
        ));
    }
    if width_i <= 0 || height_i == 0 {
        return Err(MacOsCaptureError::InvalidBmp("invalid dimensions".into()));
    }
    if bpp != 24 && bpp != 32 {
        return Err(MacOsCaptureError::InvalidBmp(format!(
            "unsupported bits_per_pixel {bpp}"
        )));
    }
    let top_down = height_i < 0;
    let width = width_i as u32;
    let height = height_i.unsigned_abs();
    let src_stride = ((u32::from(bpp) * width).div_ceil(32)) * 4;
    let required = pixel_offset
        .checked_add((src_stride as usize).saturating_mul(height as usize))
        .ok_or_else(|| MacOsCaptureError::InvalidBmp("image size overflow".into()))?;
    if bytes.len() < required {
        return Err(MacOsCaptureError::InvalidBmp(
            "file shorter than pixel data".into(),
        ));
    }
    let dst_stride = width
        .checked_mul(4)
        .ok_or_else(|| MacOsCaptureError::InvalidBmp("BGRA stride overflow".into()))?;
    let mut bgra = vec![0u8; (dst_stride as usize) * (height as usize)];
    let src_bpp = (bpp / 8) as usize;
    for y in 0..height as usize {
        let src_y = if top_down { y } else { height as usize - 1 - y };
        let src_row = pixel_offset + src_y * src_stride as usize;
        let dst_row = y * dst_stride as usize;
        for x in 0..width as usize {
            let s = src_row + x * src_bpp;
            let d = dst_row + x * 4;
            bgra[d] = bytes[s];
            bgra[d + 1] = bytes[s + 1];
            bgra[d + 2] = bytes[s + 2];
            bgra[d + 3] = if src_bpp == 4 {
                bytes[s + 3].max(0x01)
            } else {
                0xff
            };
        }
    }
    Ok(CapturedFrame {
        width,
        height,
        stride: dst_stride,
        timestamp_micros,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp_2x1_24() -> Vec<u8> {
        let mut out = vec![0u8; 54];
        out[0..2].copy_from_slice(b"BM");
        out[10..14].copy_from_slice(&(54u32).to_le_bytes());
        out[14..18].copy_from_slice(&(40u32).to_le_bytes());
        out[18..22].copy_from_slice(&(2i32).to_le_bytes());
        out[22..26].copy_from_slice(&(1i32).to_le_bytes());
        out[26..28].copy_from_slice(&(1u16).to_le_bytes());
        out[28..30].copy_from_slice(&(24u16).to_le_bytes());
        out.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]);
        out
    }

    #[test]
    fn parses_bmp_to_bgra() {
        let frame = BmpDesktopCapturer::parse_bmp(&bmp_2x1_24(), 7).unwrap();
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.bgra, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(frame.timestamp_micros, 7);
    }

    #[test]
    fn rejects_non_bmp() {
        assert!(matches!(
            BmpDesktopCapturer::parse_bmp(b"not a bmp", 0),
            Err(MacOsCaptureError::InvalidBmp(_))
        ));
    }
}
