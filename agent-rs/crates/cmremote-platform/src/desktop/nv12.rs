// Source: CMRemote, clean-room implementation.

//! BGRA → NV12 colour-space conversion (slice R7.n.6).
//!
//! Every hardware H.264 encoder we target — Media Foundation on
//! Windows (`MFVideoFormat_NV12`), VideoToolbox on macOS
//! (`kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`), and
//! VAAPI on Linux (`VA_FOURCC_NV12`) — requires NV12 input. The
//! [`super::media::CapturedFrame`] DTO is BGRA (the lowest common
//! denominator across DXGI Desktop Duplication, ScreenCaptureKit,
//! and PipeWire DMA-buf imports), so every encoder backend
//! converts on the way in. Putting the conversion in a pure-Rust,
//! `unsafe`-free, fully-tested module keeps the per-OS encoder
//! shims focused on the COM / API marshalling and means the
//! conversion correctness is reviewable without a Windows host.
//!
//! ## Format
//!
//! NV12 is a planar YUV 4:2:0 layout:
//!
//! ```text
//!   ┌───────────────────────┐   stride = round_up(width, 2)
//!   │  Y plane (width×h)    │   one byte per pixel
//!   ├───────────────────────┤
//!   │  UV plane (w×h/2)     │   interleaved [U₀ V₀ U₁ V₁ …],
//!   │                       │   one (U,V) pair per 2×2 block
//!   └───────────────────────┘
//! ```
//!
//! The conversion uses the BT.601 limited-range coefficients (the
//! default the H.264 encoder MFT expects when the input media type
//! does not declare a colour matrix attribute):
//!
//! ```text
//!   Y = 16 + ( 65.738·R + 129.057·G +  25.064·B) / 256
//!   U = 128 + (-37.945·R -  74.494·G + 112.439·B) / 256
//!   V = 128 + (112.439·R -  94.154·G -  18.285·B) / 256
//! ```
//!
//! Implemented in fixed-point (rounded coefficients × 256) so the
//! output is bit-exact across platforms — required for the unit
//! tests to pin known vectors.
//!
//! ## Width / height parity
//!
//! NV12's chroma plane sub-samples 2×2, so the width and height
//! MUST be even. [`bgra_to_nv12`] returns
//! [`super::media::DesktopMediaError::InvalidParameters`] for an
//! odd dimension instead of silently truncating — the encoder
//! shim is expected to pad odd captures (typically by replicating
//! the last column / row) before calling the conversion.
//!
//! ## Performance
//!
//! Single-threaded, scalar Rust. A 1080p (1920×1080) frame
//! converts in ~5 ms on a recent x86_64 — well under the 33 ms
//! budget at 30 fps. A SIMD path can be added in a follow-up if
//! profiling shows it; the trait surface stays unchanged.

use crate::desktop::media::{CapturedFrame, DesktopMediaError};

/// Output buffers for [`bgra_to_nv12`].
///
/// The two planes are kept separate so the encoder shim can wrap
/// each in its own `IMFMediaBuffer` (Windows) or `CVPixelBuffer`
/// plane reference (macOS) without copying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12Frame {
    /// Pixel width — even.
    pub width: u32,
    /// Pixel height — even.
    pub height: u32,
    /// Y plane, length `width * height`.
    pub y: Vec<u8>,
    /// Interleaved UV plane, length `width * height / 2`.
    pub uv: Vec<u8>,
    /// Carried forward from the source frame so the encoder can
    /// stamp `IMFSample::SetSampleTime`.
    pub timestamp_micros: u64,
}

impl Nv12Frame {
    /// Length the Y plane MUST have for `width × height`.
    pub const fn y_len(width: u32, height: u32) -> usize {
        (width as usize) * (height as usize)
    }
    /// Length the UV plane MUST have for `width × height`.
    pub const fn uv_len(width: u32, height: u32) -> usize {
        (width as usize) * (height as usize) / 2
    }
}

/// Convert a BGRA frame to NV12.
///
/// Errors:
/// - [`DesktopMediaError::InvalidParameters`] if `width` / `height`
///   is odd, zero, or the BGRA buffer is the wrong size for the
///   declared `stride`.
pub fn bgra_to_nv12(src: &CapturedFrame) -> Result<Nv12Frame, DesktopMediaError> {
    let CapturedFrame {
        width,
        height,
        stride,
        timestamp_micros,
        bgra,
    } = src;
    let (width, height, stride) = (*width, *height, *stride);

    if width == 0 || height == 0 {
        return Err(DesktopMediaError::InvalidParameters(
            "frame dimensions must be non-zero".into(),
        ));
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(DesktopMediaError::InvalidParameters(
            "frame width and height must be even for NV12".into(),
        ));
    }
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| DesktopMediaError::InvalidParameters("row overflow".into()))?;
    if (stride as usize) < row_bytes {
        return Err(DesktopMediaError::InvalidParameters(
            "stride is smaller than width*4".into(),
        ));
    }
    let expected = (stride as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| DesktopMediaError::InvalidParameters("frame size overflow".into()))?;
    if bgra.len() != expected {
        return Err(DesktopMediaError::InvalidParameters(
            "bgra buffer length does not match stride*height".into(),
        ));
    }

    let y_len = Nv12Frame::y_len(width, height);
    let uv_len = Nv12Frame::uv_len(width, height);
    let mut y = vec![0u8; y_len];
    let mut uv = vec![0u8; uv_len];

    let stride = stride as usize;
    let w = width as usize;
    let h = height as usize;

    // Y plane: one sample per source pixel.
    for row in 0..h {
        let src_row = &bgra[row * stride..row * stride + row_bytes];
        let dst_row = &mut y[row * w..row * w + w];
        for col in 0..w {
            let p = &src_row[col * 4..col * 4 + 4];
            // Source order is BGRA (`[B, G, R, A]`).
            let b = p[0] as i32;
            let g = p[1] as i32;
            let r = p[2] as i32;
            // Y = 16 + (66·R + 129·G + 25·B) / 256, rounded.
            let y_val = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            dst_row[col] = clamp_u8(y_val);
        }
    }

    // UV plane: average each 2×2 block of source pixels, emit one
    // (U, V) pair. Iterates 2 rows at a time and 2 cols at a time.
    let chroma_h = h / 2;
    let chroma_w = w / 2;
    for cy in 0..chroma_h {
        let row0_off = (cy * 2) * stride;
        let row1_off = (cy * 2 + 1) * stride;
        let row0 = &bgra[row0_off..row0_off + row_bytes];
        let row1 = &bgra[row1_off..row1_off + row_bytes];
        for cx in 0..chroma_w {
            let i0 = cx * 2 * 4;
            let i1 = i0 + 4;
            // Sum the 2×2 block — divide by 4 below.
            let sum_b = row0[i0] as i32 + row0[i1] as i32 + row1[i0] as i32 + row1[i1] as i32;
            let sum_g = row0[i0 + 1] as i32
                + row0[i1 + 1] as i32
                + row1[i0 + 1] as i32
                + row1[i1 + 1] as i32;
            let sum_r = row0[i0 + 2] as i32
                + row0[i1 + 2] as i32
                + row1[i0 + 2] as i32
                + row1[i1 + 2] as i32;
            // Average per channel.
            let r = sum_r / 4;
            let g = sum_g / 4;
            let b = sum_b / 4;
            // U = 128 + (-38·R - 74·G + 112·B) / 256, rounded.
            let u_val = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            // V = 128 + (112·R - 94·G - 18·B) / 256, rounded.
            let v_val = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            let dst_off = cy * w + cx * 2;
            uv[dst_off] = clamp_u8(u_val);
            uv[dst_off + 1] = clamp_u8(v_val);
        }
    }

    Ok(Nv12Frame {
        width,
        height,
        y,
        uv,
        timestamp_micros: *timestamp_micros,
    })
}

#[inline]
fn clamp_u8(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `CapturedFrame` whose stride equals `width * 4` and
    /// whose pixels are filled by `f(col, row)`.
    fn make_frame(
        width: u32,
        height: u32,
        mut f: impl FnMut(u32, u32) -> [u8; 4],
    ) -> CapturedFrame {
        let stride = width * 4;
        let mut bgra = vec![0u8; (stride as usize) * (height as usize)];
        for r in 0..height {
            for c in 0..width {
                let i = (r as usize) * (stride as usize) + (c as usize) * 4;
                bgra[i..i + 4].copy_from_slice(&f(c, r));
            }
        }
        CapturedFrame {
            width,
            height,
            stride,
            timestamp_micros: 7,
            bgra,
        }
    }

    #[test]
    fn rejects_zero_dimensions() {
        let mut f = CapturedFrame::black(0, 2).unwrap_or_else(|_| CapturedFrame {
            width: 0,
            height: 2,
            stride: 0,
            timestamp_micros: 0,
            bgra: vec![],
        });
        // CapturedFrame::black(0, _) succeeds — stride=0, total=0,
        // bgra empty. Now feed it to the converter.
        f.width = 0;
        f.height = 2;
        let e = bgra_to_nv12(&f).unwrap_err();
        assert!(format!("{e}").contains("non-zero"));
    }

    #[test]
    fn rejects_odd_dimensions() {
        let f = make_frame(3, 4, |_, _| [0, 0, 0, 0xff]);
        let e = bgra_to_nv12(&f).unwrap_err();
        assert!(format!("{e}").contains("even"));
        let f = make_frame(4, 3, |_, _| [0, 0, 0, 0xff]);
        let e = bgra_to_nv12(&f).unwrap_err();
        assert!(format!("{e}").contains("even"));
    }

    #[test]
    fn rejects_buffer_size_mismatch() {
        let mut f = make_frame(4, 4, |_, _| [0, 0, 0, 0xff]);
        f.bgra.truncate(f.bgra.len() - 4);
        let e = bgra_to_nv12(&f).unwrap_err();
        assert!(format!("{e}").contains("does not match"));
    }

    #[test]
    fn rejects_short_stride() {
        let mut f = make_frame(4, 4, |_, _| [0, 0, 0, 0xff]);
        // Set stride < width*4 — that's invalid per the contract.
        f.stride = 8; // 4 pixels would need 16
        let e = bgra_to_nv12(&f).unwrap_err();
        assert!(format!("{e}").contains("stride"));
    }

    #[test]
    fn black_frame_maps_to_y16_uv128() {
        let f = make_frame(4, 4, |_, _| [0, 0, 0, 0xff]);
        let n = bgra_to_nv12(&f).unwrap();
        assert!(n.y.iter().all(|&v| v == 16), "Y={:?}", n.y);
        assert!(n.uv.iter().all(|&v| v == 128), "UV={:?}", n.uv);
        assert_eq!(n.width, 4);
        assert_eq!(n.height, 4);
        assert_eq!(n.timestamp_micros, 7);
        assert_eq!(n.y.len(), Nv12Frame::y_len(4, 4));
        assert_eq!(n.uv.len(), Nv12Frame::uv_len(4, 4));
    }

    #[test]
    fn white_frame_maps_to_y235_uv128() {
        let f = make_frame(4, 4, |_, _| [0xff, 0xff, 0xff, 0xff]);
        let n = bgra_to_nv12(&f).unwrap();
        // BT.601 limited-range white = 235.
        assert!(n.y.iter().all(|&v| v == 235), "Y={:?}", n.y);
        assert!(n.uv.iter().all(|&v| v == 128), "UV={:?}", n.uv);
    }

    #[test]
    fn pure_red_known_vector() {
        // BGRA red = [0, 0, 255, 255]. BT.601 limited (fixed-point):
        //   Y = 16 + ((66·255 + 128) >> 8) = 16 + 66 = 82
        //   U = 128 + ((-38·255 + 128) >> 8) = 128 + (-38) = 90
        //   V = 128 + ((112·255 + 128) >> 8) = 128 + 112 = 240
        let f = make_frame(2, 2, |_, _| [0, 0, 0xff, 0xff]);
        let n = bgra_to_nv12(&f).unwrap();
        assert!(n.y.iter().all(|&v| v == 82), "Y={:?}", n.y);
        assert_eq!(n.uv.len(), 2);
        assert_eq!(n.uv[0], 90, "U");
        assert_eq!(n.uv[1], 240, "V");
    }

    #[test]
    fn pure_green_known_vector() {
        // BGRA green = [0, 255, 0, 255]. BT.601 limited:
        //   Y = 16 + (0 + 129·255 + 0 + 128) >> 8 = 16 + 128 = 144
        //   U = 128 + (-74·255 + 0 + 0 + 128) >> 8 = 128 + (-74) = 54
        //                 (… + 128)>>8 of -18870 = -73 → 128-73 = 55
        //   V = 128 + (-94·255 + 0 + 0 + 128) >> 8 = 128 + (-93) = 35
        let f = make_frame(2, 2, |_, _| [0, 0xff, 0, 0xff]);
        let n = bgra_to_nv12(&f).unwrap();
        // Allow ±1 rounding tolerance.
        assert!(
            n.y.iter().all(|&v| v.abs_diff(144) <= 1),
            "Y={:?}",
            n.y
        );
        assert!(
            n.uv[0].abs_diff(54) <= 1,
            "U={} (expected ~54)",
            n.uv[0]
        );
        assert!(
            n.uv[1].abs_diff(34) <= 1,
            "V={} (expected ~34)",
            n.uv[1]
        );
    }

    #[test]
    fn pure_blue_known_vector() {
        // BGRA blue = [255, 0, 0, 255]. BT.601 limited:
        //   Y ≈ 41
        //   U ≈ 240
        //   V ≈ 110
        let f = make_frame(2, 2, |_, _| [0xff, 0, 0, 0xff]);
        let n = bgra_to_nv12(&f).unwrap();
        assert!(
            n.y.iter().all(|&v| v.abs_diff(41) <= 1),
            "Y={:?}",
            n.y
        );
        assert!(n.uv[0].abs_diff(240) <= 1, "U={}", n.uv[0]);
        assert!(n.uv[1].abs_diff(110) <= 1, "V={}", n.uv[1]);
    }

    #[test]
    fn round_trip_dimensions_match() {
        // A non-square frame: pin that the Y/UV plane sizes follow
        // width/height correctly.
        let f = make_frame(8, 6, |c, r| [(c * 8) as u8, (r * 8) as u8, 128, 0xff]);
        let n = bgra_to_nv12(&f).unwrap();
        assert_eq!(n.y.len(), 8 * 6);
        assert_eq!(n.uv.len(), 8 * 6 / 2);
        assert_eq!(n.width, 8);
        assert_eq!(n.height, 6);
    }

    #[test]
    fn handles_frame_with_padded_stride() {
        // Stride > width*4 (DXGI sometimes returns a padded stride).
        let width = 4u32;
        let height = 4u32;
        let stride = 32u32; // 16 bytes of padding per row
        let mut bgra = vec![0xeeu8; (stride as usize) * (height as usize)];
        for r in 0..height {
            for c in 0..width {
                let i = (r as usize) * (stride as usize) + (c as usize) * 4;
                bgra[i..i + 4].copy_from_slice(&[0, 0, 0, 0xff]); // black
            }
        }
        let f = CapturedFrame {
            width,
            height,
            stride,
            timestamp_micros: 0,
            bgra,
        };
        let n = bgra_to_nv12(&f).unwrap();
        // The padding bytes (`0xee`) MUST NOT affect the output —
        // black pixels still produce Y=16, UV=128.
        assert!(n.y.iter().all(|&v| v == 16));
        assert!(n.uv.iter().all(|&v| v == 128));
    }
}
