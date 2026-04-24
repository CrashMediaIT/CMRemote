// Source: CMRemote, clean-room implementation.

//! Jittered exponential backoff for the agent's reconnect loop.
//!
//! The schedule is pinned by `docs/wire-protocol.md` ➜ *Connection
//! lifecycle* ➜ point 4: base **1 s**, factor **2**, cap **60 s**,
//! **full jitter**. The reconnect counter resets on every successful
//! handshake.
//!
//! "Full jitter" follows the AWS Architecture Blog formulation
//! (`sleep ∈ [0, base * 2^attempt]`, capped). It is the lowest-variance
//! of the common backoff strategies and has the helpful property that
//! the 0-second draw is always reachable, so a transient outage on a
//! large fleet does not produce a synchronised reconnect storm.

use std::time::Duration;

use rand::{rngs::StdRng, Rng, SeedableRng};

/// Base delay before the first retry. A new connection attempt is
/// always issued immediately; this is the upper bound of the **first**
/// retry's sleep distribution.
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// Hard cap on the upper bound of a sleep distribution, regardless of
/// how many failures have stacked up.
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// Doubling factor applied per failed attempt.
pub const BACKOFF_FACTOR: u32 = 2;

/// Stateful iterator yielding the *next* backoff sleep, with full
/// jitter applied per draw.
///
/// The `Backoff` is reset (`attempt` counter back to zero) on a
/// successful handshake so a long-running connection that finally
/// drops is treated as "first failure", not "twentieth failure".
#[derive(Debug)]
pub struct Backoff {
    attempt: u32,
    rng: StdRng,
}

impl Backoff {
    /// Build a `Backoff` seeded from the OS entropy source.
    pub fn new() -> Self {
        Self {
            attempt: 0,
            rng: StdRng::from_entropy(),
        }
    }

    /// Build a `Backoff` from a deterministic seed; intended for
    /// tests so we can pin the jitter sequence.
    pub fn from_seed(seed: u64) -> Self {
        Self {
            attempt: 0,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// Reset the attempt counter. Call after a successful handshake.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Number of failed attempts observed so far.
    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    /// Compute and return the upper bound of the current draw, the
    /// realised sleep, then advance the attempt counter so the next
    /// call doubles.
    pub fn next_sleep(&mut self) -> Duration {
        let upper = upper_bound(self.attempt);
        // Full jitter: sleep ∈ [0, upper). The half-open interval is
        // important: capping at `upper` inclusive would cluster draws
        // at the cap, which is exactly the synchronisation pathology
        // jitter is supposed to break.
        let nanos = if upper.is_zero() {
            0
        } else {
            self.rng.gen_range(0..upper.as_nanos() as u64)
        };
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_nanos(nanos)
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Upper bound (inclusive of the cap, exclusive of the random draw)
/// for a given attempt index.
fn upper_bound(attempt: u32) -> Duration {
    // base * 2^attempt, saturating at the cap. Use `checked_pow` on the
    // factor so attempt = 32 doesn't overflow before we clamp.
    let multiplier = BACKOFF_FACTOR.checked_pow(attempt).unwrap_or(u32::MAX);
    let bound = BACKOFF_BASE.saturating_mul(multiplier);
    if bound > BACKOFF_CAP {
        BACKOFF_CAP
    } else {
        bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_bound_doubles_until_cap() {
        assert_eq!(upper_bound(0), Duration::from_secs(1));
        assert_eq!(upper_bound(1), Duration::from_secs(2));
        assert_eq!(upper_bound(2), Duration::from_secs(4));
        assert_eq!(upper_bound(3), Duration::from_secs(8));
        assert_eq!(upper_bound(4), Duration::from_secs(16));
        assert_eq!(upper_bound(5), Duration::from_secs(32));
        // 64 > 60 → clamps to the cap.
        assert_eq!(upper_bound(6), BACKOFF_CAP);
    }

    #[test]
    fn upper_bound_stays_clamped_for_huge_attempts() {
        // Anything past the cap must keep returning the cap, not
        // overflow or wrap.
        assert_eq!(upper_bound(20), BACKOFF_CAP);
        assert_eq!(upper_bound(100), BACKOFF_CAP);
        assert_eq!(upper_bound(u32::MAX - 1), BACKOFF_CAP);
        assert_eq!(upper_bound(u32::MAX), BACKOFF_CAP);
    }

    #[test]
    fn next_sleep_stays_within_upper_bound() {
        let mut b = Backoff::from_seed(42);
        for attempt in 0..20 {
            let upper = upper_bound(attempt);
            let sleep = b.next_sleep();
            assert!(
                sleep < upper,
                "attempt {attempt}: sleep {sleep:?} >= upper {upper:?}"
            );
        }
    }

    #[test]
    fn reset_zeroes_the_counter() {
        let mut b = Backoff::from_seed(7);
        for _ in 0..5 {
            b.next_sleep();
        }
        assert_eq!(b.attempts(), 5);
        b.reset();
        assert_eq!(b.attempts(), 0);
        // Next draw is back to the [0, 1s) window.
        assert!(b.next_sleep() < Duration::from_secs(1));
    }

    #[test]
    fn deterministic_seeds_produce_deterministic_sequences() {
        let seq = |seed: u64| {
            let mut b = Backoff::from_seed(seed);
            (0..5).map(|_| b.next_sleep()).collect::<Vec<_>>()
        };
        let a = seq(99);
        let b = seq(99);
        assert_eq!(a, b);
    }

    #[test]
    fn jitter_is_actually_jittered() {
        // Two different seeds must produce at least one different
        // draw within the first few attempts; otherwise the rng is
        // not actually being consulted.
        let mut a = Backoff::from_seed(1);
        let mut b = Backoff::from_seed(2);
        let differs = (0..10).any(|_| a.next_sleep() != b.next_sleep());
        assert!(differs, "two different seeds yielded identical sequences");
    }
}
