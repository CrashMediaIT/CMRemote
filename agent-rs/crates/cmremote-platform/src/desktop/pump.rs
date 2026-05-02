// Source: CMRemote, clean-room implementation.

//! Per-session capture pump (slice R7.n.5).
//!
//! Drives a [`super::media::DesktopCapturer`] at a fixed cadence
//! and forwards every captured frame into a [`CaptureSink`]. Owns
//! its Tokio task, exposes live [`CaptureStats`], and halts itself
//! after a configurable number of consecutive errors so a
//! `NotSupported` capturer (e.g. on a non-Windows host) cannot burn
//! a CPU spinning on the same error forever.
//!
//! ## Pipeline
//!
//! ```text
//!   tokio::time::sleep(target_fps interval)
//!         │
//!         ▼
//!   capturer.capture_next_frame() ──► CaptureStats::record_frame
//!         │
//!         ▼
//!   sink.consume(frame) ──► CaptureStats::record_consume / record_drop
//!         │
//!         └─► consecutive errors counted; pump halts at threshold
//! ```
//!
//! ## Why a separate `CaptureSink` trait
//!
//! The capturer produces BGRA frames; the eventual production sink
//! is the [`super::media::VideoEncoder`] (H.264 / AV1) wrapped to
//! the [`CaptureSink`] shape. Slice R7.n.6 lands the real
//! `WindowsVideoEncoder`; until then [`DiscardingCaptureSink`]
//! counts and drops frames so the pump's lifecycle plumbing can be
//! exercised end-to-end without an encoder. Splitting the trait
//! keeps the pump's contract narrow (one async method), matches
//! the `Send + Sync` bound the WebRTC layer needs, and means
//! adding the encoder is a pure additive change.
//!
//! ## Threading
//!
//! - The pump owns one `tokio::task::JoinHandle<()>`.
//! - [`CaptureStats`] uses an `std::sync::Mutex` because every lock
//!   is held only across local field updates — never across an
//!   `await`. The sync mutex is cheap enough at 30 fps that the
//!   contention budget is invisible next to the capture+encode
//!   work.
//! - [`CapturePump::stop`] aborts the task and awaits it; the
//!   task itself is `cancel-safe` because the only `await` points
//!   are `tokio::time::sleep` (cancellable) and the capturer/sink
//!   calls (which the task itself owns).
//!
//! ## Security
//!
//! The pump never logs frame contents, never echoes operator
//! identifiers, and never includes a session id in the
//! `tracing::warn!` events — those are the caller's responsibility
//! (the WebRTC transport adds the session id field via the
//! [`tracing::Span`] it spawns the pump under).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::task::JoinHandle;

use super::media::{CapturedFrame, DesktopCapturer, DesktopMediaError};

// ---------------------------------------------------------------------------
// CaptureSink trait
// ---------------------------------------------------------------------------

/// Consumer of captured frames.
///
/// Implementations are expected to either encode the frame and
/// forward the resulting [`super::media::EncodedVideoChunk`] onto a
/// WebRTC video track, or to drop the frame deliberately (e.g.
/// [`DiscardingCaptureSink`] until the encoder lands).
///
/// `consume` returns a [`DesktopMediaError`] on backpressure or a
/// recoverable encoder error; the pump records the error in
/// [`CaptureStats::last_sink_error`] but does **not** halt on a
/// single failure (the encoder may catch up on the next frame).
/// Permanent errors should be reported as [`DesktopMediaError::Io`]
/// and the pump's `max_consecutive_errors` budget will eventually
/// halt the loop.
#[async_trait]
pub trait CaptureSink: Send + Sync {
    /// Take ownership of `frame` and forward it. Implementations
    /// MUST NOT panic on invalid frame data — return
    /// [`DesktopMediaError::InvalidParameters`] instead.
    async fn consume(&self, frame: CapturedFrame) -> Result<(), DesktopMediaError>;
}

/// Drops every frame after recording its size; the default sink
/// used by the pump until [`super::media::VideoEncoder`] gets a
/// `CaptureSink` adapter.
///
/// Useful in production too: a session that hasn't completed SDP
/// negotiation has no track to push to, so dropping is the only
/// correct behaviour.
#[derive(Debug, Default, Clone)]
pub struct DiscardingCaptureSink {
    /// Number of frames consumed (drop-counted). Atomic-ish via the
    /// inner `Mutex` so concurrent observers see a consistent value.
    consumed: Arc<Mutex<u64>>,
}

impl DiscardingCaptureSink {
    /// Build a fresh sink with `consumed == 0`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of frames dropped since construction.
    pub fn frames_dropped(&self) -> u64 {
        *self
            .consumed
            .lock()
            .expect("DiscardingCaptureSink mutex poisoned")
    }
}

#[async_trait]
impl CaptureSink for DiscardingCaptureSink {
    async fn consume(&self, _frame: CapturedFrame) -> Result<(), DesktopMediaError> {
        let mut g = self
            .consumed
            .lock()
            .expect("DiscardingCaptureSink mutex poisoned");
        *g = g.saturating_add(1);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LateBoundCaptureSink — slice R7.n.6.
// ---------------------------------------------------------------------------

/// `CaptureSink` whose downstream is bound *after* construction.
///
/// The slice R7.n.5 capture pump is spawned by the WebRTC driver as
/// soon as `RemoteControl` opens the session, but the per-session
/// downstream sink (encoder + WebRTC video track) is built later
/// when the peer connection is constructed (on
/// `ProvideIceServers` / `SendSdpOffer`). This adapter lets the
/// pump start immediately and have its frames quietly dropped
/// (and counted) until [`Self::bind`] swaps in the real downstream.
///
/// ## Threading
///
/// Uses a [`std::sync::RwLock`] over the inner `Arc` slot. The lock
/// is held only for the duration of a `clone()` of the inner `Arc`
/// (or a `replace`) — never across an `await` — so the read-side
/// contention stays comparable to a plain `Mutex` clone of an
/// `Arc`. This intentionally avoids `tokio::sync::RwLock` because
/// the pump's `consume` path cannot safely await another task while
/// holding a lock that the egress / signalling path may need.
///
/// ## Counters
///
/// - [`Self::dropped_before_bind`] counts frames dropped because no
///   downstream was bound. Useful for the runtime audit log so an
///   operator can tell how much capture work was wasted between
///   `RemoteControl` and the first `RTCPeerConnection` build.
/// - Errors from the bound downstream propagate to the pump
///   verbatim; the late-bound sink does **not** swallow them.
#[derive(Default)]
pub struct LateBoundCaptureSink {
    inner: std::sync::RwLock<Option<Arc<dyn CaptureSink>>>,
    dropped_before_bind: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for LateBoundCaptureSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The wrapped `dyn CaptureSink` has no `Debug` bound — and
        // a real downstream may be the WebRTC video track sink,
        // whose Debug impl would touch the upstream RTP packetizer
        // state. Report only the bookkeeping shape.
        f.debug_struct("LateBoundCaptureSink")
            .field("bound", &self.is_bound())
            .field("dropped_before_bind", &self.dropped_before_bind())
            .finish()
    }
}

impl LateBoundCaptureSink {
    /// Build a sink with no downstream — every `consume` drops the
    /// frame and bumps [`Self::dropped_before_bind`] until
    /// [`Self::bind`] is called.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install (or replace) the downstream sink. Subsequent
    /// `consume` calls forward into `sink` until [`Self::unbind`]
    /// or another `bind` is invoked. Cheap — one short write-lock
    /// over an `Option<Arc<…>>` swap.
    pub fn bind(&self, sink: Arc<dyn CaptureSink>) {
        let mut g = self
            .inner
            .write()
            .expect("LateBoundCaptureSink rwlock poisoned");
        *g = Some(sink);
    }

    /// Drop the downstream sink, returning to the "drop and count"
    /// behaviour. Used by the WebRTC driver when the peer
    /// connection is closed but the underlying session is still
    /// live (e.g. between `change_windows_session` and the next
    /// `ProvideIceServers`).
    pub fn unbind(&self) {
        let mut g = self
            .inner
            .write()
            .expect("LateBoundCaptureSink rwlock poisoned");
        *g = None;
    }

    /// Number of frames dropped because no downstream was bound at
    /// the time. Monotonically increases for the lifetime of the
    /// sink; never reset by `bind` / `unbind`.
    ///
    /// Read with [`Ordering::Relaxed`] because the counter is
    /// telemetry-only — it never gates control flow elsewhere, so
    /// no happens-before edge with another atomic is required.
    /// The counter itself is monotonically increasing, so a
    /// possibly-stale read just under-reports for one observation
    /// window; the next read picks up the rest.
    pub fn dropped_before_bind(&self) -> u64 {
        self.dropped_before_bind
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `true` when a downstream is currently bound.
    pub fn is_bound(&self) -> bool {
        self.inner
            .read()
            .expect("LateBoundCaptureSink rwlock poisoned")
            .is_some()
    }
}

#[async_trait]
impl CaptureSink for LateBoundCaptureSink {
    async fn consume(&self, frame: CapturedFrame) -> Result<(), DesktopMediaError> {
        // Clone the inner `Arc` under the read lock and release the
        // lock *before* awaiting the downstream — the downstream's
        // `consume` may itself take an internal lock, so holding
        // ours across the await would risk a cross-lock deadlock
        // with the WebRTC driver's PC-build path.
        let bound = {
            let g = self
                .inner
                .read()
                .expect("LateBoundCaptureSink rwlock poisoned");
            g.clone()
        };
        match bound {
            Some(sink) => sink.consume(frame).await,
            None => {
                self.dropped_before_bind
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CaptureStats
// ---------------------------------------------------------------------------

/// Live counters + last-error window for one [`CapturePump`].
///
/// Cheap to clone (`Arc` to a sync mutex). Take a stable snapshot
/// via [`CaptureStats::snapshot`] for logging or wire emission;
/// never serialise the live handle.
#[derive(Debug, Clone)]
pub struct CaptureStats {
    inner: Arc<Mutex<CaptureStatsInner>>,
}

/// Plain-data view of [`CaptureStats`] at one moment in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureStatsSnapshot {
    /// `tokio::time::Instant` when [`CapturePump::start`] was called.
    pub started_at: Instant,
    /// `tokio::time::Instant` when [`CapturePump::stop`] completed,
    /// or `None` while the pump is still running.
    pub stopped_at: Option<Instant>,
    /// Frames the capturer produced (success returns from
    /// `capture_next_frame`).
    pub frames_captured: u64,
    /// Frames the sink accepted (`consume` returned `Ok`).
    pub frames_consumed: u64,
    /// Frames the sink rejected with `Err` (capture succeeded;
    /// downstream backpressure / encoder error).
    pub frames_dropped: u64,
    /// Total errors returned by the capturer (every call site).
    pub capture_errors: u64,
    /// Total errors returned by the sink.
    pub sink_errors: u64,
    /// `Some(_)` instant of the most recent successful capture.
    pub last_frame_at: Option<Instant>,
    /// Most recent capture-error message, opaque (carries no
    /// frame contents).
    pub last_capture_error: Option<String>,
    /// Most recent sink-error message.
    pub last_sink_error: Option<String>,
}

#[derive(Debug)]
struct CaptureStatsInner {
    started_at: Instant,
    stopped_at: Option<Instant>,
    frames_captured: u64,
    frames_consumed: u64,
    frames_dropped: u64,
    capture_errors: u64,
    sink_errors: u64,
    last_frame_at: Option<Instant>,
    last_capture_error: Option<String>,
    last_sink_error: Option<String>,
}

impl CaptureStats {
    fn new(started_at: Instant) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CaptureStatsInner {
                started_at,
                stopped_at: None,
                frames_captured: 0,
                frames_consumed: 0,
                frames_dropped: 0,
                capture_errors: 0,
                sink_errors: 0,
                last_frame_at: None,
                last_capture_error: None,
                last_sink_error: None,
            })),
        }
    }

    /// Stable plain-data snapshot of the current counters.
    pub fn snapshot(&self) -> CaptureStatsSnapshot {
        let g = self.inner.lock().expect("CaptureStats mutex poisoned");
        CaptureStatsSnapshot {
            started_at: g.started_at,
            stopped_at: g.stopped_at,
            frames_captured: g.frames_captured,
            frames_consumed: g.frames_consumed,
            frames_dropped: g.frames_dropped,
            capture_errors: g.capture_errors,
            sink_errors: g.sink_errors,
            last_frame_at: g.last_frame_at,
            last_capture_error: g.last_capture_error.clone(),
            last_sink_error: g.last_sink_error.clone(),
        }
    }

    fn record_capture(&self) {
        let now = Instant::now();
        let mut g = self.inner.lock().expect("CaptureStats mutex poisoned");
        g.frames_captured = g.frames_captured.saturating_add(1);
        g.last_frame_at = Some(now);
    }

    fn record_consume(&self) {
        let mut g = self.inner.lock().expect("CaptureStats mutex poisoned");
        g.frames_consumed = g.frames_consumed.saturating_add(1);
    }

    fn record_drop(&self) {
        let mut g = self.inner.lock().expect("CaptureStats mutex poisoned");
        g.frames_dropped = g.frames_dropped.saturating_add(1);
    }

    fn record_capture_error(&self, err: &DesktopMediaError) {
        let mut g = self.inner.lock().expect("CaptureStats mutex poisoned");
        g.capture_errors = g.capture_errors.saturating_add(1);
        g.last_capture_error = Some(err.to_string());
    }

    fn record_sink_error(&self, err: &DesktopMediaError) {
        let mut g = self.inner.lock().expect("CaptureStats mutex poisoned");
        g.sink_errors = g.sink_errors.saturating_add(1);
        g.last_sink_error = Some(err.to_string());
    }

    fn record_stopped(&self) {
        let mut g = self.inner.lock().expect("CaptureStats mutex poisoned");
        if g.stopped_at.is_none() {
            g.stopped_at = Some(Instant::now());
        }
    }
}

// ---------------------------------------------------------------------------
// CapturePumpConfig
// ---------------------------------------------------------------------------

/// Tunables for [`CapturePump::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePumpConfig {
    /// Target frame rate. Pinned at construction; renegotiation
    /// would require stopping and restarting the pump.
    pub target_fps: u32,
    /// Pump halts itself once this many *consecutive* errors are
    /// observed (capture-side or sink-side, mixed counts together).
    /// A successful capture-and-consume cycle resets the counter.
    pub max_consecutive_errors: u32,
    /// Sleep inserted between two consecutive failed cycles. Caps
    /// the wasted-CPU budget when the underlying capturer is
    /// permanently broken (e.g. `NotSupported`).
    pub error_backoff: Duration,
}

impl Default for CapturePumpConfig {
    fn default() -> Self {
        Self {
            target_fps: 30,
            // 30 consecutive errors at 30 fps + 100 ms backoff = at
            // most ~3 seconds of wasted-CPU before the pump halts.
            max_consecutive_errors: 30,
            error_backoff: Duration::from_millis(100),
        }
    }
}

impl CapturePumpConfig {
    /// Frame interval implied by [`Self::target_fps`]. A
    /// `target_fps == 0` collapses to a 1-second tick — the pump
    /// degrades gracefully rather than panicking on a bad config.
    pub fn frame_interval(&self) -> Duration {
        if self.target_fps == 0 {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(1) / self.target_fps
        }
    }
}

// ---------------------------------------------------------------------------
// CapturePump
// ---------------------------------------------------------------------------

/// Owning handle for the capture-pump task.
///
/// Drop or call [`CapturePump::stop`] to terminate the loop; the
/// underlying `JoinHandle::abort` runs at drop time too, but
/// `stop().await` is preferred so the caller observes a final
/// stats snapshot.
#[derive(Debug)]
pub struct CapturePump {
    /// `Option` so [`Self::stop`] can `take` the handle out without
    /// fighting the [`Drop`] impl below — the Drop impl only acts
    /// when the handle is still present.
    join: Option<JoinHandle<()>>,
    stats: CaptureStats,
}

impl CapturePump {
    /// Spawn the pump on the current Tokio runtime.
    ///
    /// Returns immediately; the pump runs until [`Self::stop`] is
    /// called, the [`CapturePumpConfig::max_consecutive_errors`]
    /// budget is exceeded, or the task is aborted (drop / abort).
    pub fn start(
        capturer: Arc<dyn DesktopCapturer>,
        sink: Arc<dyn CaptureSink>,
        config: CapturePumpConfig,
    ) -> Self {
        let stats = CaptureStats::new(Instant::now());
        let pump_stats = stats.clone();
        let join = tokio::spawn(async move {
            run_pump(capturer, sink, config, pump_stats).await;
        });
        Self {
            join: Some(join),
            stats,
        }
    }

    /// Stop the pump and await termination. Final stats are
    /// recorded before returning.
    pub async fn stop(mut self) -> CaptureStatsSnapshot {
        if let Some(join) = self.join.take() {
            join.abort();
            // Best-effort join — `abort` may have already been
            // honoured by the runtime before we reach here. We
            // swallow `JoinError` because the pump's own halt path
            // also drops the handle through this method (a
            // Cancelled error is the expected shape).
            let _ = join.await;
        }
        self.stats.record_stopped();
        self.stats.snapshot()
    }

    /// Live stats handle (cheap clone).
    pub fn stats(&self) -> CaptureStats {
        self.stats.clone()
    }

    /// `true` while the underlying task has not finished. Useful in
    /// tests to verify the halt-on-errors path.
    pub fn is_running(&self) -> bool {
        self.join
            .as_ref()
            .map(|j| !j.is_finished())
            .unwrap_or(false)
    }
}

impl Drop for CapturePump {
    fn drop(&mut self) {
        // Defensive: if the owner forgot to call `stop`, abort the
        // task so it doesn't outlive the pump handle. The task is
        // cancel-safe (every `await` is on a Tokio primitive that
        // handles abort cleanly). When `stop` already consumed the
        // handle, `take` returns `None` and we do nothing here.
        if let Some(join) = self.join.take() {
            join.abort();
            self.stats.record_stopped();
        }
    }
}

/// The pump's body — split out so it can be reused under test.
async fn run_pump(
    capturer: Arc<dyn DesktopCapturer>,
    sink: Arc<dyn CaptureSink>,
    config: CapturePumpConfig,
    stats: CaptureStats,
) {
    let frame_interval = config.frame_interval();
    let mut next_tick = Instant::now();
    let mut consecutive_errors: u32 = 0;

    loop {
        // Pace: sleep until the next tick, then bump.
        let now = Instant::now();
        if next_tick > now {
            tokio::time::sleep(next_tick - now).await;
        }
        next_tick += frame_interval;

        let capture_outcome = capturer.capture_next_frame().await;
        let frame = match capture_outcome {
            Ok(f) => {
                stats.record_capture();
                f
            }
            Err(e) => {
                stats.record_capture_error(&e);
                consecutive_errors = consecutive_errors.saturating_add(1);
                if consecutive_errors >= config.max_consecutive_errors {
                    tracing::warn!(
                        capture_errors = consecutive_errors,
                        "capture pump halting after consecutive capture errors",
                    );
                    break;
                }
                tokio::time::sleep(config.error_backoff).await;
                continue;
            }
        };

        match sink.consume(frame).await {
            Ok(()) => {
                stats.record_consume();
                consecutive_errors = 0;
            }
            Err(e) => {
                stats.record_drop();
                stats.record_sink_error(&e);
                consecutive_errors = consecutive_errors.saturating_add(1);
                if consecutive_errors >= config.max_consecutive_errors {
                    tracing::warn!(
                        sink_errors = consecutive_errors,
                        "capture pump halting after consecutive sink errors",
                    );
                    break;
                }
                tokio::time::sleep(config.error_backoff).await;
            }
        }
    }

    stats.record_stopped();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostOs;
    use std::collections::VecDeque;
    use tokio::sync::Notify;

    /// Capturer that yields a fixed number of pre-built frames, then
    /// returns `Io("exhausted")` on every subsequent call.
    struct ScriptedCapturer {
        frames: Mutex<VecDeque<CapturedFrame>>,
    }

    impl ScriptedCapturer {
        fn new(n: usize) -> Self {
            let mut frames = VecDeque::new();
            for i in 0..n {
                let mut f = CapturedFrame::black(8, 8).unwrap();
                f.timestamp_micros = i as u64;
                frames.push_back(f);
            }
            Self {
                frames: Mutex::new(frames),
            }
        }
    }

    #[async_trait]
    impl DesktopCapturer for ScriptedCapturer {
        async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
            let mut g = self.frames.lock().unwrap();
            g.pop_front()
                .ok_or_else(|| DesktopMediaError::Io("exhausted".into()))
        }
    }

    /// Capturer that always errors with `NotSupported(host_os)`.
    struct AlwaysErrorCapturer {
        host_os: HostOs,
    }

    #[async_trait]
    impl DesktopCapturer for AlwaysErrorCapturer {
        async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
            Err(DesktopMediaError::NotSupported(self.host_os))
        }
    }

    /// Capturer that yields one frame and then blocks forever; used
    /// to verify `CapturePump::stop` actually aborts an in-flight
    /// `await`.
    struct BlockingAfterFirstCapturer {
        first: Mutex<Option<CapturedFrame>>,
    }

    impl BlockingAfterFirstCapturer {
        fn new() -> Self {
            Self {
                first: Mutex::new(Some(CapturedFrame::black(4, 4).unwrap())),
            }
        }
    }

    #[async_trait]
    impl DesktopCapturer for BlockingAfterFirstCapturer {
        async fn capture_next_frame(&self) -> Result<CapturedFrame, DesktopMediaError> {
            if let Some(f) = self.first.lock().unwrap().take() {
                return Ok(f);
            }
            // Block forever; only `JoinHandle::abort` gets us out.
            let n = Notify::new();
            n.notified().await;
            unreachable!()
        }
    }

    /// Sink that fails its first N consume calls then succeeds.
    struct FlakySink {
        remaining_failures: Mutex<u32>,
    }

    impl FlakySink {
        fn new(failures: u32) -> Self {
            Self {
                remaining_failures: Mutex::new(failures),
            }
        }
    }

    #[async_trait]
    impl CaptureSink for FlakySink {
        async fn consume(&self, _frame: CapturedFrame) -> Result<(), DesktopMediaError> {
            let mut g = self.remaining_failures.lock().unwrap();
            if *g > 0 {
                *g -= 1;
                Err(DesktopMediaError::Io("flaky-sink".into()))
            } else {
                Ok(())
            }
        }
    }

    fn fast_config() -> CapturePumpConfig {
        // 1000 fps + 1 ms backoff so tests finish in milliseconds.
        CapturePumpConfig {
            target_fps: 1000,
            max_consecutive_errors: 5,
            error_backoff: Duration::from_millis(1),
        }
    }

    #[test]
    fn discarding_sink_increments_count() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let sink = DiscardingCaptureSink::new();
            for _ in 0..3 {
                sink.consume(CapturedFrame::black(2, 2).unwrap())
                    .await
                    .unwrap();
            }
            assert_eq!(sink.frames_dropped(), 3);
        });
    }

    #[test]
    fn config_frame_interval_is_safe_at_zero_fps() {
        let c = CapturePumpConfig {
            target_fps: 0,
            ..CapturePumpConfig::default()
        };
        assert_eq!(c.frame_interval(), Duration::from_secs(1));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn pump_captures_scripted_frames_and_records_stats() {
        let cap: Arc<dyn DesktopCapturer> = Arc::new(ScriptedCapturer::new(4));
        let counter = Arc::new(DiscardingCaptureSink::new());
        let sink: Arc<dyn CaptureSink> = counter.clone();
        let pump = CapturePump::start(cap, sink, fast_config());

        // Drive the paused-time runtime forward enough for the pump
        // to consume all 4 frames + halt on the resulting capture
        // errors (5 max_consecutive_errors).
        for _ in 0..30 {
            tokio::time::advance(Duration::from_millis(2)).await;
            tokio::task::yield_now().await;
        }
        let snap = pump.stop().await;

        assert_eq!(snap.frames_captured, 4, "captured: {snap:?}");
        assert_eq!(snap.frames_consumed, 4, "consumed: {snap:?}");
        assert_eq!(snap.frames_dropped, 0);
        assert_eq!(counter.frames_dropped(), 4);
        assert!(snap.last_capture_error.is_some(), "{snap:?}");
        assert!(snap
            .last_capture_error
            .as_deref()
            .unwrap()
            .contains("exhausted"));
        assert!(snap.stopped_at.is_some());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn pump_halts_after_consecutive_capture_errors() {
        let cap: Arc<dyn DesktopCapturer> = Arc::new(AlwaysErrorCapturer {
            host_os: HostOs::Linux,
        });
        let sink: Arc<dyn CaptureSink> = Arc::new(DiscardingCaptureSink::new());
        let pump = CapturePump::start(cap, sink, fast_config());

        for _ in 0..30 {
            tokio::time::advance(Duration::from_millis(2)).await;
            tokio::task::yield_now().await;
        }
        // After the budget is exhausted the task must finish on its
        // own without `stop()` being called.
        for _ in 0..30 {
            if !pump.is_running() {
                break;
            }
            tokio::time::advance(Duration::from_millis(2)).await;
            tokio::task::yield_now().await;
        }
        assert!(!pump.is_running(), "pump should have halted");
        let snap = pump.stop().await;
        assert_eq!(snap.frames_captured, 0);
        assert!(snap.capture_errors >= fast_config().max_consecutive_errors as u64);
        assert!(snap
            .last_capture_error
            .as_deref()
            .unwrap()
            .to_lowercase()
            .contains("not supported"));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn pump_recovers_from_transient_sink_errors() {
        // Sink fails the first 2 frames then succeeds; with
        // `max_consecutive_errors = 5` the pump must reset the
        // counter on the first success and continue.
        let cap: Arc<dyn DesktopCapturer> = Arc::new(ScriptedCapturer::new(6));
        let flaky = Arc::new(FlakySink::new(2));
        let sink: Arc<dyn CaptureSink> = flaky.clone();
        let pump = CapturePump::start(cap, sink, fast_config());

        for _ in 0..40 {
            tokio::time::advance(Duration::from_millis(2)).await;
            tokio::task::yield_now().await;
        }
        let snap = pump.stop().await;

        assert_eq!(snap.frames_captured, 6, "{snap:?}");
        assert_eq!(snap.frames_consumed, 4, "{snap:?}");
        assert_eq!(snap.frames_dropped, 2, "{snap:?}");
        assert_eq!(snap.sink_errors, 2);
        assert!(snap.last_sink_error.as_deref().unwrap().contains("flaky"));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn stop_aborts_in_flight_capture_await() {
        let cap: Arc<dyn DesktopCapturer> = Arc::new(BlockingAfterFirstCapturer::new());
        let sink: Arc<dyn CaptureSink> = Arc::new(DiscardingCaptureSink::new());
        let pump = CapturePump::start(cap, sink, fast_config());

        // Let the first frame land, then stop while the capturer
        // task is parked inside `Notify::notified().await`. The
        // stop call must return promptly — no hanging.
        for _ in 0..10 {
            tokio::time::advance(Duration::from_millis(2)).await;
            tokio::task::yield_now().await;
            if pump.stats().snapshot().frames_captured >= 1 {
                break;
            }
        }
        let snap = pump.stop().await;
        assert!(snap.frames_captured >= 1);
        assert!(snap.stopped_at.is_some());
    }

    #[test]
    fn stats_snapshot_starts_zeroed() {
        let s = CaptureStats::new(Instant::now());
        let snap = s.snapshot();
        assert_eq!(snap.frames_captured, 0);
        assert_eq!(snap.frames_consumed, 0);
        assert_eq!(snap.frames_dropped, 0);
        assert!(snap.stopped_at.is_none());
        assert!(snap.last_capture_error.is_none());
    }

    #[test]
    fn drop_aborts_task_without_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let cap: Arc<dyn DesktopCapturer> = Arc::new(BlockingAfterFirstCapturer::new());
            let sink: Arc<dyn CaptureSink> = Arc::new(DiscardingCaptureSink::new());
            let pump = CapturePump::start(cap, sink, fast_config());
            // Drop without calling `stop`. Must not panic; abort
            // happens in the Drop impl.
            drop(pump);
            // Spin once so the runtime sees the abort.
            tokio::task::yield_now().await;
        });
    }

    // ---------------------------------------------------------------
    // LateBoundCaptureSink — slice R7.n.6.
    // ---------------------------------------------------------------

    fn frame() -> CapturedFrame {
        CapturedFrame::black(2, 2).unwrap()
    }

    #[tokio::test]
    async fn late_bound_sink_drops_and_counts_before_bind() {
        let s = LateBoundCaptureSink::new();
        assert!(!s.is_bound());
        for _ in 0..3 {
            s.consume(frame()).await.unwrap();
        }
        assert_eq!(s.dropped_before_bind(), 3);
    }

    #[tokio::test]
    async fn late_bound_sink_forwards_after_bind() {
        let s = LateBoundCaptureSink::new();
        let down = Arc::new(DiscardingCaptureSink::new());
        s.bind(down.clone() as Arc<dyn CaptureSink>);
        assert!(s.is_bound());
        for _ in 0..4 {
            s.consume(frame()).await.unwrap();
        }
        assert_eq!(down.frames_dropped(), 4);
        // No frames should be counted as "dropped before bind" once
        // a downstream is wired.
        assert_eq!(s.dropped_before_bind(), 0);
    }

    #[tokio::test]
    async fn late_bound_sink_unbind_returns_to_drop_and_count() {
        let s = LateBoundCaptureSink::new();
        let down = Arc::new(DiscardingCaptureSink::new());
        s.bind(down.clone() as Arc<dyn CaptureSink>);
        s.consume(frame()).await.unwrap();
        s.unbind();
        assert!(!s.is_bound());
        s.consume(frame()).await.unwrap();
        assert_eq!(down.frames_dropped(), 1);
        assert_eq!(s.dropped_before_bind(), 1);
    }

    #[tokio::test]
    async fn late_bound_sink_propagates_downstream_errors() {
        struct Failing;
        #[async_trait]
        impl CaptureSink for Failing {
            async fn consume(&self, _frame: CapturedFrame) -> Result<(), DesktopMediaError> {
                Err(DesktopMediaError::Io("downstream broken".into()))
            }
        }
        let s = LateBoundCaptureSink::new();
        s.bind(Arc::new(Failing));
        let e = s.consume(frame()).await.unwrap_err();
        assert!(format!("{e}").contains("downstream broken"));
    }

    #[tokio::test]
    async fn late_bound_sink_replace_swaps_downstream() {
        let s = LateBoundCaptureSink::new();
        let first = Arc::new(DiscardingCaptureSink::new());
        let second = Arc::new(DiscardingCaptureSink::new());
        s.bind(first.clone() as Arc<dyn CaptureSink>);
        s.consume(frame()).await.unwrap();
        // Replace the downstream — subsequent frames go to `second`.
        s.bind(second.clone() as Arc<dyn CaptureSink>);
        s.consume(frame()).await.unwrap();
        s.consume(frame()).await.unwrap();
        assert_eq!(first.frames_dropped(), 1);
        assert_eq!(second.frames_dropped(), 2);
    }
}
