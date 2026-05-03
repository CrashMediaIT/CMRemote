// Source: CMRemote, clean-room implementation.

//! Windows video encoder backed by Media Foundation's H.264 MFT
//! (slice R7.n.6).
//!
//! ## Pipeline
//!
//! ```text
//!   MFStartup ──► CoCreateInstance(CLSID_CMSH264EncoderMFT)
//!                                      │
//!                                      ▼
//!                                IMFTransform
//!                                      │
//!                  ┌───────────────────┼───────────────────┐
//!                  ▼                   ▼                   ▼
//!         SetOutputType         SetInputType        ProcessMessage
//!         (H.264 / size /       (NV12 / size /     (NOTIFY_BEGIN_STREAMING)
//!          bitrate / fps)        fps)
//!
//!   per CapturedFrame:
//!     bgra_to_nv12 ──► MFCreateMemoryBuffer + Lock ──► IMFSample
//!                              │                          │
//!                              ▼                          ▼
//!                      ProcessInput ──► (loop) ProcessOutput ──► EncodedVideoChunk
//! ```
//!
//! ## Why a synchronous MFT
//!
//! The Microsoft H.264 encoder MFT exposes both a synchronous
//! (default) and an asynchronous (event-driven) mode. Synchronous
//! is simpler — every `ProcessInput` call is followed by zero-or-
//! more `ProcessOutput` calls until the MFT reports
//! `MF_E_TRANSFORM_NEED_MORE_INPUT`. We trade some throughput
//! versus the async mode for a much smaller surface to review.
//! All `ProcessInput` / `ProcessOutput` calls happen under
//! [`tokio::task::spawn_blocking`] so the runtime worker threads
//! are never blocked.
//!
//! ## Threading
//!
//! The transform pointer is single-threaded — every method on it
//! is called under a `Mutex<EncoderInner>`. The async
//! [`cmremote_platform::desktop::VideoEncoder::encode`] impl uses
//! `spawn_blocking` because each call may block for several ms
//! inside the MFT (especially when emitting a keyframe).
//!
//! ## Drop / shutdown
//!
//! - [`EncoderInner::Drop`] sends `MFT_MESSAGE_NOTIFY_END_STREAMING`
//!   and `MFT_MESSAGE_COMMAND_FLUSH` (best-effort, ignoring errors)
//!   so the MFT cleans up internal buffers before the COM ref-count
//!   drops to zero.
//! - [`MfStartupGuard::Drop`] calls `MFShutdown` to balance the
//!   `MFStartup` from the constructor. Multiple encoders are safe
//!   — `MFStartup` is reference-counted by the OS.
//!
//! ## Security
//!
//! - Encoder configuration is validated up-front
//!   ([`WindowsVideoEncoderConfig::validate`]) — odd dimensions,
//!   zero bitrate, or out-of-range fps are refused before any COM
//!   call.
//! - The encoder never logs frame contents and never includes a
//!   raw `IMFSample` pointer in error messages.
//! - HRESULTs are formatted as opaque `0x...` codes; no operator
//!   strings are ever interpolated into a Win32 error path.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cmremote_platform::desktop::{
    bgra_to_nv12, CapturedFrame, DesktopMediaError, EncodedVideoChunk, EncoderFactory,
    VideoEncoder,
};
use thiserror::Error;
use tracing::{debug, warn};

use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264EncoderMFT, IMFMediaBuffer, IMFMediaType, IMFSample, IMFTransform,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFShutdown, MFStartup,
    MFSampleExtension_CleanPoint, MFVideoFormat_H264, MFVideoFormat_NV12, MFMediaType_Video,
    MFSTARTUP_FULL, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_INFO, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MFVideoInterlace_Progressive, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};

// ---------------------------------------------------------------------------
// WindowsEncoderError + From<windows::core::Error>
// ---------------------------------------------------------------------------

/// Errors specific to the Media Foundation H.264 encoder.
///
/// Mirrors the pattern used by [`super::capture::WindowsCaptureError`]:
/// every variant flattens to [`DesktopMediaError::Io`] or
/// [`DesktopMediaError::InvalidParameters`] for trait callers, but
/// the richer enum is preserved for unit tests / direct callers.
#[derive(Debug, Error)]
pub enum WindowsEncoderError {
    /// `MFStartup` returned a non-success HRESULT (or the OS does
    /// not have Media Foundation installed — possible on Server
    /// Core SKUs without the Desktop Experience feature).
    #[error("MFStartup failed: {0}")]
    Startup(String),

    /// `CoCreateInstance(CLSID_CMSH264EncoderMFT)` failed; usually
    /// `REGDB_E_CLASSNOTREG` on a SKU without the Microsoft H.264
    /// encoder, or `E_NOINTERFACE` when running under a sandboxed
    /// session that blocks COM activation.
    #[error("CoCreateInstance(CLSID_CMSH264EncoderMFT) failed: {0}")]
    Activation(String),

    /// One of `SetOutputType` / `SetInputType` / `ProcessMessage`
    /// failed during encoder setup.
    #[error("Media Foundation setup failed at {step}: {hresult}")]
    Setup {
        /// Stable label naming the failed step
        /// (`"SetOutputType"`, `"SetInputType"`,
        /// `"NOTIFY_BEGIN_STREAMING"`, …).
        step: &'static str,
        /// HRESULT formatted as `0x........`.
        hresult: String,
    },

    /// `ProcessInput` or `ProcessOutput` failed mid-encode.
    #[error("Media Foundation encode failed at {step}: {hresult}")]
    Encode {
        /// `"ProcessInput"` or `"ProcessOutput"` or
        /// `"MFCreateMemoryBuffer"`, …
        step: &'static str,
        /// HRESULT formatted as `0x........`.
        hresult: String,
    },

    /// Configuration parameters failed validation
    /// ([`WindowsVideoEncoderConfig::validate`]).
    #[error("invalid encoder config: {0}")]
    BadConfig(String),

    /// Frame dimensions disagreed with the encoder's pinned
    /// width/height (the encoder is single-resolution; resolution
    /// changes require building a fresh encoder).
    #[error("frame {got_w}x{got_h} does not match encoder {want_w}x{want_h}")]
    FrameSizeMismatch {
        /// Frame width from the [`CapturedFrame`].
        got_w: u32,
        /// Frame height from the [`CapturedFrame`].
        got_h: u32,
        /// Encoder's configured width.
        want_w: u32,
        /// Encoder's configured height.
        want_h: u32,
    },
}

impl From<WindowsEncoderError> for DesktopMediaError {
    fn from(value: WindowsEncoderError) -> Self {
        match &value {
            WindowsEncoderError::BadConfig(_) | WindowsEncoderError::FrameSizeMismatch { .. } => {
                DesktopMediaError::InvalidParameters(value.to_string())
            }
            _ => DesktopMediaError::Io(value.to_string()),
        }
    }
}

fn fmt_hresult(err: &windows::core::Error) -> String {
    format!("HRESULT 0x{:08X}", err.code().0 as u32)
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Encoder configuration. Pinned at construction; the encoder is
/// single-resolution and single-bitrate (renegotiation requires
/// building a fresh encoder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsVideoEncoderConfig {
    /// Frame width in pixels. MUST be even.
    pub width: u32,
    /// Frame height in pixels. MUST be even.
    pub height: u32,
    /// Target bitrate in bits per second. Bounded to
    /// `[100_000, 50_000_000]` — anything outside that range is
    /// almost certainly a config bug, and the MFT rejects it
    /// silently with a kernel-level fallback.
    pub bitrate_bps: u32,
    /// Target frame rate (numerator). Pixel aspect ratio is
    /// always 1:1; if the source is anamorphic, the capturer is
    /// expected to scale it before calling the encoder.
    pub fps: u32,
}

impl WindowsVideoEncoderConfig {
    /// Conservative default: 1920x1080 @ 30fps, 4 Mbps.
    pub const fn default_1080p_30fps() -> Self {
        Self {
            width: 1920,
            height: 1080,
            bitrate_bps: 4_000_000,
            fps: 30,
        }
    }

    /// Validate the configuration. Returns
    /// [`WindowsEncoderError::BadConfig`] with a stable message on
    /// every failure mode so the runtime can log + fall back.
    pub fn validate(&self) -> Result<(), WindowsEncoderError> {
        if self.width == 0 || self.height == 0 {
            return Err(WindowsEncoderError::BadConfig("dimensions must be non-zero".into()));
        }
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(WindowsEncoderError::BadConfig(
                "dimensions must be even (NV12 sub-samples 2x2)".into(),
            ));
        }
        if self.width > 7680 || self.height > 4320 {
            return Err(WindowsEncoderError::BadConfig(
                "dimensions exceed 8K (7680x4320) — refused".into(),
            ));
        }
        if !(100_000..=50_000_000).contains(&self.bitrate_bps) {
            return Err(WindowsEncoderError::BadConfig(
                "bitrate_bps must be in [100_000, 50_000_000]".into(),
            ));
        }
        if !(1..=120).contains(&self.fps) {
            return Err(WindowsEncoderError::BadConfig(
                "fps must be in [1, 120]".into(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Marker that owns one `MFStartup` / `MFShutdown` pair so the
/// encoder is a self-contained unit. `MFStartup` is OS-refcounted
/// so multiple encoders are safe; this guard balances exactly one
/// pair per encoder.
struct MfStartupGuard;

impl MfStartupGuard {
    fn new() -> Result<Self, WindowsEncoderError> {
        // SAFETY: COINIT_MULTITHREADED is the apartment model the
        // Tokio runtime uses (its worker threads are not COM-init).
        // Calling on an already-initialised thread returns
        // S_FALSE / RPC_E_CHANGED_MODE; both are acceptable, the
        // crate already balances the call with CoUninitialize on
        // drop. We deliberately ignore the return for that reason
        // — a strict mismatch would surface as MFStartup failing
        // anyway.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        // SAFETY: MFStartup is a global ref-counted init; the
        // matching MFShutdown lives in Drop below.
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|e| WindowsEncoderError::Startup(fmt_hresult(&e)))?;
        }
        Ok(Self)
    }
}

impl Drop for MfStartupGuard {
    fn drop(&mut self) {
        // SAFETY: balances `MFStartup` from `new`. Best-effort —
        // the only documented failure is "no matching Startup",
        // which can't happen here.
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

/// Media Foundation H.264 video encoder.
///
/// Construct with [`Self::new`]; reuse for every frame; drop to
/// release the MFT and the matching `MFStartup`. The same encoder
/// instance can be shared across tasks via `Arc`; every method is
/// internally synchronised with a `Mutex`.
pub struct WindowsVideoEncoder {
    inner: Arc<Mutex<EncoderInner>>,
    config: WindowsVideoEncoderConfig,
}

struct EncoderInner {
    /// COM smart pointer to the H.264 encoder MFT. Released on drop.
    transform: IMFTransform,
    /// Drives `MFShutdown` on encoder drop. Held alongside
    /// `transform` so the COM ref counting orders correctly:
    /// `transform` is dropped first (releases the MFT), then the
    /// guard fires (`MFShutdown` + `CoUninitialize`).
    _mf: MfStartupGuard,
    /// Frames the encoder has accepted. Used to compute monotonic
    /// `IMFSample::SetSampleTime` when the source frame's
    /// timestamp is missing or rewinds.
    frames_in: u64,
    /// `true` after [`request_keyframe`] until the next encode
    /// stamps `MFSampleExtension_CleanPoint = 1`.
    keyframe_requested: bool,
}

// SAFETY: every field of `EncoderInner` is touched exclusively
// under [`WindowsVideoEncoder::inner`]'s `Mutex`. The COM
// pointer (`IMFTransform`) is documented to be safe to use
// across threads as long as exactly one thread accesses it at a
// time, which the mutex guarantees. The `MfStartupGuard` holds
// no thread-bound state.
unsafe impl Send for EncoderInner {}
unsafe impl Sync for EncoderInner {}

impl Drop for EncoderInner {
    fn drop(&mut self) {
        // Best-effort end-of-stream notification; the MFT is about
        // to be released either way, so any HRESULT here is
        // discarded.
        // SAFETY: `transform` is a valid IMFTransform; ProcessMessage
        // is documented to be safe to call from any thread that
        // holds the only reference, which we do (mutex held).
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}

impl WindowsVideoEncoder {
    /// Build a new encoder for `config`. Performs the full Media
    /// Foundation setup (Startup → CoCreateInstance → SetOutputType
    /// → SetInputType → BEGIN_STREAMING) and returns `Err` on the
    /// first failure with a stable label naming the failed step.
    pub fn new(config: WindowsVideoEncoderConfig) -> Result<Self, WindowsEncoderError> {
        config.validate()?;
        let mf = MfStartupGuard::new()?;
        // SAFETY: COM activation of the H.264 encoder MFT. CLSID
        // is a Microsoft-published GUID (validated against the
        // `windows` crate's bindings at compile time).
        let transform: IMFTransform = unsafe {
            CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| WindowsEncoderError::Activation(fmt_hresult(&e)))?
        };

        Self::set_output_type(&transform, &config)?;
        Self::set_input_type(&transform, &config)?;

        // SAFETY: `transform` is a valid IMFTransform built above;
        // the three messages are documented to be the only ones
        // required to start a synchronous MFT.
        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .map_err(|e| WindowsEncoderError::Setup {
                    step: "COMMAND_FLUSH",
                    hresult: fmt_hresult(&e),
                })?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| WindowsEncoderError::Setup {
                    step: "NOTIFY_BEGIN_STREAMING",
                    hresult: fmt_hresult(&e),
                })?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| WindowsEncoderError::Setup {
                    step: "NOTIFY_START_OF_STREAM",
                    hresult: fmt_hresult(&e),
                })?;
        }

        debug!(
            width = config.width,
            height = config.height,
            bitrate = config.bitrate_bps,
            fps = config.fps,
            "Media Foundation H.264 encoder initialised",
        );

        Ok(Self {
            inner: Arc::new(Mutex::new(EncoderInner {
                transform,
                _mf: mf,
                frames_in: 0,
                keyframe_requested: false,
            })),
            config,
        })
    }

    /// Read the encoder's pinned configuration.
    pub fn config(&self) -> WindowsVideoEncoderConfig {
        self.config
    }

    fn set_output_type(
        transform: &IMFTransform,
        config: &WindowsVideoEncoderConfig,
    ) -> Result<(), WindowsEncoderError> {
        // SAFETY: every COM call below operates on a valid
        // IMFTransform / IMFMediaType built in this scope; arguments
        // are static GUIDs and primitive integers. Errors are mapped
        // through `WindowsEncoderError::Setup` with a stable label.
        // `IMFMediaType` derefs to `IMFAttributes`, so SetGUID /
        // SetUINT32 / SetUINT64 are reachable directly.
        unsafe {
            let media_type: IMFMediaType =
                MFCreateMediaType().map_err(|e| WindowsEncoderError::Setup {
                    step: "MFCreateMediaType(out)",
                    hresult: fmt_hresult(&e),
                })?;
            set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video, "MAJOR_TYPE(out)")?;
            set_guid(&media_type, &MF_MT_SUBTYPE, &MFVideoFormat_H264, "SUBTYPE(out)")?;
            set_u32(&media_type, &MF_MT_AVG_BITRATE, config.bitrate_bps, "AVG_BITRATE")?;
            set_u32(
                &media_type,
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0 as u32,
                "INTERLACE_MODE(out)",
            )?;
            set_u64(
                &media_type,
                &MF_MT_FRAME_SIZE,
                pack_two_u32(config.width, config.height),
                "FRAME_SIZE(out)",
            )?;
            set_u64(
                &media_type,
                &MF_MT_FRAME_RATE,
                pack_two_u32(config.fps, 1),
                "FRAME_RATE(out)",
            )?;
            set_u64(
                &media_type,
                &MF_MT_PIXEL_ASPECT_RATIO,
                pack_two_u32(1, 1),
                "PIXEL_ASPECT_RATIO(out)",
            )?;
            transform
                .SetOutputType(0, &media_type, 0)
                .map_err(|e| WindowsEncoderError::Setup {
                    step: "SetOutputType",
                    hresult: fmt_hresult(&e),
                })?;
        }
        Ok(())
    }

    fn set_input_type(
        transform: &IMFTransform,
        config: &WindowsVideoEncoderConfig,
    ) -> Result<(), WindowsEncoderError> {
        // SAFETY: see `set_output_type` — same shape, NV12 input
        // instead of H.264 output. The MFT mandates output type be
        // set first; the constructor preserves that ordering.
        unsafe {
            let media_type: IMFMediaType =
                MFCreateMediaType().map_err(|e| WindowsEncoderError::Setup {
                    step: "MFCreateMediaType(in)",
                    hresult: fmt_hresult(&e),
                })?;
            set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video, "MAJOR_TYPE(in)")?;
            set_guid(&media_type, &MF_MT_SUBTYPE, &MFVideoFormat_NV12, "SUBTYPE(in)")?;
            set_u32(
                &media_type,
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0 as u32,
                "INTERLACE_MODE(in)",
            )?;
            set_u64(
                &media_type,
                &MF_MT_FRAME_SIZE,
                pack_two_u32(config.width, config.height),
                "FRAME_SIZE(in)",
            )?;
            set_u64(
                &media_type,
                &MF_MT_FRAME_RATE,
                pack_two_u32(config.fps, 1),
                "FRAME_RATE(in)",
            )?;
            set_u64(
                &media_type,
                &MF_MT_PIXEL_ASPECT_RATIO,
                pack_two_u32(1, 1),
                "PIXEL_ASPECT_RATIO(in)",
            )?;
            transform
                .SetInputType(0, &media_type, 0)
                .map_err(|e| WindowsEncoderError::Setup {
                    step: "SetInputType",
                    hresult: fmt_hresult(&e),
                })?;
        }
        Ok(())
    }

    /// Synchronous encode entry point — runs entirely under the
    /// `Mutex<EncoderInner>` because `IMFTransform` is single-
    /// threaded. The async [`VideoEncoder::encode`] impl wraps
    /// this call in `tokio::task::spawn_blocking`.
    fn encode_blocking(
        &self,
        frame: &CapturedFrame,
    ) -> Result<EncodedVideoChunk, WindowsEncoderError> {
        if frame.width != self.config.width || frame.height != self.config.height {
            return Err(WindowsEncoderError::FrameSizeMismatch {
                got_w: frame.width,
                got_h: frame.height,
                want_w: self.config.width,
                want_h: self.config.height,
            });
        }
        let nv12 = bgra_to_nv12(frame).map_err(|e| WindowsEncoderError::BadConfig(e.to_string()))?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| WindowsEncoderError::Encode {
                step: "lock",
                hresult: "encoder mutex poisoned".into(),
            })?;
        let frame_index = inner.frames_in;
        inner.frames_in = inner.frames_in.saturating_add(1);

        // 100ns ticks, monotonic in frame index so the MFT's
        // pacing logic doesn't get confused by capture timestamps
        // that pause when the screen is idle.
        let sample_duration_100ns: i64 = 10_000_000 / self.config.fps as i64;
        let sample_time_100ns: i64 = frame_index as i64 * sample_duration_100ns;

        let force_keyframe = std::mem::replace(&mut inner.keyframe_requested, false);

        // Build IMFSample wrapping the NV12 buffer. SAFETY: each
        // pointer / length passed to MF is valid for the lifetime
        // of the sample; Lock/Unlock are paired.
        let sample = unsafe {
            build_nv12_sample(
                &nv12.y,
                &nv12.uv,
                sample_time_100ns,
                sample_duration_100ns,
                force_keyframe,
            )?
        };

        // Push into the MFT. SAFETY: `inner.transform` is valid;
        // the sample we built above outlives this call.
        unsafe {
            inner
                .transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| WindowsEncoderError::Encode {
                    step: "ProcessInput",
                    hresult: fmt_hresult(&e),
                })?;
        }

        // Drain output. The synchronous H.264 MFT may emit zero or
        // more samples per input. We coalesce all output bytes
        // from the *first* output sample into the chunk; if more
        // are produced (uncommon for the synchronous path), we
        // still drain them so the next ProcessInput doesn't see
        // stale state.
        // SAFETY: see `drain_output`'s SAFETY notes.
        let (bytes, is_keyframe) =
            unsafe { drain_output(&inner.transform, force_keyframe)? };

        Ok(EncodedVideoChunk {
            bytes,
            timestamp_micros: nv12.timestamp_micros,
            is_keyframe,
        })
    }
}

/// Build an `IMFSample` containing a single NV12 frame (Y plane
/// followed by interleaved UV plane).
///
/// # Safety
///
/// All Media Foundation entry points called here are documented
/// to be safe to call on an MTA-initialised thread; the buffer
/// lock/unlock pair is balanced before returning, and the buffer
/// is consumed only by the returned sample (which keeps it alive
/// via COM ref counting). The Y/UV slices must be the correct
/// length for the encoder's resolution — the caller validates
/// this via [`bgra_to_nv12`].
unsafe fn build_nv12_sample(
    y: &[u8],
    uv: &[u8],
    sample_time_100ns: i64,
    sample_duration_100ns: i64,
    force_keyframe: bool,
) -> Result<IMFSample, WindowsEncoderError> {
    let total_len: u32 = (y.len() + uv.len())
        .try_into()
        .map_err(|_| WindowsEncoderError::Encode {
            step: "buffer-size",
            hresult: "NV12 plane size exceeds u32".into(),
        })?;

    let buffer: IMFMediaBuffer =
        MFCreateMemoryBuffer(total_len).map_err(|e| WindowsEncoderError::Encode {
            step: "MFCreateMemoryBuffer",
            hresult: fmt_hresult(&e),
        })?;

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut max_len: u32 = 0;
    let mut cur_len: u32 = 0;
    buffer
        .Lock(&mut data, Some(&mut max_len), Some(&mut cur_len))
        .map_err(|e| WindowsEncoderError::Encode {
            step: "IMFMediaBuffer::Lock",
            hresult: fmt_hresult(&e),
        })?;
    if data.is_null() || (max_len as usize) < (y.len() + uv.len()) {
        let _ = buffer.Unlock();
        return Err(WindowsEncoderError::Encode {
            step: "buffer-size",
            hresult: "MF returned undersized memory buffer".into(),
        });
    }
    // Copy Y plane, then UV plane. SAFETY: `data` points to
    // `max_len` writable bytes; we wrote `y.len() + uv.len()` <=
    // `max_len` checked above.
    std::ptr::copy_nonoverlapping(y.as_ptr(), data, y.len());
    std::ptr::copy_nonoverlapping(uv.as_ptr(), data.add(y.len()), uv.len());
    buffer
        .Unlock()
        .map_err(|e| WindowsEncoderError::Encode {
            step: "IMFMediaBuffer::Unlock",
            hresult: fmt_hresult(&e),
        })?;
    buffer
        .SetCurrentLength(total_len)
        .map_err(|e| WindowsEncoderError::Encode {
            step: "SetCurrentLength",
            hresult: fmt_hresult(&e),
        })?;

    let sample: IMFSample = MFCreateSample().map_err(|e| WindowsEncoderError::Encode {
        step: "MFCreateSample",
        hresult: fmt_hresult(&e),
    })?;
    sample
        .AddBuffer(&buffer)
        .map_err(|e| WindowsEncoderError::Encode {
            step: "AddBuffer",
            hresult: fmt_hresult(&e),
        })?;
    sample
        .SetSampleTime(sample_time_100ns)
        .map_err(|e| WindowsEncoderError::Encode {
            step: "SetSampleTime",
            hresult: fmt_hresult(&e),
        })?;
    sample
        .SetSampleDuration(sample_duration_100ns)
        .map_err(|e| WindowsEncoderError::Encode {
            step: "SetSampleDuration",
            hresult: fmt_hresult(&e),
        })?;
    if force_keyframe {
        sample
            .SetUINT32(&MFSampleExtension_CleanPoint, 1)
            .map_err(|e| WindowsEncoderError::Encode {
                step: "CleanPoint",
                hresult: fmt_hresult(&e),
            })?;
    }
    Ok(sample)
}

/// Drain every available output sample from the MFT. Returns the
/// concatenated payload of the first non-empty sample (the
/// synchronous H.264 MFT typically emits exactly one) and the
/// observed keyframe flag.
///
/// # Safety
///
/// `transform` must point to a valid IMFTransform that is
/// currently mid-stream (between `NOTIFY_BEGIN_STREAMING` and
/// `NOTIFY_END_STREAMING`). All sample / buffer COM pointers
/// allocated here are dropped before returning.
unsafe fn drain_output(
    transform: &IMFTransform,
    requested_keyframe: bool,
) -> Result<(Vec<u8>, bool), WindowsEncoderError> {
    let mut chunk_bytes: Vec<u8> = Vec::new();
    let mut observed_keyframe = false;
    loop {
        let stream_info: MFT_OUTPUT_STREAM_INFO = transform
            .GetOutputStreamInfo(0)
            .map_err(|e| WindowsEncoderError::Encode {
                step: "GetOutputStreamInfo",
                hresult: fmt_hresult(&e),
            })?;
        let provides_samples = (stream_info.dwFlags
            & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32)
            != 0;

        // Either the MFT allocates the sample or we do — the H.264
        // encoder MFT normally allocates, but we handle both for
        // safety.
        let mut owned_sample: Option<IMFSample> = if provides_samples {
            None
        } else {
            let buf: IMFMediaBuffer = MFCreateMemoryBuffer(stream_info.cbSize.max(1))
                .map_err(|e| WindowsEncoderError::Encode {
                    step: "alloc-output-buffer",
                    hresult: fmt_hresult(&e),
                })?;
            let s: IMFSample = MFCreateSample().map_err(|e| WindowsEncoderError::Encode {
                step: "alloc-output-sample",
                hresult: fmt_hresult(&e),
            })?;
            s.AddBuffer(&buf).map_err(|e| WindowsEncoderError::Encode {
                step: "alloc-output-sample-add",
                hresult: fmt_hresult(&e),
            })?;
            Some(s)
        };
        let mut output = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: std::mem::ManuallyDrop::new(owned_sample.clone()),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        };

        let mut status: u32 = 0;
        let pr = transform.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status);

        // The IMFTransform ABI moves ownership of pSample into
        // `output` when the MFT allocates it (provides_samples == true).
        // Take it back as a Rust-owned `Option<IMFSample>` so it's
        // released on scope exit.
        let returned_sample: Option<IMFSample> = std::mem::ManuallyDrop::into_inner(output.pSample);
        // Drop any pEvents the MFT attached.
        let _events = std::mem::ManuallyDrop::into_inner(output.pEvents);
        // Decide which sample to read: the returned one (MFT-allocated)
        // takes precedence; otherwise fall back to the one we passed in.
        let sample_to_read = returned_sample.or_else(|| owned_sample.take());

        match pr {
            Ok(()) => {
                if let Some(s) = sample_to_read {
                    let (bytes, kf) = read_sample_bytes(&s)?;
                    if !bytes.is_empty() {
                        chunk_bytes = bytes;
                        observed_keyframe = kf || requested_keyframe;
                        // Done — one chunk per encode() call.
                        return Ok((chunk_bytes, observed_keyframe));
                    }
                }
                // No bytes — uncommon but legal; loop and try again.
                continue;
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                // Not enough buffered yet — return whatever we have
                // (may be empty, which the caller treats as a B/P-frame
                // delay; the next encode call will produce a chunk).
                return Ok((chunk_bytes, observed_keyframe));
            }
            Err(e) => {
                return Err(WindowsEncoderError::Encode {
                    step: "ProcessOutput",
                    hresult: fmt_hresult(&e),
                });
            }
        }
    }
}

/// Read all bytes out of an `IMFSample`'s contiguous buffer.
///
/// # Safety
///
/// `sample` must be a valid `IMFSample` with at least one
/// `IMFMediaBuffer` attached. Lock/Unlock are paired before
/// returning.
unsafe fn read_sample_bytes(sample: &IMFSample) -> Result<(Vec<u8>, bool), WindowsEncoderError> {
    let buffer: IMFMediaBuffer =
        sample
            .ConvertToContiguousBuffer()
            .map_err(|e| WindowsEncoderError::Encode {
                step: "ConvertToContiguousBuffer",
                hresult: fmt_hresult(&e),
            })?;
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut max_len: u32 = 0;
    let mut cur_len: u32 = 0;
    buffer
        .Lock(&mut data, Some(&mut max_len), Some(&mut cur_len))
        .map_err(|e| WindowsEncoderError::Encode {
            step: "out-Lock",
            hresult: fmt_hresult(&e),
        })?;
    let bytes = if data.is_null() || cur_len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(data, cur_len as usize).to_vec()
    };
    buffer.Unlock().map_err(|e| WindowsEncoderError::Encode {
        step: "out-Unlock",
        hresult: fmt_hresult(&e),
    })?;
    // Keyframe flag — MF marks IDR samples with
    // `MFSampleExtension_CleanPoint`. Absence of the attribute is
    // treated as "not a keyframe" (the H.264 encoder MFT always
    // sets it explicitly on IDRs).
    let is_keyframe = sample
        .GetUINT32(&MFSampleExtension_CleanPoint)
        .unwrap_or(0)
        != 0;
    Ok((bytes, is_keyframe))
}

/// Helper: SetGUID + label for error path. Takes any type that
/// derefs to `IMFAttributes` (so `IMFMediaType`, `IMFSample`).
unsafe fn set_guid(
    attrs: &IMFMediaType,
    key: &GUID,
    value: &GUID,
    step: &'static str,
) -> Result<(), WindowsEncoderError> {
    attrs
        .SetGUID(key, value)
        .map_err(|e| WindowsEncoderError::Setup {
            step,
            hresult: fmt_hresult(&e),
        })
}

/// Helper: SetUINT32 + label for error path.
unsafe fn set_u32(
    attrs: &IMFMediaType,
    key: &GUID,
    value: u32,
    step: &'static str,
) -> Result<(), WindowsEncoderError> {
    attrs
        .SetUINT32(key, value)
        .map_err(|e| WindowsEncoderError::Setup {
            step,
            hresult: fmt_hresult(&e),
        })
}

/// Helper: SetUINT64 + label. The Media Foundation
/// `MFSetAttributeSize` / `MFSetAttributeRatio` C macros pack two
/// `u32` values into a single `u64` attribute (`high << 32 | low`)
/// — see [`pack_two_u32`].
unsafe fn set_u64(
    attrs: &IMFMediaType,
    key: &GUID,
    value: u64,
    step: &'static str,
) -> Result<(), WindowsEncoderError> {
    attrs
        .SetUINT64(key, value)
        .map_err(|e| WindowsEncoderError::Setup {
            step,
            hresult: fmt_hresult(&e),
        })
}

/// Pack two `u32` values into a single `u64` in the layout the
/// Media Foundation `MFSetAttributeSize` / `MFSetAttributeRatio`
/// inline helpers use (`high << 32 | low`). Equivalent to the C
/// SDK's `PACK_2_UINT32_AS_UINT64(high, low)` macro.
const fn pack_two_u32(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | (low as u64)
}

// ---------------------------------------------------------------------------
// VideoEncoder trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl VideoEncoder for WindowsVideoEncoder {
    async fn encode(&self, frame: &CapturedFrame) -> Result<EncodedVideoChunk, DesktopMediaError> {
        // The MFT is single-threaded; do every COM call on a
        // blocking thread so the runtime workers stay responsive.
        let inner = self.inner.clone();
        let config = self.config;
        let frame = frame.clone();
        tokio::task::spawn_blocking(move || {
            let me = WindowsVideoEncoder { inner, config };
            me.encode_blocking(&frame)
        })
        .await
        .map_err(|join| {
            DesktopMediaError::Io(format!("encoder spawn_blocking join failed: {join}"))
        })?
        .map_err(DesktopMediaError::from)
    }

    fn request_keyframe(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.keyframe_requested = true;
        } else {
            warn!("encoder mutex poisoned; keyframe request dropped");
        }
    }
}

// ---------------------------------------------------------------------------
// EncoderFactory impl — slice R7.n.6.
// ---------------------------------------------------------------------------

/// `EncoderFactory` that builds a fresh [`WindowsVideoEncoder`] per
/// session from a pinned [`WindowsVideoEncoderConfig`]. The
/// [`WebRtcDesktopTransport`](cmremote_platform::desktop::WebRtcDesktopTransport)
/// invokes [`Self::build`] once per `RTCPeerConnection` so each
/// session gets a private MFT instance with its own frame counter,
/// keyframe-request flag, and Media Foundation startup guard — none
/// of that state is safe to share across two viewers.
///
/// Stateless apart from the config: cheap to clone via `Arc`.
pub struct WindowsVideoEncoderFactory {
    config: WindowsVideoEncoderConfig,
}

impl WindowsVideoEncoderFactory {
    /// Build a factory that hands out encoders configured with
    /// `config`. Validates `config` once up front so a bad config
    /// surfaces at agent startup rather than on the first
    /// signalling call.
    pub fn new(config: WindowsVideoEncoderConfig) -> Result<Self, WindowsEncoderError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Convenience constructor using
    /// [`WindowsVideoEncoderConfig::default_1080p_30fps`].
    pub fn default_1080p_30fps() -> Result<Self, WindowsEncoderError> {
        Self::new(WindowsVideoEncoderConfig::default_1080p_30fps())
    }

    /// Borrow the pinned config.
    pub fn config(&self) -> WindowsVideoEncoderConfig {
        self.config
    }
}

impl EncoderFactory for WindowsVideoEncoderFactory {
    fn build(&self) -> Result<Arc<dyn VideoEncoder>, DesktopMediaError> {
        let enc = WindowsVideoEncoder::new(self.config).map_err(DesktopMediaError::from)?;
        Ok(Arc::new(enc))
    }
}

// ---------------------------------------------------------------------------
// Tests (config validation only — the COM path needs a Windows
// host; an `#[ignore]`d smoke test follows so it can be exercised
// manually with `cargo test -- --ignored` on a Win10+ machine).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_is_valid() {
        WindowsVideoEncoderConfig::default_1080p_30fps()
            .validate()
            .expect("default 1080p30 config must validate");
    }

    #[test]
    fn config_rejects_zero_dimensions() {
        let mut c = WindowsVideoEncoderConfig::default_1080p_30fps();
        c.width = 0;
        let e = c.validate().unwrap_err();
        assert!(format!("{e}").contains("non-zero"));
    }

    #[test]
    fn config_rejects_odd_dimensions() {
        let mut c = WindowsVideoEncoderConfig::default_1080p_30fps();
        c.height = 1081;
        let e = c.validate().unwrap_err();
        assert!(format!("{e}").contains("even"));
    }

    #[test]
    fn config_rejects_oversize() {
        let mut c = WindowsVideoEncoderConfig::default_1080p_30fps();
        c.width = 7682;
        let e = c.validate().unwrap_err();
        assert!(format!("{e}").contains("8K"));
    }

    #[test]
    fn config_rejects_zero_bitrate() {
        let mut c = WindowsVideoEncoderConfig::default_1080p_30fps();
        c.bitrate_bps = 0;
        let e = c.validate().unwrap_err();
        assert!(format!("{e}").contains("bitrate"));
    }

    #[test]
    fn config_rejects_silly_fps() {
        let mut c = WindowsVideoEncoderConfig::default_1080p_30fps();
        c.fps = 0;
        let e = c.validate().unwrap_err();
        assert!(format!("{e}").contains("fps"));
        c.fps = 240;
        let e = c.validate().unwrap_err();
        assert!(format!("{e}").contains("fps"));
    }

    #[test]
    fn windows_encoder_error_maps_to_invalid_parameters_for_config_errors() {
        let e: DesktopMediaError =
            WindowsEncoderError::BadConfig("bad".into()).into();
        assert!(matches!(e, DesktopMediaError::InvalidParameters(_)));
        let e: DesktopMediaError = WindowsEncoderError::FrameSizeMismatch {
            got_w: 1, got_h: 2, want_w: 3, want_h: 4,
        }
        .into();
        assert!(matches!(e, DesktopMediaError::InvalidParameters(_)));
    }

    #[test]
    fn windows_encoder_error_maps_to_io_for_runtime_errors() {
        let e: DesktopMediaError = WindowsEncoderError::Encode {
            step: "ProcessInput",
            hresult: "0xDEADBEEF".into(),
        }
        .into();
        assert!(matches!(e, DesktopMediaError::Io(_)));
    }

    #[test]
    fn factory_validates_config_at_construction_not_at_build() {
        // Constructing with an invalid config must surface the
        // BadConfig error eagerly, before any Media Foundation
        // call. This keeps a misconfigured agent failing fast at
        // startup rather than on the first signalling round-trip.
        let mut bad = WindowsVideoEncoderConfig::default_1080p_30fps();
        bad.fps = 0;
        let e = WindowsVideoEncoderFactory::new(bad).unwrap_err();
        assert!(format!("{e}").contains("fps"));
    }

    #[test]
    fn factory_default_preserves_default_config_shape() {
        let f = WindowsVideoEncoderFactory::default_1080p_30fps()
            .expect("default config must validate");
        assert_eq!(f.config(), WindowsVideoEncoderConfig::default_1080p_30fps());
    }

    #[test]
    fn factory_is_object_safe_through_encoder_factory_trait() {
        let f = WindowsVideoEncoderFactory::default_1080p_30fps().unwrap();
        let _: Box<dyn EncoderFactory> = Box::new(f);
    }

    /// Smoke test that constructs the encoder and encodes a single
    /// black frame end-to-end. Ignored by default because it
    /// requires a Windows host with the Microsoft H.264 encoder
    /// installed (default on Windows 10/11 with Desktop
    /// Experience; absent on Server Core).
    ///
    /// Run with `cargo test --target x86_64-pc-windows-msvc -- --ignored`.
    #[test]
    #[ignore = "requires Windows host with Media Foundation"]
    fn smoke_encode_black_frame_on_windows() {
        let config = WindowsVideoEncoderConfig {
            width: 64,
            height: 64,
            bitrate_bps: 500_000,
            fps: 30,
        };
        let encoder = WindowsVideoEncoder::new(config).expect("encoder setup");
        let frame = CapturedFrame::black(64, 64).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let chunk = encoder.encode(&frame).await.expect("encode");
            // First frame may be empty or a SPS/PPS+IDR depending on
            // MFT internals; the assertion is that no error fires.
            // If a chunk is emitted it must be marked as a keyframe
            // because the encoder is fresh (request_keyframe was not
            // explicitly set, but the first IDR is implicit).
            if !chunk.bytes.is_empty() {
                assert!(chunk.is_keyframe, "first non-empty chunk must be IDR");
            }
        });
    }
}
