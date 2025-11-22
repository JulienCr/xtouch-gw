//! Pitch bend squelch module
//!
//! Prevents motorized fader feedback loops by suppressing incoming pitch bend messages
//! temporarily after sending fader position commands.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Pitch bend squelch mechanism
///
/// Maintains a timestamp until which incoming pitchbend messages should be suppressed.
#[derive(Clone)]
pub struct PitchBendSquelch {
    /// Monotonic start time for relative timestamp calculation
    start_instant: Instant,

    /// Suppress pitch bend until this timestamp (milliseconds since start_instant)
    suppress_until_ms: Arc<AtomicU64>,
}

impl PitchBendSquelch {
    /// Create a new pitch bend squelch
    pub fn new() -> Self {
        Self {
            start_instant: Instant::now(),
            suppress_until_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get current time in milliseconds (monotonic)
    fn current_time_ms(&self) -> u64 {
        self.start_instant.elapsed().as_millis() as u64
    }

    /// Squelch pitch bend for the specified duration
    ///
    /// This extends the squelch window using max(), never shortening it.
    pub fn squelch(&self, duration_ms: u64) {
        let target_time = self.current_time_ms() + duration_ms;

        // Atomically update to max of current and target
        self.suppress_until_ms
            .fetch_max(target_time, Ordering::Relaxed);
    }

    /// Check if pitch bend is currently squelched
    pub fn is_squelched(&self) -> bool {
        let now = self.current_time_ms();
        let until = self.suppress_until_ms.load(Ordering::Relaxed);
        now < until
    }
}

impl Default for PitchBendSquelch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_squelch_basic() {
        let squelch = PitchBendSquelch::new();

        // Initially not squelched
        assert!(!squelch.is_squelched());

        // Squelch for 100ms
        squelch.squelch(100);
        assert!(squelch.is_squelched());

        // Wait for squelch to expire
        thread::sleep(Duration::from_millis(150));
        assert!(!squelch.is_squelched());
    }

    #[test]
    fn test_squelch_extends_window() {
        let squelch = PitchBendSquelch::new();

        // Squelch for 50ms
        squelch.squelch(50);
        assert!(squelch.is_squelched());

        // Wait 30ms (still within window)
        thread::sleep(Duration::from_millis(30));
        assert!(squelch.is_squelched());

        // Extend squelch by another 100ms (total window is now ~120ms from start)
        squelch.squelch(100);
        assert!(squelch.is_squelched());

        // Wait another 50ms (total ~80ms from start, still squelched)
        thread::sleep(Duration::from_millis(50));
        assert!(squelch.is_squelched());

        // Wait another 80ms (total ~160ms from start, should be clear)
        thread::sleep(Duration::from_millis(80));
        assert!(!squelch.is_squelched());
    }

    #[test]
    fn test_squelch_zero_duration() {
        let squelch = PitchBendSquelch::new();

        // Squelch for 0ms should not squelch
        squelch.squelch(0);

        // Should effectively be unsquelched immediately
        assert!(!squelch.is_squelched());
    }
}
