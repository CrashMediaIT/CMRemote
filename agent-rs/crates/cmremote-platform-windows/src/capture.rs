// Source: CMRemote, clean-room implementation.

//! Windows desktop capturer backed by DXGI Desktop Duplication
//! (slice R7.n).
//!
//! ## Pipeline
//!
//! ```text
//!   D3D11CreateDevice (BGRA support, FEATURE_LEVEL_11_0)
//!         │
//!         ▼
//!   IDXGIDevice ──► IDXGIAdapter ──► IDXGIOutput ──► IDXGIOutput1
//!                                                       │
//!                                                       ▼
//!                                       IDXGIOutput1::DuplicateOutput
//!                                                       │
//!                                                       ▼
//!                                          IDXGIOutputDuplication
//!                                                       │
//!                                                       ▼
//!                       AcquireNextFrame ──► IDXGIResource (BGRA texture)
//!                                                       │
//!                                                       ▼
//!                       CopyResource → staging texture (CPU readable)
//!                                                       │
//!                                                       ▼
//!                       Map(D3D11_MAP_READ) → BGRA pixels → CapturedFrame
//!                                                       │
//!                                                       ▼
//!                       Unmap + ReleaseFrame
//! ```
//!
//! ## Threading
//!
//! D3D11 immediate contexts are single-threaded — the device is
//! created with the `D3D11_CREATE_DEVICE_BGRA_SUPPORT` flag and the
//! immediate context is wrapped in a [`Mutex`] so the
//! `DesktopCapturer` impl can stay `Send + Sync` without per-call
//! `CreateDeferredContext` overhead. The capturer is intended to be
//! driven by a single capture task; the mutex is defence-in-depth
//! against accidental concurrent calls from the runtime.
//!
//! ## Error handling
//!
//! Every COM call is wrapped in a `safe_*` helper that converts a
//! `windows::core::Error` into a [`WindowsCaptureError`]. The
//! capturer **never panics** on a failed COM call — capture loss
//! (HRESULT `DXGI_ERROR_ACCESS_LOST`) is mapped to
//! [`DesktopMediaError::Io`] so the WebRTC layer can rebuild the
//! pipeline without taking down the agent. Timeouts are mapped to
//! [`DesktopMediaError::Io`] with a stable string so the calling
//! task can decide whether to retry or back off.
//!
//! ## Security
//!
//! - Only the **primary output** of the **default adapter** is
//!   captured by [`WindowsDesktopCapturer::for_primary_output`]; the
//!   constructor takes no operator-supplied display id, so the
//!   wire-layer guards already enforced by
//!   [`cmremote_platform::desktop::guards`] cannot be bypassed by
//!   feeding a hostile display index.
//! - The captured BGRA buffer is owned by the [`CapturedFrame`] DTO
//!   and is dropped before the next `AcquireNextFrame` call. The
//!   GPU staging texture is reused across calls but never escapes
//!   this module.
//! - Mouse cursor compositing is **not** done in this slice (the
//!   `pointer_visible` / `last_mouse_update_time` fields from
//!   `DXGI_OUTDUPL_FRAME_INFO` are deliberately ignored). Cursor
//!   compositing lands in a follow-up slice with its own threat
//!   model (a malicious shape in shared memory could otherwise be
//!   blitted into the framebuffer).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cmremote_platform::desktop::{CapturedFrame, DesktopCapturer, DesktopMediaError};
use thiserror::Error;
use tracing::{debug, warn};

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_1, D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_3,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// Errors specific to the Windows DXGI capture path.
///
/// All variants are surface-able as
/// [`DesktopMediaError::Io`] for callers that only consume the
/// trait-level error type. The richer enum is preserved so unit
/// tests in this crate (and anyone reaching for the concrete
/// implementation directly) can pattern-match on the failure mode.
#[derive(Debug, Error)]
pub enum WindowsCaptureError {
    /// `D3D11CreateDevice` failed for every supported feature level.
    #[error("failed to create D3D11 device: {0}")]
    DeviceCreation(String),

    /// The default adapter has no usable output (no primary monitor).
    #[error("no DXGI output (primary monitor) available")]
    NoPrimaryOutput,

    /// `IDXGIOutput1::DuplicateOutput` failed; usually means a
    /// fullscreen exclusive app holds the output, or the user is on
    /// the secure desktop (UAC / Ctrl-Alt-Del). The driver should
    /// retry rather than tear the agent down.
    #[error("DuplicateOutput failed: {0}")]
    DuplicateOutput(String),

    /// `AcquireNextFrame` timed out. Maps to
    /// [`DesktopMediaError::Io`] with a stable string the WebRTC
    /// layer can match on for back-pressure handling.
    #[error("AcquireNextFrame timed out after {0:?}")]
    AcquireTimeout(Duration),

    /// The desktop-duplication session was lost (mode change, GPU
    /// reset, secure-desktop transition). The caller MUST drop the
    /// capturer and rebuild — every subsequent `AcquireNextFrame`
    /// will keep returning `DXGI_ERROR_ACCESS_LOST`.
    #[error("DXGI desktop-duplication access lost; capturer must be rebuilt")]
    AccessLost,

    /// A COM call returned an unexpected HRESULT. The string is a
    /// pre-formatted `windows::core::Error` and never carries
    /// operator-supplied data.
    #[error("DXGI COM error: {0}")]
    Com(String),

    /// The frame's reported pitch (`DXGI_MAPPED_RECT::Pitch`) does
    /// not match the texture's width × 4 BGRA stride. Treated as
    /// fatal so we never copy a torn buffer.
    #[error("unexpected mapped pitch: {pitch} for width {width}")]
    UnexpectedPitch {
        /// Reported `RowPitch` from the mapped subresource.
        pitch: u32,
        /// Frame width in pixels (expected stride is `width * 4`).
        width: u32,
    },
}

impl From<WindowsCaptureError> for DesktopMediaError {
    fn from(value: WindowsCaptureError) -> Self {
        // Preserve the message but flatten to the trait-level
        // error type so callers that only know about
        // `DesktopMediaError` can still propagate the failure.
        DesktopMediaError::Io(value.to_string())
    }
}

impl From<windows::core::Error> for WindowsCaptureError {
    fn from(err: windows::core::Error) -> Self {
        WindowsCaptureError::Com(format!(
            "HRESULT 0x{:08X}: {}",
            err.code().0 as u32,
            err.message()
        ))
    }
}

/// Default `AcquireNextFrame` timeout. 100 ms is the value used by
/// the Microsoft sample (`DXGI_DUPLICATE_DESKTOP_SAMPLE`) and gives
/// the encode loop ~10 chances per second to react to back-pressure
/// without burning a CPU core in a tight wait loop.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(100);

/// DXGI Desktop Duplication backed [`DesktopCapturer`] for Windows.
///
/// Construct with [`for_primary_output`](Self::for_primary_output);
/// the constructor performs the D3D11 device + duplication setup
/// once and reuses it for every `capture_next_frame` call. A capture
/// loss (`DXGI_ERROR_ACCESS_LOST`) is reported via
/// [`DesktopMediaError::Io`] and the caller is expected to drop and
/// rebuild the capturer.
pub struct WindowsDesktopCapturer {
    inner: Arc<Mutex<CapturerInner>>,
    acquire_timeout: Duration,
}

struct CapturerInner {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// Cached staging texture, lazily created / resized on first
    /// frame and on resolution change.
    staging: Option<StagingTexture>,
    /// Set once after `AcquireNextFrame` returned ACCESS_LOST so all
    /// subsequent calls fail fast without bothering the GPU.
    access_lost: bool,
}

struct StagingTexture {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

impl WindowsDesktopCapturer {
    /// Create a capturer attached to the **primary output** of the
    /// **default adapter**. This is the only supported entry point
    /// in slice R7.n; multi-monitor selection lands in a follow-up
    /// alongside the consent-prompt UI for picking which display to
    /// share.
    pub fn for_primary_output() -> Result<Self, WindowsCaptureError> {
        Self::with_acquire_timeout(DEFAULT_ACQUIRE_TIMEOUT)
    }

    /// Like [`for_primary_output`](Self::for_primary_output) but with
    /// a caller-supplied `AcquireNextFrame` timeout. Useful in tests
    /// where a faster failure path is preferable to the 100 ms
    /// default.
    pub fn with_acquire_timeout(acquire_timeout: Duration) -> Result<Self, WindowsCaptureError> {
        let (device, context) = create_d3d11_device()?;
        let duplication = duplicate_primary_output(&device)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(CapturerInner {
                _device: device,
                context,
                duplication,
                staging: None,
                access_lost: false,
            })),
            acquire_timeout,
        })
    }

    fn capture_blocking(&self) -> Result<CapturedFrame, WindowsCaptureError> {
        let mut guard = self
            .inner
            .lock()
            .expect("WindowsDesktopCapturer mutex poisoned");
        if guard.access_lost {
            return Err(WindowsCaptureError::AccessLost);
        }

        let timeout_ms: u32 = self
            .acquire_timeout
            .as_millis()
            .try_into()
            .unwrap_or(u32::MAX);

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        // SAFETY: AcquireNextFrame is documented to write through the
        // out-pointers when it returns S_OK. On error it leaves them
        // unchanged, which matches the `Option<...>` initialisation
        // above. We translate the documented timeout / access-lost
        // HRESULTs back into structured errors before the caller
        // ever sees a raw HRESULT.
        let hr = unsafe {
            guard
                .duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
        };

        match hr {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                return Err(WindowsCaptureError::AcquireTimeout(self.acquire_timeout));
            }
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                guard.access_lost = true;
                return Err(WindowsCaptureError::AccessLost);
            }
            Err(e) => return Err(e.into()),
        }

        let resource = resource.ok_or_else(|| {
            WindowsCaptureError::Com("AcquireNextFrame returned S_OK but no resource".into())
        })?;

        // Always release the acquired frame before returning, even
        // on the error path below.
        let result = (|| -> Result<CapturedFrame, WindowsCaptureError> {
            let frame_texture: ID3D11Texture2D =
                resource.cast().map_err(WindowsCaptureError::from)?;

            // SAFETY: GetDesc is documented to fully initialise its
            // out-parameter on every call (no failure mode).
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { frame_texture.GetDesc(&mut desc) };

            ensure_or_resize_staging(&mut guard, &desc)?;
            let staging = guard
                .staging
                .as_ref()
                .expect("staging texture initialised above")
                .texture
                .clone();

            // SAFETY: Both source and destination textures live in
            // the device above; CopyResource performs a same-device
            // copy with no overlapping allocations because the
            // staging texture is freshly allocated for this device.
            unsafe { guard.context.CopyResource(&staging, &frame_texture) };

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: Map writes the out-parameter on success and
            // leaves it untouched on failure. We Unmap before the
            // borrow of `mapped.pData` ends.
            unsafe {
                guard
                    .context
                    .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?
            };

            let map_result = (|| -> Result<CapturedFrame, WindowsCaptureError> {
                let width = desc.Width;
                let height = desc.Height;
                let pitch = mapped.RowPitch;
                let expected_pitch = width
                    .checked_mul(4)
                    .ok_or(WindowsCaptureError::UnexpectedPitch { pitch, width })?;
                if pitch < expected_pitch {
                    return Err(WindowsCaptureError::UnexpectedPitch { pitch, width });
                }

                let total = (pitch as usize)
                    .checked_mul(height as usize)
                    .ok_or(WindowsCaptureError::UnexpectedPitch { pitch, width })?;
                let mut bgra = vec![0u8; (expected_pitch as usize) * (height as usize)];
                // SAFETY: `mapped.pData` is valid for at least
                // `pitch * height` bytes (DXGI contract for a 2D
                // staging map). We copy row-by-row when `pitch >
                // expected_pitch` to drop the per-row padding.
                unsafe {
                    if pitch == expected_pitch {
                        std::ptr::copy_nonoverlapping(
                            mapped.pData as *const u8,
                            bgra.as_mut_ptr(),
                            total.min(bgra.len()),
                        );
                    } else {
                        for row in 0..height as usize {
                            let src = (mapped.pData as *const u8).add(row * pitch as usize);
                            let dst = bgra.as_mut_ptr().add(row * expected_pitch as usize);
                            std::ptr::copy_nonoverlapping(src, dst, expected_pitch as usize);
                        }
                    }
                }

                Ok(CapturedFrame {
                    width,
                    height,
                    stride: expected_pitch,
                    timestamp_micros: monotonic_micros(),
                    bgra,
                })
            })();

            // SAFETY: Always unmap before releasing the borrow,
            // regardless of whether the copy succeeded.
            unsafe { guard.context.Unmap(&staging, 0) };

            map_result
        })();

        // SAFETY: ReleaseFrame is the documented inverse of
        // AcquireNextFrame and must run exactly once per acquire
        // even on error paths. We do **not** propagate its result —
        // a failure here would only matter for the next acquire,
        // which will surface it directly.
        let release_hr = unsafe { guard.duplication.ReleaseFrame() };
        if let Err(e) = release_hr {
            warn!(error = %e, "IDXGIOutputDuplication::ReleaseFrame failed");
        }

        result
    }
}

#[async_trait]
impl DesktopCapturer for WindowsDesktopCapturer {
    async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
        // DXGI is a blocking COM API; offload to the blocking pool
        // so we don't stall the runtime on AcquireNextFrame's
        // internal kernel wait. `tokio::task::spawn_blocking`
        // returns the join error as a panic, so we wrap it in our
        // own structured error rather than letting it abort the
        // capture task.
        let inner = self.inner.clone();
        let timeout = self.acquire_timeout;
        let join = tokio::task::spawn_blocking(move || {
            // Reconstruct just enough of the public type to call
            // `capture_blocking`; we deliberately don't expose this
            // helper publicly because the blocking call must always
            // run inside `spawn_blocking`.
            let capturer = WindowsDesktopCapturer {
                inner,
                acquire_timeout: timeout,
            };
            capturer.capture_blocking()
        })
        .await;

        match join {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(e)) => {
                debug!(error = %e, "DXGI capture failed");
                Err(e.into())
            }
            Err(join_err) => Err(DesktopMediaError::Io(format!(
                "capture task panicked: {join_err}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — D3D11 device / staging-texture construction.
// ---------------------------------------------------------------------------

fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext), WindowsCaptureError> {
    // Mirror the feature-level set the Microsoft sample requests.
    // We try hardware first, then the WARP software adapter as a
    // last-resort fallback so headless / VM environments still get
    // a working device for unit tests.
    let feature_levels = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
        D3D_FEATURE_LEVEL_9_2,
        D3D_FEATURE_LEVEL_9_1,
    ];

    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;

    // SAFETY: D3D11CreateDevice's out-pointers are written on
    // success and left untouched on failure (documented in MSDN).
    // We pass a non-null `Adapter = None`, which selects the
    // default adapter — same behaviour as the Microsoft sample.
    let result = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    };

    if result.is_err() {
        // Hardware adapter failed; try the WARP software fallback
        // so we still build a device on a headless CI runner.
        // SAFETY: Same contract as above.
        let warp = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        warp.map_err(|e| {
            WindowsCaptureError::DeviceCreation(format!(
                "hardware: {result:?}; warp: HRESULT 0x{:08X} {}",
                e.code().0 as u32,
                e.message()
            ))
        })?;
    }

    let device = device
        .ok_or_else(|| WindowsCaptureError::DeviceCreation("device out-pointer null".into()))?;
    let context = context
        .ok_or_else(|| WindowsCaptureError::DeviceCreation("context out-pointer null".into()))?;
    Ok((device, context))
}

fn duplicate_primary_output(
    device: &ID3D11Device,
) -> Result<IDXGIOutputDuplication, WindowsCaptureError> {
    let dxgi_device: IDXGIDevice = device.cast().map_err(WindowsCaptureError::from)?;
    // SAFETY: GetAdapter returns a refcounted COM pointer on
    // success; the `windows` crate handles the AddRef/Release.
    let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter()? };

    // SAFETY: EnumOutputs returns DXGI_ERROR_NOT_FOUND when the
    // index is past the last output; we treat that as
    // "no primary output" rather than a hard failure.
    let output: IDXGIOutput = unsafe {
        adapter
            .EnumOutputs(0)
            .map_err(|_| WindowsCaptureError::NoPrimaryOutput)?
    };
    let output1: IDXGIOutput1 = output.cast().map_err(WindowsCaptureError::from)?;

    // SAFETY: DuplicateOutput returns the duplication interface on
    // success. Its only documented failure modes are
    // E_ACCESSDENIED (secure desktop), DXGI_ERROR_UNSUPPORTED
    // (RDP without the required protocol), and E_INVALIDARG.
    let duplication = unsafe {
        output1.DuplicateOutput(device).map_err(|e| {
            WindowsCaptureError::DuplicateOutput(format!(
                "HRESULT 0x{:08X}: {}",
                e.code().0 as u32,
                e.message(),
            ))
        })?
    };
    Ok(duplication)
}

fn ensure_or_resize_staging(
    inner: &mut CapturerInner,
    frame_desc: &D3D11_TEXTURE2D_DESC,
) -> Result<(), WindowsCaptureError> {
    if let Some(s) = &inner.staging {
        if s.width == frame_desc.Width && s.height == frame_desc.Height {
            return Ok(());
        }
    }

    let desc = D3D11_TEXTURE2D_DESC {
        Width: frame_desc.Width,
        Height: frame_desc.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: CreateTexture2D writes its out-parameter on success.
    // No initial data is provided (`pInitialData = None`) because
    // the texture is overwritten by `CopyResource` before the
    // first read.
    unsafe {
        inner
            ._device
            .CreateTexture2D(&desc, None, Some(&mut texture))
    }
    .map_err(WindowsCaptureError::from)?;
    let texture =
        texture.ok_or_else(|| WindowsCaptureError::Com("CreateTexture2D returned null".into()))?;

    inner.staging = Some(StagingTexture {
        texture,
        width: frame_desc.Width,
        height: frame_desc.Height,
    });
    Ok(())
}

fn monotonic_micros() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    Instant::now()
        .saturating_duration_since(*epoch)
        .as_micros()
        .min(u64::MAX as u128) as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// The construction tests are `#[ignore]` by default because they
// require an interactive desktop session — DXGI Desktop Duplication
// refuses to attach when the agent runs as a SYSTEM service on the
// secure desktop, and CI runners often start in that state. A
// developer running the tests locally on Windows passes
// `cargo test -- --ignored` to drive them.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_capture_error_into_desktop_media_error_preserves_message() {
        let e = WindowsCaptureError::AccessLost;
        let s = e.to_string();
        let media: DesktopMediaError = WindowsCaptureError::AccessLost.into();
        match media {
            DesktopMediaError::Io(msg) => assert_eq!(msg, s),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn windows_capture_error_for_timeout_carries_duration() {
        let e = WindowsCaptureError::AcquireTimeout(Duration::from_millis(42));
        assert!(e.to_string().contains("42"));
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn for_primary_output_constructs_against_real_desktop() {
        let _ = WindowsDesktopCapturer::for_primary_output().expect("primary output capturer");
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires an interactive Windows desktop session"]
    async fn capture_next_frame_returns_bgra_frame() {
        let cap = WindowsDesktopCapturer::with_acquire_timeout(Duration::from_millis(500))
            .expect("primary output capturer");
        // Some frames may legitimately time out (no screen update);
        // retry a small bounded number of times before failing the
        // test outright.
        let mut last = None;
        for _ in 0..10 {
            match cap.capture_next_frame().await {
                Ok(f) => {
                    assert_eq!(f.bgra.len() as u32, f.stride * f.height);
                    assert!(f.width > 0 && f.height > 0);
                    return;
                }
                Err(e) => last = Some(e),
            }
        }
        panic!("no frame after 10 attempts; last error: {last:?}");
    }
}
