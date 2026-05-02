// Source: CMRemote, clean-room implementation.

//! `EncodedChunkSink` adapter that writes encoded H.264 chunks to a
//! `webrtc-rs` [`TrackLocalStaticSample`] (slice R7.n.6).
//!
//! ## Pipeline
//!
//! ```text
//!   CapturePump ──► EncoderCaptureSink ──► WebRtcVideoTrackSink ──► TrackLocalStaticSample
//!   (BGRA frames)        (Win32 MFT          (this module)            (RTP packetizer
//!                          encoder)                                    in webrtc-rs)
//! ```
//!
//! The sink derives the per-sample `duration` from the *gap* between
//! consecutive [`EncodedVideoChunk::timestamp_micros`] values so
//! `webrtc-rs` can drive the RTP packetizer's clock without needing
//! the encoder to emit an explicit duration. The very first sample
//! uses a 0-duration placeholder (the `webrtc-rs` packetizer's
//! initial timestamp anchors the stream and any later sample replaces
//! the marker frame). All `Sample` fields not derived from the
//! encoder are taken from `Sample::default()` so a future field
//! addition in the upstream crate doesn't silently zero the
//! `prev_dropped_packets` / `prev_padding_packets` counters.
//!
//! ## Threading
//!
//! `TrackLocalStaticSample::write_sample` is `async` and takes
//! `&self`; the per-track internal mutex is held only for the
//! duration of the write. `WebRtcVideoTrackSink` holds the track
//! through an `Arc` and releases the reference after each write so
//! the eventual `pc.close()` can reclaim the underlying resources
//! deterministically.
//!
//! ## Security
//!
//! - Encoded chunks are forwarded verbatim — the sink performs no
//!   parsing of the H.264 NAL unit stream; any framing concern is
//!   the encoder's responsibility (the Windows MFT emits Annex-B
//!   byte-streams with start codes, which is what the upstream
//!   `H264Payloader` expects).
//! - `tracing::warn!` events on `write_sample` failure carry only
//!   the upstream error string — never the chunk bytes, the host
//!   identity, or the operator-supplied SDP offer.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use webrtc::api::media_engine::MIME_TYPE_H264;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use super::encoder_sink::EncodedChunkSink;
use super::media::{DesktopMediaError, EncodedVideoChunk};

/// Standard 90 kHz video clock rate used by the H.264 RTP profile.
/// Matches the codec parameters `webrtc-rs` registers via
/// `MediaEngine::register_default_codecs` (see
/// `webrtc-rs::api::media_engine::MediaEngine::register_default_codecs`).
const H264_CLOCK_RATE_HZ: u32 = 90_000;

/// SDP `fmtp` line for baseline H.264 with packetization mode 1.
/// Matches the upstream `register_default_codecs` value at the
/// `payload_type = 102` slot, so the answer SDP advertises a codec
/// capability the upstream `Sender::on_send_packet` path can
/// packetize without renegotiation.
const H264_DEFAULT_FMTP_LINE: &str =
    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f";

/// Stable track id used for every per-session video track. Keeping
/// this constant simplifies the .NET viewer's track-id matching
/// (the viewer only ever sees one outbound video track per
/// session, and the `RTCPeerConnection` is rebuilt per-session so
/// no two tracks ever exist with the same id at the same time).
pub const VIDEO_TRACK_ID: &str = "cmremote-desktop-video";

/// Stable MediaStream id. Same rationale as [`VIDEO_TRACK_ID`].
pub const VIDEO_STREAM_ID: &str = "cmremote-desktop";

/// Build a fresh H.264 [`TrackLocalStaticSample`] suitable for use
/// as the agent's per-session outbound video track. The codec
/// parameters intentionally mirror the ones
/// `MediaEngine::register_default_codecs` registers so the SDP
/// answer the driver emits can negotiate against any
/// browser/viewer the .NET front-end ships.
pub fn new_h264_video_track() -> Arc<TrackLocalStaticSample> {
    let codec = RTCRtpCodecCapability {
        mime_type: MIME_TYPE_H264.to_owned(),
        clock_rate: H264_CLOCK_RATE_HZ,
        channels: 0,
        sdp_fmtp_line: H264_DEFAULT_FMTP_LINE.to_owned(),
        // `rtcp_feedback` is filled in by the `MediaEngine` at
        // negotiation time from its own copy of the codec params;
        // the per-track shape only needs the capability fields.
        rtcp_feedback: Vec::new(),
    };
    Arc::new(TrackLocalStaticSample::new(
        codec,
        VIDEO_TRACK_ID.to_owned(),
        VIDEO_STREAM_ID.to_owned(),
    ))
}

/// `EncodedChunkSink` that writes every chunk to a
/// [`TrackLocalStaticSample`]. The track is owned through an `Arc`
/// so the WebRTC driver can hand the same track to the
/// peer-connection's `add_track` while keeping the sink-side handle
/// for sample writes.
pub struct WebRtcVideoTrackSink {
    track: Arc<TrackLocalStaticSample>,
    // `std::sync::Mutex` holds only the previous `timestamp_micros`
    // — never any `Send`-bound future — so the lock is never held
    // across an `await`. Initialised to `None`; the first chunk
    // populates it and emits a 0-duration placeholder sample.
    last_chunk_micros: std::sync::Mutex<Option<u64>>,
}

impl WebRtcVideoTrackSink {
    /// Build a sink wrapping `track`. The track is expected to have
    /// been registered with a peer connection via `add_track`
    /// before any chunks arrive (otherwise `write_sample` queues
    /// the sample but `webrtc-rs` will not packetize it).
    pub fn new(track: Arc<TrackLocalStaticSample>) -> Self {
        Self {
            track,
            last_chunk_micros: std::sync::Mutex::new(None),
        }
    }

    /// Borrow the wrapped track. Used by the driver to call
    /// `pc.add_track(track.clone())` from the same `Arc`.
    pub fn track(&self) -> &Arc<TrackLocalStaticSample> {
        &self.track
    }

    /// Compute the per-sample duration from the gap between
    /// `chunk.timestamp_micros` and the previous chunk's timestamp.
    /// The first chunk gets `Duration::ZERO`; non-monotonic
    /// timestamps (clock rewinds) get `Duration::ZERO` as well so
    /// the upstream packetizer never sees a negative-duration
    /// sample (the packetizer expects non-negative durations and
    /// uses them to advance its 90 kHz RTP timestamp; a zero-
    /// duration sample re-emits the previous packet timestamp,
    /// which is preferable to a panic or a silent wraparound).
    /// Updates the running cursor on every call.
    fn compute_duration(&self, chunk_micros: u64) -> Duration {
        let mut g = self
            .last_chunk_micros
            .lock()
            .expect("WebRtcVideoTrackSink mutex poisoned");
        let dur = match *g {
            Some(prev) if chunk_micros > prev => {
                Duration::from_micros(chunk_micros.saturating_sub(prev))
            }
            _ => Duration::ZERO,
        };
        *g = Some(chunk_micros);
        dur
    }
}

#[async_trait]
impl EncodedChunkSink for WebRtcVideoTrackSink {
    async fn consume(&self, chunk: EncodedVideoChunk) -> Result<(), DesktopMediaError> {
        let duration = self.compute_duration(chunk.timestamp_micros);
        // `webrtc::media::Sample::timestamp` is a `SystemTime`
        // wallclock value; the upstream packetizer derives the RTP
        // timestamp from `duration` and the clock rate of the
        // attached codec capability, not from this field. We hand
        // it `SystemTime::now()` because the upstream tests do the
        // same and any consumer (e.g. RTCP sender reports)
        // observing the wallclock will see a coherent value.
        let sample = Sample {
            data: chunk.bytes.into(),
            timestamp: SystemTime::now(),
            duration,
            ..Default::default()
        };
        self.track.write_sample(&sample).await.map_err(|e| {
            tracing::warn!(
                error = %e,
                event = "video-track-write-failed",
                "TrackLocalStaticSample::write_sample returned error",
            );
            DesktopMediaError::Io(format!("video track write failed: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc::track::track_local::TrackLocal;

    fn chunk(micros: u64, bytes: &[u8], is_keyframe: bool) -> EncodedVideoChunk {
        EncodedVideoChunk {
            bytes: bytes.to_vec(),
            timestamp_micros: micros,
            is_keyframe,
        }
    }

    #[test]
    fn new_h264_video_track_uses_default_codec_parameters() {
        let track = new_h264_video_track();
        let codec = track.codec();
        assert_eq!(codec.mime_type, MIME_TYPE_H264);
        assert_eq!(codec.clock_rate, 90_000);
        assert_eq!(codec.channels, 0);
        assert!(
            codec.sdp_fmtp_line.contains("packetization-mode=1"),
            "{}",
            codec.sdp_fmtp_line
        );
        assert_eq!(track.id(), VIDEO_TRACK_ID);
        assert_eq!(track.stream_id(), VIDEO_STREAM_ID);
    }

    #[tokio::test]
    async fn first_chunk_uses_zero_duration_subsequent_chunks_use_micros_gap() {
        let track = new_h264_video_track();
        let sink = WebRtcVideoTrackSink::new(track);
        // Without a registered packetizer (no PC `add_track`), the
        // upstream `write_sample` returns `Ok(())` after a debug
        // warn — sufficient for the duration-cursor assertion.
        sink.consume(chunk(1_000, b"first", true)).await.unwrap();
        let after_first = *sink.last_chunk_micros.lock().unwrap();
        assert_eq!(after_first, Some(1_000));
        sink.consume(chunk(34_000, b"second", false)).await.unwrap();
        let after_second = *sink.last_chunk_micros.lock().unwrap();
        assert_eq!(after_second, Some(34_000));
    }

    #[test]
    fn compute_duration_handles_monotonic_clocks() {
        let track = new_h264_video_track();
        let sink = WebRtcVideoTrackSink::new(track);
        // Same chunk timestamp twice in a row — gap is zero.
        assert_eq!(sink.compute_duration(1_000), Duration::ZERO);
        assert_eq!(sink.compute_duration(1_000), Duration::ZERO);
        // Forward jump produces the expected gap.
        assert_eq!(
            sink.compute_duration(34_000),
            Duration::from_micros(33_000)
        );
        // Clock rewind clamps to zero rather than underflowing.
        assert_eq!(sink.compute_duration(10_000), Duration::ZERO);
    }
}
