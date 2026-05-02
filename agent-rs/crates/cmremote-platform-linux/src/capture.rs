// Source: CMRemote, clean-room implementation.

//! X11 desktop capture driver backed by `xwd`.
//!
//! `xwd -root -silent` produces an X Window Dump stream containing a
//! small fixed header followed by raw pixels. The parser below accepts
//! the common `ZPixmap` 24-bit and 32-bit forms and normalises them to
//! the cross-platform [`CapturedFrame`] BGRA contract.

use async_trait::async_trait;
use cmremote_platform::desktop::{CapturedFrame, DesktopCapturer, DesktopMediaError};
use thiserror::Error;
use tokio::process::Command;

/// Errors surfaced by the Linux capture driver.
#[derive(Debug, Error)]
pub enum LinuxCaptureError {
    /// The `xwd` executable is not available on PATH.
    #[error("xwd is not available on PATH")]
    MissingXwd,
    /// The `xwd` process failed.
    #[error("xwd failed: {0}")]
    Process(String),
    /// The XWD stream was malformed or used an unsupported pixel format.
    #[error("invalid XWD frame: {0}")]
    InvalidXwd(String),
}

impl From<LinuxCaptureError> for DesktopMediaError {
    fn from(value: LinuxCaptureError) -> Self {
        match value {
            LinuxCaptureError::MissingXwd => {
                DesktopMediaError::NotSupported(cmremote_platform::HostOs::Linux)
            }
            LinuxCaptureError::Process(e) | LinuxCaptureError::InvalidXwd(e) => {
                DesktopMediaError::Io(e)
            }
        }
    }
}

/// Linux desktop capturer that shells out to `xwd` and parses the
/// returned XWD stream.
#[derive(Debug, Default)]
pub struct XwdDesktopCapturer;

impl XwdDesktopCapturer {
    /// Construct a capturer after verifying `xwd` exists.
    pub fn new() -> Result<Self, LinuxCaptureError> {
        if !crate::command_exists("xwd") {
            return Err(LinuxCaptureError::MissingXwd);
        }
        Ok(Self)
    }

    /// Parse a raw XWD byte stream into a BGRA frame. Public for unit
    /// tests and for future PipeWire/X11 fallback experiments.
    pub fn parse_xwd(
        bytes: &[u8],
        timestamp_micros: u64,
    ) -> Result<CapturedFrame, LinuxCaptureError> {
        parse_xwd(bytes, timestamp_micros)
    }
}

#[async_trait]
impl DesktopCapturer for XwdDesktopCapturer {
    async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
        let output = Command::new("xwd")
            .args(["-root", "-silent"])
            .output()
            .await
            .map_err(|e| DesktopMediaError::Io(format!("xwd spawn failed: {e}")))?;
        if !output.status.success() {
            return Err(DesktopMediaError::Io(format!(
                "xwd exited with status {:?}",
                output.status.code()
            )));
        }
        let ts = current_timestamp_micros();
        parse_xwd(&output.stdout, ts).map_err(Into::into)
    }
}

fn current_timestamp_micros() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
enum Endian {
    Big,
    Little,
}

impl Endian {
    fn read_u32(self, b: &[u8]) -> u32 {
        match self {
            Endian::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            Endian::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
}

fn parse_xwd(bytes: &[u8], timestamp_micros: u64) -> Result<CapturedFrame, LinuxCaptureError> {
    if bytes.len() < 100 {
        return Err(LinuxCaptureError::InvalidXwd(
            "stream shorter than fixed header".into(),
        ));
    }
    let endian = if u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) == 7 {
        Endian::Big
    } else if u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) == 7 {
        Endian::Little
    } else {
        return Err(LinuxCaptureError::InvalidXwd(
            "unsupported XWD version".into(),
        ));
    };

    let read = |idx: usize| -> u32 {
        let start = idx * 4;
        endian.read_u32(&bytes[start..start + 4])
    };

    let header_size = read(0) as usize;
    let pixmap_format = read(2);
    let width = read(4);
    let height = read(5);
    let bits_per_pixel = read(11);
    let bytes_per_line = read(12);
    let ncolors = read(19) as usize;

    if pixmap_format != 2 {
        return Err(LinuxCaptureError::InvalidXwd(format!(
            "unsupported pixmap format {pixmap_format}; expected ZPixmap"
        )));
    }
    if width == 0 || height == 0 {
        return Err(LinuxCaptureError::InvalidXwd("zero-sized frame".into()));
    }
    if bits_per_pixel != 24 && bits_per_pixel != 32 {
        return Err(LinuxCaptureError::InvalidXwd(format!(
            "unsupported bits_per_pixel {bits_per_pixel}; expected 24 or 32"
        )));
    }
    let color_map_bytes = ncolors
        .checked_mul(12)
        .ok_or_else(|| LinuxCaptureError::InvalidXwd("color map overflow".into()))?;
    let pixel_offset = header_size
        .checked_add(color_map_bytes)
        .ok_or_else(|| LinuxCaptureError::InvalidXwd("pixel offset overflow".into()))?;
    let required = pixel_offset
        .checked_add((bytes_per_line as usize).saturating_mul(height as usize))
        .ok_or_else(|| LinuxCaptureError::InvalidXwd("image size overflow".into()))?;
    if bytes.len() < required {
        return Err(LinuxCaptureError::InvalidXwd(
            "stream shorter than image payload".into(),
        ));
    }

    let stride = width
        .checked_mul(4)
        .ok_or_else(|| LinuxCaptureError::InvalidXwd("BGRA stride overflow".into()))?;
    let mut bgra = vec![0u8; (stride as usize) * (height as usize)];
    let src_bpp = (bits_per_pixel / 8) as usize;
    for y in 0..height as usize {
        let src_row = pixel_offset + y * bytes_per_line as usize;
        let dst_row = y * stride as usize;
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
        stride,
        timestamp_micros,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_xwd() -> Vec<u8> {
        let mut fields = [0u32; 25];
        fields[0] = 100;
        fields[1] = 7;
        fields[2] = 2;
        fields[3] = 24;
        fields[4] = 2;
        fields[5] = 1;
        fields[7] = 0;
        fields[11] = 32;
        fields[12] = 8;
        let mut out = Vec::new();
        for f in fields {
            out.extend_from_slice(&f.to_be_bytes());
        }
        out.extend_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]);
        out
    }

    #[test]
    fn parses_32bpp_xwd_to_bgra() {
        let frame = XwdDesktopCapturer::parse_xwd(&synthetic_xwd(), 42).unwrap();
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.stride, 8);
        assert_eq!(frame.timestamp_micros, 42);
        assert_eq!(frame.bgra, vec![1, 2, 3, 1, 4, 5, 6, 1]);
    }

    #[test]
    fn rejects_short_xwd() {
        assert!(matches!(
            XwdDesktopCapturer::parse_xwd(&[0; 8], 0),
            Err(LinuxCaptureError::InvalidXwd(_))
        ));
    }
}
