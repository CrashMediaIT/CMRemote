// Source: CMRemote, clean-room implementation.

//! H.264 encoder driver backed by `ffmpeg`.
//!
//! The factory checks for `ffmpeg` at provider construction. Each
//! `encode` call feeds one BGRA frame to a short-lived ffmpeg process
//! and captures Annex-B H.264 bytes from stdout. This is deliberately
//! conservative and stateless: it is slower than a persistent VAAPI or
//! libx264 session, but it is safe, testable, and keeps the trait
//! contract complete until a hardware pipeline is selected by ADR.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use cmremote_platform::desktop::{
    CapturedFrame, DesktopMediaError, EncodedVideoChunk, EncoderFactory, VideoEncoder,
};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Errors surfaced by the ffmpeg-backed Linux encoder.
#[derive(Debug, Error)]
pub enum LinuxEncoderError {
    /// The `ffmpeg` executable is not available on PATH.
    #[error("ffmpeg is not available on PATH")]
    MissingFfmpeg,
    /// The frame shape is not acceptable to ffmpeg.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// ffmpeg failed to encode the frame.
    #[error("ffmpeg encode failed: {0}")]
    Process(String),
}

impl From<LinuxEncoderError> for DesktopMediaError {
    fn from(value: LinuxEncoderError) -> Self {
        match value {
            LinuxEncoderError::MissingFfmpeg => {
                DesktopMediaError::NotSupported(cmremote_platform::HostOs::Linux)
            }
            LinuxEncoderError::InvalidFrame(e) => DesktopMediaError::InvalidParameters(e),
            LinuxEncoderError::Process(e) => DesktopMediaError::Io(e),
        }
    }
}

/// Builds ffmpeg-backed H.264 encoders.
#[derive(Debug, Default)]
pub struct FfmpegH264EncoderFactory;

impl FfmpegH264EncoderFactory {
    /// Construct a factory after verifying `ffmpeg` exists.
    pub fn new() -> Result<Self, LinuxEncoderError> {
        if !crate::command_exists("ffmpeg") {
            return Err(LinuxEncoderError::MissingFfmpeg);
        }
        Ok(Self)
    }
}

impl EncoderFactory for FfmpegH264EncoderFactory {
    fn build(&self) -> Result<Arc<dyn VideoEncoder>, DesktopMediaError> {
        Ok(Arc::new(FfmpegH264Encoder::default()))
    }
}

/// Stateless per-session H.264 encoder backed by ffmpeg.
#[derive(Debug, Default)]
pub struct FfmpegH264Encoder {
    keyframe_requested: AtomicBool,
}

#[async_trait]
impl VideoEncoder for FfmpegH264Encoder {
    async fn encode(&self, frame: &CapturedFrame) -> Result<EncodedVideoChunk, DesktopMediaError> {
        validate_frame(frame).map_err(DesktopMediaError::from)?;
        let force_keyframe = self.keyframe_requested.swap(false, Ordering::SeqCst);
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
            &format!("{}x{}", frame.width, frame.height),
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
        ]);
        if force_keyframe {
            cmd.args(["-force_key_frames", "expr:gte(t,0)"]);
        }
        cmd.args(["-f", "h264", "pipe:1"]);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| DesktopMediaError::Io(format!("ffmpeg spawn failed: {e}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| DesktopMediaError::Io("ffmpeg stdin unavailable".into()))?;
        stdin
            .write_all(&frame.bgra)
            .await
            .map_err(|e| DesktopMediaError::Io(format!("ffmpeg stdin write failed: {e}")))?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| DesktopMediaError::Io(format!("ffmpeg wait failed: {e}")))?;
        if !output.status.success() || output.stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopMediaError::Io(format!(
                "ffmpeg exited {:?}: {}",
                output.status.code(),
                stderr.trim()
            )));
        }
        Ok(EncodedVideoChunk {
            bytes: output.stdout,
            timestamp_micros: frame.timestamp_micros,
            is_keyframe: true,
        })
    }

    fn request_keyframe(&self) {
        self.keyframe_requested.store(true, Ordering::SeqCst);
    }
}

fn validate_frame(frame: &CapturedFrame) -> Result<(), LinuxEncoderError> {
    if frame.width == 0 || frame.height == 0 {
        return Err(LinuxEncoderError::InvalidFrame("zero-sized frame".into()));
    }
    let expected_stride = frame
        .width
        .checked_mul(4)
        .ok_or_else(|| LinuxEncoderError::InvalidFrame("stride overflow".into()))?;
    if frame.stride != expected_stride {
        return Err(LinuxEncoderError::InvalidFrame(
            "only tightly-packed BGRA frames are supported".into(),
        ));
    }
    let expected_len = (frame.stride as usize)
        .checked_mul(frame.height as usize)
        .ok_or_else(|| LinuxEncoderError::InvalidFrame("frame size overflow".into()))?;
    if frame.bgra.len() != expected_len {
        return Err(LinuxEncoderError::InvalidFrame(
            "BGRA buffer length mismatch".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_frame_accepts_tightly_packed_bgra() {
        validate_frame(&CapturedFrame::black(2, 2).unwrap()).unwrap();
    }

    #[test]
    fn validate_frame_rejects_stride_mismatch() {
        let mut f = CapturedFrame::black(2, 2).unwrap();
        f.stride = 16;
        assert!(matches!(
            validate_frame(&f),
            Err(LinuxEncoderError::InvalidFrame(_))
        ));
    }
}
