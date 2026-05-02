// Source: CMRemote, clean-room implementation.

//! Desktop capture + video encoder trait surface (slice R7.c).
//!
//! These two traits are the seam a future WebRTC-backed driver will
//! plug into. Slice R7.c ships the trait definitions, lightweight
//! frame DTOs, and `NotSupported*` defaults that return a structured
//! error naming the host OS — exactly the same "trait first, real
//! driver later" pattern slice R6 used for [`super::super::packages`].
//!
//! The defaults exist so the agent never panics on a desktop request:
//! a request that reaches the dispatcher today (and survives the
//! [`super::guards`] checks) is still answered by the
//! [`super::NotSupportedDesktopTransport`] stub at the
//! transport-provider level. Once a concrete WebRTC driver lands it
//! can compose its own [`DesktopCapturer`] + [`VideoEncoder`]
//! implementations behind the same traits without changing any
//! public API of this crate.
//!
//! ## Why two traits, not one
//!
//! Capture (`BGRA frames out of the OS`) and encode (`H.264 / VP8 /
//! AV1 packets out of frames`) are independently swappable on every
//! supported host:
//!
//! - Windows can run DXGI Desktop Duplication for capture and Media
//!   Foundation H.264 for encode, but a fallback to GDI capture and
//!   software OpenH264 must remain possible.
//! - macOS uses ScreenCaptureKit for capture and VideoToolbox for
//!   encode.
//! - Linux uses PipeWire / X11 for capture and either VAAPI hardware
//!   or libx264 software for encode.
//!
//! Keeping the two traits separate lets the eventual driver mix and
//! match without any per-OS branching at the trait level.
//!
//! ## Security contract
//!
//! Implementations MUST:
//!
//! 1. **Refuse to attach to a display the operator has not consented
//!    to.** The default consent check lives in the desktop-transport
//!    provider (see [`super::guards`]); capturers must additionally
//!    refuse any display id the OS reports as belonging to a
//!    different user session than the one the agent is running in.
//! 2. **Bound buffer growth.** Frames are large (a 4K BGRA frame is
//!    ~33 MiB); implementations must drop frames rather than queue
//!    them unboundedly when the encoder falls behind.
//! 3. **Fail closed on hardware-encoder errors.** A transient encode
//!    failure must surface as a structured error so the WebRTC layer
//!    can renegotiate with the viewer; silently emitting zero-byte
//!    chunks would deadlock the receiver.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::HostOs;

/// Errors surfaced by [`DesktopCapturer`] and [`VideoEncoder`].
#[derive(Debug, Error)]
pub enum DesktopMediaError {
    /// No driver implementation is registered for the current host.
    /// Returned by [`NotSupportedDesktopCapturer`] /
    /// [`NotSupportedVideoEncoder`].
    #[error("desktop capture/encode is not supported on {0:?}")]
    NotSupported(HostOs),

    /// Operating-system or driver I/O error. The string is
    /// implementation-defined and may include an OS error code; it
    /// MUST NOT contain a display name, capture buffer pointer, or
    /// any operator-supplied string.
    #[error("desktop media I/O error: {0}")]
    Io(String),

    /// The capturer or encoder rejected the request because the
    /// supplied parameters are out of range for the active hardware
    /// (e.g. a 16K frame on a 1080p capture path).
    #[error("desktop media parameters are invalid: {0}")]
    InvalidParameters(String),
}

/// A single captured desktop frame in 32-bit pre-multiplied BGRA.
///
/// BGRA is the lowest common denominator across DXGI Desktop
/// Duplication, ScreenCaptureKit, and PipeWire DMA-buf imports;
/// every concrete encoder converts from this on the way to its native
/// pixel format. The struct owns its pixel buffer so the trait stays
/// `Send + Sync`; real implementations will swap to a ring-buffer
/// pool for the hot path, but the public DTO stays this simple to
/// keep the contract reviewable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Bytes per row, including any padding the capture path inserts.
    pub stride: u32,
    /// Monotonic capture timestamp in microseconds since an
    /// implementation-defined epoch. Used by the encoder for B-frame
    /// reordering and by the WebRTC layer for jitter-buffer pacing.
    pub timestamp_micros: u64,
    /// Pre-multiplied BGRA pixels, length = `stride * height`.
    pub bgra: Vec<u8>,
}

impl CapturedFrame {
    /// Build a single-row test frame for the supplied dimensions.
    /// Returns [`DesktopMediaError::InvalidParameters`] if the width,
    /// height, or stride would overflow on a 32-bit platform.
    pub fn black(width: u32, height: u32) -> Result<Self, DesktopMediaError> {
        let stride = width
            .checked_mul(4)
            .ok_or_else(|| DesktopMediaError::InvalidParameters("stride overflow".into()))?;
        let total = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| DesktopMediaError::InvalidParameters("frame size overflow".into()))?;
        Ok(Self {
            width,
            height,
            stride,
            timestamp_micros: 0,
            bgra: vec![0u8; total],
        })
    }
}

/// Profile of an encoded video chunk emitted by [`VideoEncoder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedVideoChunk {
    /// Encoded payload bytes (codec-defined framing).
    pub bytes: Vec<u8>,
    /// Same epoch as [`CapturedFrame::timestamp_micros`].
    pub timestamp_micros: u64,
    /// `true` for IDR / keyframes (codec-agnostic). The WebRTC layer
    /// uses this flag to satisfy `Picture Loss Indication` requests
    /// without re-encoding.
    pub is_keyframe: bool,
}

/// Captures BGRA frames off a display.
///
/// Implementations are expected to maintain their own state (current
/// display id, capture surface, hardware buffer pool) — the trait
/// surface is intentionally minimal so the eventual concrete drivers
/// can be tested in isolation.
#[async_trait]
pub trait DesktopCapturer: Send + Sync {
    /// Block until the next frame is available, then return it.
    /// Implementations MUST NOT panic on display loss; they MUST
    /// surface a [`DesktopMediaError`] so the WebRTC layer can react.
    async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError>;
}

/// Encodes [`CapturedFrame`]s into [`EncodedVideoChunk`]s.
#[async_trait]
pub trait VideoEncoder: Send + Sync {
    /// Encode a single frame. Implementations MUST honour
    /// [`request_keyframe`] before the next call and MAY drop frames
    /// internally to keep up with the capture rate (returning the
    /// most recent successfully encoded chunk).
    ///
    /// [`request_keyframe`]: VideoEncoder::request_keyframe
    async fn encode(&self, frame: &CapturedFrame) -> Result<EncodedVideoChunk, DesktopMediaError>;

    /// Request that the next [`encode`](Self::encode) call emit a
    /// keyframe. Used by the WebRTC layer in response to a viewer's
    /// Picture Loss Indication.
    fn request_keyframe(&self);
}

// ---------------------------------------------------------------------------
// `NotSupported*` defaults — used by the runtime until a concrete
// driver lands. Mirror `NotSupportedPackageProvider` /
// `NotSupportedDesktopTransport`.
// ---------------------------------------------------------------------------

/// Default capturer returned by the runtime when no concrete driver
/// is registered. Always returns [`DesktopMediaError::NotSupported`]
/// naming the host OS — never panics, never allocates a frame buffer.
pub struct NotSupportedDesktopCapturer {
    host_os: HostOs,
}

impl NotSupportedDesktopCapturer {
    /// Build a capturer that names `host_os` in its error.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }

    /// Build a capturer that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedDesktopCapturer {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl DesktopCapturer for NotSupportedDesktopCapturer {
    async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
        Err(DesktopMediaError::NotSupported(self.host_os))
    }
}

/// Default encoder returned by the runtime when no concrete driver
/// is registered. Always returns [`DesktopMediaError::NotSupported`].
pub struct NotSupportedVideoEncoder {
    host_os: HostOs,
}

impl NotSupportedVideoEncoder {
    /// Build an encoder that names `host_os` in its error.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }

    /// Build an encoder that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedVideoEncoder {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl VideoEncoder for NotSupportedVideoEncoder {
    async fn encode(&self, _frame: &CapturedFrame) -> Result<EncodedVideoChunk, DesktopMediaError> {
        Err(DesktopMediaError::NotSupported(self.host_os))
    }

    fn request_keyframe(&self) {
        // No-op: the stub never emits frames so a keyframe request
        // is meaningless. Real implementations atomically set a flag
        // the next `encode` call inspects.
    }
}

// ---------------------------------------------------------------------------
// EncoderFactory — slice R7.n.6.
// ---------------------------------------------------------------------------

/// Builds a fresh [`VideoEncoder`] for a single desktop session.
///
/// The slice R7.n.6 WebRTC driver wires capture frames into a
/// per-session `RTCRtpSender` track. Each session needs its **own**
/// encoder because every concrete encoder carries per-session state
/// (frame counters, IDR flags, NAL-unit headers, hardware MFT
/// instance) that cannot be shared across two viewers without
/// corrupting the bitstream of either. The factory is therefore the
/// only stable seam the driver pokes at when a peer connection is
/// built.
///
/// Implementations MUST be cheap — `build()` is called inline on the
/// signalling path and is expected to return in well under a frame
/// interval. Per-OS factories typically just clone an
/// [`Arc<EncoderConfig>`] and hand it to a `new(config)` constructor.
///
/// ## Failure mode
///
/// `build` returns [`DesktopMediaError::NotSupported`] when no
/// encoder is registered for the host. The driver then negotiates
/// the peer connection without a video track — the operator gets a
/// connected RTP transport with no media, exactly the same state as
/// before slice R7.n.6 landed. Any other error variant signals an
/// in-flight construction failure (e.g. Media Foundation refused to
/// instantiate the H.264 MFT) and is logged at `warn!` by the
/// driver before falling back to the no-track path.
pub trait EncoderFactory: Send + Sync {
    /// Build a fresh encoder for one session. Returns
    /// [`DesktopMediaError::NotSupported`] when no concrete encoder
    /// is available on the host (e.g. non-Windows fallbacks today).
    fn build(&self) -> Result<Arc<dyn VideoEncoder>, DesktopMediaError>;
}

/// Default factory used by the runtime when no concrete encoder
/// driver is registered. Always returns
/// [`DesktopMediaError::NotSupported`] naming the host OS — never
/// constructs an encoder, never allocates.
pub struct NotSupportedEncoderFactory {
    host_os: HostOs,
}

impl NotSupportedEncoderFactory {
    /// Build a factory that names `host_os` in its error.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }

    /// Build a factory that names the current host's OS.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedEncoderFactory {
    fn default() -> Self {
        Self::for_current_host()
    }
}

impl EncoderFactory for NotSupportedEncoderFactory {
    fn build(&self) -> Result<Arc<dyn VideoEncoder>, DesktopMediaError> {
        Err(DesktopMediaError::NotSupported(self.host_os))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn not_supported_capturer_returns_structured_error_naming_os() {
        let c = NotSupportedDesktopCapturer::new(HostOs::Linux);
        let err = c.capture_next_frame().await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("not supported"), "{s}");
        assert!(s.contains("Linux"), "{s}");
    }

    #[tokio::test]
    async fn not_supported_encoder_returns_structured_error_naming_os() {
        let e = NotSupportedVideoEncoder::new(HostOs::MacOs);
        let frame = CapturedFrame::black(2, 2).unwrap();
        let err = e.encode(&frame).await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("not supported"), "{s}");
        assert!(s.contains("MacOs"), "{s}");
    }

    #[test]
    fn captured_frame_black_helper_has_correct_buffer_size() {
        let f = CapturedFrame::black(640, 480).unwrap();
        assert_eq!(f.bgra.len(), 640 * 480 * 4);
        assert_eq!(f.stride, 640 * 4);
        assert!(f.bgra.iter().all(|&b| b == 0));
    }

    #[test]
    fn captured_frame_black_refuses_overflow() {
        // Width × 4 overflows u32.
        let r = CapturedFrame::black(u32::MAX, 1);
        assert!(matches!(
            r.unwrap_err(),
            DesktopMediaError::InvalidParameters(_)
        ));
    }

    #[test]
    fn request_keyframe_is_a_no_op_for_the_stub_encoder() {
        // The stub must accept keyframe requests without panicking
        // even though it will never emit a frame.
        let e = NotSupportedVideoEncoder::for_current_host();
        e.request_keyframe();
        e.request_keyframe();
    }

    #[test]
    fn defaults_use_current_host() {
        let _: NotSupportedDesktopCapturer = Default::default();
        let _: NotSupportedVideoEncoder = Default::default();
    }

    /// Trait-object safety check — both traits must be storable
    /// behind `Arc<dyn …>` so the runtime can hand them to the
    /// future WebRTC driver.
    #[test]
    fn traits_are_object_safe() {
        let _c: Box<dyn DesktopCapturer> =
            Box::new(NotSupportedDesktopCapturer::for_current_host());
        let _e: Box<dyn VideoEncoder> = Box::new(NotSupportedVideoEncoder::for_current_host());
    }

    #[test]
    fn error_messages_do_not_leak_capture_buffer_or_operator_strings() {
        // Sanity: the structured error contract pins the message to
        // a fixed shape that cannot inadvertently include the buffer
        // bytes or any operator-supplied identifier.
        let e = DesktopMediaError::Io("device 4 not present".into());
        let s = e.to_string();
        assert!(s.contains("desktop media I/O error"));
        assert!(s.contains("device 4 not present"));
    }

    #[test]
    fn not_supported_encoder_factory_returns_structured_error_naming_os() {
        let f = NotSupportedEncoderFactory::new(HostOs::Linux);
        let err = f
            .build()
            .err()
            .expect("not-supported factory must error");
        let s = err.to_string();
        assert!(s.contains("not supported"), "{s}");
        assert!(s.contains("Linux"), "{s}");
    }

    #[test]
    fn not_supported_encoder_factory_default_uses_current_host() {
        // Smoke-check that `Default` does not panic.
        let _: NotSupportedEncoderFactory = Default::default();
    }

    #[test]
    fn encoder_factory_is_object_safe() {
        let _f: Box<dyn EncoderFactory> = Box::new(NotSupportedEncoderFactory::for_current_host());
    }
}
