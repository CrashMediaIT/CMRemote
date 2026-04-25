// Source: CMRemote, clean-room implementation.

//! Adapter that wraps a [`super::media::VideoEncoder`] as a
//! [`super::pump::CaptureSink`] (slice R7.n.6).
//!
//! The capture pump (slice R7.n.5) pushes BGRA
//! [`super::media::CapturedFrame`]s into a `CaptureSink`. Every
//! production deployment encodes those frames before forwarding
//! them onto a WebRTC video track; this adapter bridges the two
//! traits with no per-OS code:
//!
//! ```text
//!   CapturePump ──► EncoderCaptureSink ──► VideoEncoder ──► EncodedChunkSink
//!   (BGRA frames)        (this module)      (per-OS encoder)   (WebRTC track)
//! ```
//!
//! Splitting the downstream "where does the encoded chunk go"
//! into its own [`EncodedChunkSink`] trait keeps the encoder
//! decoupled from the transport: until slice R7.l/R7.m wires the
//! WebRTC track-builder, [`DiscardingEncodedChunkSink`] counts
//! bytes and drops chunks. Once the track-builder lands, the
//! Windows runtime swaps in a `WebRtcVideoTrackSink` with no
//! changes to either the encoder or the pump.
//!
//! ## Error handling
//!
//! - A capturer-side error never reaches this adapter (the pump
//!   handles capture errors itself).
//! - A `VideoEncoder::encode` error is recorded by the pump as a
//!   *sink* error (the pump treats this adapter as the sink) and
//!   counts toward the `max_consecutive_errors` halt budget.
//! - A `EncodedChunkSink::consume` error is mapped to the same
//!   [`super::media::DesktopMediaError`] shape so the pump's
//!   error-budget logic is uniform.

use std::sync::Arc;

use async_trait::async_trait;

use super::media::{CapturedFrame, DesktopMediaError, EncodedVideoChunk, VideoEncoder};
use super::pump::CaptureSink;

/// Consumer of encoded video chunks.
///
/// Implementations push the chunk onto an `RTCRtpSender` track, a
/// recording file, or a test harness. Errors flow back through
/// [`super::media::DesktopMediaError`] so the pump's halt-budget
/// logic stays uniform across capture / encode / transport.
#[async_trait]
pub trait EncodedChunkSink: Send + Sync {
    /// Take ownership of `chunk` and forward it.
    async fn consume(&self, chunk: EncodedVideoChunk) -> Result<(), DesktopMediaError>;
}

/// Drops every chunk after recording its length and keyframe
/// flag. The default downstream sink until slice R7.l/R7.m wires
/// the WebRTC video track.
#[derive(Debug, Default)]
pub struct DiscardingEncodedChunkSink {
    inner: std::sync::Mutex<DiscardingEncodedChunkSinkInner>,
}

#[derive(Debug, Default)]
struct DiscardingEncodedChunkSinkInner {
    chunks_dropped: u64,
    bytes_dropped: u64,
    keyframes_dropped: u64,
}

impl DiscardingEncodedChunkSink {
    /// Build a fresh sink with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Total chunks consumed (every consume call counts).
    pub fn chunks_dropped(&self) -> u64 {
        self.inner.lock().unwrap().chunks_dropped
    }
    /// Total encoded bytes consumed.
    pub fn bytes_dropped(&self) -> u64 {
        self.inner.lock().unwrap().bytes_dropped
    }
    /// Subset of [`Self::chunks_dropped`] that were keyframes.
    pub fn keyframes_dropped(&self) -> u64 {
        self.inner.lock().unwrap().keyframes_dropped
    }
}

#[async_trait]
impl EncodedChunkSink for DiscardingEncodedChunkSink {
    async fn consume(&self, chunk: EncodedVideoChunk) -> Result<(), DesktopMediaError> {
        let mut g = self.inner.lock().unwrap();
        g.chunks_dropped = g.chunks_dropped.saturating_add(1);
        g.bytes_dropped = g.bytes_dropped.saturating_add(chunk.bytes.len() as u64);
        if chunk.is_keyframe {
            g.keyframes_dropped = g.keyframes_dropped.saturating_add(1);
        }
        Ok(())
    }
}

/// `CaptureSink` adapter that encodes every incoming frame and
/// forwards the resulting [`EncodedVideoChunk`] to an
/// [`EncodedChunkSink`].
pub struct EncoderCaptureSink {
    encoder: Arc<dyn VideoEncoder>,
    downstream: Arc<dyn EncodedChunkSink>,
}

impl EncoderCaptureSink {
    /// Build the adapter.
    pub fn new(encoder: Arc<dyn VideoEncoder>, downstream: Arc<dyn EncodedChunkSink>) -> Self {
        Self {
            encoder,
            downstream,
        }
    }

    /// Forwarded helper so the runtime can request a keyframe in
    /// response to a viewer's Picture Loss Indication without
    /// having to keep a separate `Arc<dyn VideoEncoder>` around.
    pub fn request_keyframe(&self) {
        self.encoder.request_keyframe();
    }
}

#[async_trait]
impl CaptureSink for EncoderCaptureSink {
    async fn consume(&self, frame: CapturedFrame) -> Result<(), DesktopMediaError> {
        let chunk = self.encoder.encode(&frame).await?;
        self.downstream.consume(chunk).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Encoder that produces a fixed-size chunk per frame, marking
    /// every Nth chunk as a keyframe.
    struct StubEncoder {
        chunk_bytes: usize,
        keyframe_every: u64,
        seen: Mutex<u64>,
        force_keyframe: Mutex<bool>,
    }
    impl StubEncoder {
        fn new(chunk_bytes: usize, keyframe_every: u64) -> Self {
            Self {
                chunk_bytes,
                keyframe_every,
                seen: Mutex::new(0),
                force_keyframe: Mutex::new(false),
            }
        }
    }
    #[async_trait]
    impl VideoEncoder for StubEncoder {
        async fn encode(
            &self,
            frame: &CapturedFrame,
        ) -> Result<EncodedVideoChunk, DesktopMediaError> {
            let mut s = self.seen.lock().unwrap();
            *s += 1;
            let n = *s;
            let mut force = self.force_keyframe.lock().unwrap();
            let is_kf = *force || n % self.keyframe_every == 0;
            *force = false;
            Ok(EncodedVideoChunk {
                bytes: vec![0u8; self.chunk_bytes],
                timestamp_micros: frame.timestamp_micros,
                is_keyframe: is_kf,
            })
        }
        fn request_keyframe(&self) {
            *self.force_keyframe.lock().unwrap() = true;
        }
    }

    /// Encoder that always errors.
    struct FailEncoder;
    #[async_trait]
    impl VideoEncoder for FailEncoder {
        async fn encode(
            &self,
            _frame: &CapturedFrame,
        ) -> Result<EncodedVideoChunk, DesktopMediaError> {
            Err(DesktopMediaError::Io("encode failed".into()))
        }
        fn request_keyframe(&self) {}
    }

    /// Downstream sink that always errors — used to verify the
    /// adapter propagates downstream errors back to the pump.
    struct FailDownstream;
    #[async_trait]
    impl EncodedChunkSink for FailDownstream {
        async fn consume(&self, _chunk: EncodedVideoChunk) -> Result<(), DesktopMediaError> {
            Err(DesktopMediaError::Io("downstream broken".into()))
        }
    }

    fn frame() -> CapturedFrame {
        let mut f = CapturedFrame::black(2, 2).unwrap();
        f.timestamp_micros = 42;
        f
    }

    #[tokio::test]
    async fn discarding_chunk_sink_counts_bytes_and_keyframes() {
        let s = DiscardingEncodedChunkSink::new();
        s.consume(EncodedVideoChunk {
            bytes: vec![0; 10],
            timestamp_micros: 0,
            is_keyframe: true,
        })
        .await
        .unwrap();
        s.consume(EncodedVideoChunk {
            bytes: vec![0; 5],
            timestamp_micros: 0,
            is_keyframe: false,
        })
        .await
        .unwrap();
        assert_eq!(s.chunks_dropped(), 2);
        assert_eq!(s.bytes_dropped(), 15);
        assert_eq!(s.keyframes_dropped(), 1);
    }

    #[tokio::test]
    async fn adapter_forwards_encoded_chunk_to_downstream() {
        let enc: Arc<dyn VideoEncoder> = Arc::new(StubEncoder::new(7, 3));
        let down = Arc::new(DiscardingEncodedChunkSink::new());
        let sink = EncoderCaptureSink::new(enc, down.clone());
        for _ in 0..6 {
            sink.consume(frame()).await.unwrap();
        }
        assert_eq!(down.chunks_dropped(), 6);
        assert_eq!(down.bytes_dropped(), 42);
        // Keyframes at positions 3 and 6.
        assert_eq!(down.keyframes_dropped(), 2);
    }

    #[tokio::test]
    async fn adapter_propagates_encoder_errors() {
        let enc: Arc<dyn VideoEncoder> = Arc::new(FailEncoder);
        let down = Arc::new(DiscardingEncodedChunkSink::new());
        let sink = EncoderCaptureSink::new(enc, down.clone());
        let e = sink.consume(frame()).await.unwrap_err();
        assert!(format!("{e}").contains("encode failed"));
        assert_eq!(down.chunks_dropped(), 0);
    }

    #[tokio::test]
    async fn adapter_propagates_downstream_errors() {
        let enc: Arc<dyn VideoEncoder> = Arc::new(StubEncoder::new(1, 1));
        let down: Arc<dyn EncodedChunkSink> = Arc::new(FailDownstream);
        let sink = EncoderCaptureSink::new(enc, down);
        let e = sink.consume(frame()).await.unwrap_err();
        assert!(format!("{e}").contains("downstream broken"));
    }

    #[tokio::test]
    async fn request_keyframe_flows_through_to_encoder() {
        let enc = Arc::new(StubEncoder::new(2, 1_000_000));
        let down = Arc::new(DiscardingEncodedChunkSink::new());
        let sink = EncoderCaptureSink::new(enc.clone(), down.clone());
        // First frame is not a keyframe (keyframe_every is huge);
        // after request_keyframe it should be.
        sink.consume(frame()).await.unwrap();
        assert_eq!(down.keyframes_dropped(), 0);
        sink.request_keyframe();
        sink.consume(frame()).await.unwrap();
        assert_eq!(down.keyframes_dropped(), 1);
    }
}
