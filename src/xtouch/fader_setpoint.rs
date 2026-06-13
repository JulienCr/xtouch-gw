//! Fader setpoint scheduler — resident-worker debounced motorized fader updates
//!
//! Implements the TypeScript reference's epoch-based anti-obsolescence system
//! with a long-running worker per channel instead of a fresh `tokio::spawn`
//! per MIDI event.
//!
//! ## Key features
//! - **Epoch-based cancellation**: a new setpoint invalidates older pending work.
//! - **One resident worker per channel**: lazily spawned on first `schedule()`;
//!   awakened via `tokio::sync::Notify` — no spawn-per-event.
//! - **Bounded apply channel**: `mpsc::channel(APPLY_CHANNEL_CAPACITY)`. The
//!   per-channel worker `send().await`s, so a brief consumer stall back-pressures
//!   just that channel instead of dropping the final (last-wins) value.
//! - **Debounced application**: 90 ms default, 0 ms for the 0/16383 extremes.
//!
//! ## Why this shape
//!
//! The previous implementation `tokio::spawn`'d an untracked `sleep(90 ms)` task
//! for every PitchBend message (peak ~1 kHz) and pushed onto an unbounded
//! channel. A USB stall on the X-Touch (Windows exclusive-mode recovery ≈250 ms)
//! left commands and tasks piling up. Audit issue #54.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, trace};

/// Bounded capacity for the apply-command channel. 64 is well above the steady
/// state (~8 channels × 1 apply per debounce window) and absorbs short USB
/// stalls without unbounded growth.
pub const APPLY_CHANNEL_CAPACITY: usize = 64;

/// Default debounce delay applied when the desired value is not at either
/// extreme of the fader range.
const DEBOUNCE_DELAY_MS: u64 = 90;

/// Command to apply a fader setpoint
#[derive(Debug, Clone)]
pub struct ApplySetpointCmd {
    pub channel: u8,
    pub value14: u16,
    pub epoch: u32,
}

/// State for a single fader channel
#[derive(Default)]
struct ChannelState {
    /// Desired 14-bit position (source of truth)
    desired14: u16,
    /// Epoch counter for anti-obsolescence (per-channel)
    epoch: u32,
    /// Page epoch when this setpoint was last updated. Used by `get_desired`
    /// to detect setpoints orphaned by a page change.
    page_epoch: u64,
    /// Per-call debounce override (`schedule`'s `delay_ms`). The worker
    /// `take()`s it on its next cycle (so it applies exactly once); `None`
    /// falls back to the extreme-aware default. Last-write-wins like
    /// `desired14`, so the most recent `schedule` for the channel decides.
    override_delay_ms: Option<u64>,
}

/// Resident worker for a single channel. Owns a `Notify` used to re-arm work
/// without a fresh `tokio::spawn` per call. The `JoinHandle` is aborted when
/// the parent `FaderSetpoint` is dropped.
struct ChannelWorker {
    notify: Arc<Notify>,
    handle: JoinHandle<()>,
}

impl Drop for ChannelWorker {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Fader setpoint scheduler with epoch-based anti-obsolescence
#[derive(Clone)]
pub struct FaderSetpoint {
    /// Per-channel state (keyed by channel 1-9)
    channels: Arc<RwLock<HashMap<u8, ChannelState>>>,
    /// Bounded sender for apply commands consumed by the main event loop.
    apply_tx: mpsc::Sender<ApplySetpointCmd>,
    /// Current page epoch (updated via `set_page_epoch`)
    current_page_epoch: Arc<RwLock<u64>>,
    /// Resident worker tasks, one per active channel. Lazily populated on the
    /// first `schedule()` call for a given channel.
    workers: Arc<std::sync::Mutex<HashMap<u8, ChannelWorker>>>,
}

impl FaderSetpoint {
    /// Create a new fader setpoint scheduler.
    ///
    /// Returns the scheduler and a bounded receiver for apply commands.
    pub fn new() -> (Self, mpsc::Receiver<ApplySetpointCmd>) {
        let (apply_tx, apply_rx) = mpsc::channel(APPLY_CHANNEL_CAPACITY);

        let scheduler = Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            apply_tx,
            current_page_epoch: Arc::new(RwLock::new(0)),
            workers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        };

        (scheduler, apply_rx)
    }

    /// Update the page epoch. Existing setpoints become stale because
    /// `get_desired()` checks for an exact match.
    pub fn set_page_epoch(&self, epoch: u64) {
        let mut current = self.current_page_epoch.write().unwrap();
        *current = epoch;
        trace!("FaderSetpoint: page epoch updated to {}", epoch);
    }

    /// Schedule a fader setpoint update.
    ///
    /// Updates the desired value and bumps the per-channel epoch, then wakes
    /// the channel's resident worker via `Notify::notify_one()`. The worker
    /// (created on demand) debounces, re-reads the latest desired/epoch, and
    /// emits an `ApplySetpointCmd` if the epoch still matches.
    ///
    /// # Arguments
    ///
    /// * `channel` - MIDI channel (1-9, where 9 is master fader)
    /// * `value14` - 14-bit value (0-16383)
    /// * `delay_ms` - Optional delay override (default 90 ms, 0 ms for extremes)
    pub fn schedule(&self, channel: u8, value14: u16, delay_ms: Option<u64>) {
        if !(1..=9).contains(&channel) {
            return;
        }

        let clamped = value14.min(16383);
        let current_page_epoch = *self.current_page_epoch.read().unwrap();

        let epoch_snapshot = {
            let mut channels = self.channels.write().unwrap();
            let state = channels.entry(channel).or_default();
            state.desired14 = clamped;
            state.epoch += 1;
            state.page_epoch = current_page_epoch;
            // Record the per-call delay on the channel state so it reaches the
            // worker even when the worker already exists. Previously only the
            // first-spawn override was honored, so the 120 ms requeue backoff
            // in app.rs (after a `set_fader` failure) was silently dropped and
            // retries fell back to the 0/90 ms default — effectively immediate
            // for the 0/16383 extremes during a device failure.
            state.override_delay_ms = delay_ms;
            state.epoch
        };

        trace!(
            "FaderSetpoint schedule: ch={} value={} delay_override={:?} epoch={}",
            channel,
            clamped,
            delay_ms,
            epoch_snapshot
        );

        self.ensure_worker(channel).notify_one();
    }

    /// Lazily create the resident worker for `channel` and return its Notify.
    /// The per-call delay override travels via `ChannelState::override_delay_ms`
    /// (set in `schedule`), so this no longer needs a delay argument.
    fn ensure_worker(&self, channel: u8) -> Arc<Notify> {
        let mut workers = self.workers.lock().unwrap();
        if let Some(existing) = workers.get(&channel) {
            return existing.notify.clone();
        }

        let notify = Arc::new(Notify::new());
        let handle = Self::spawn_worker(
            channel,
            self.channels.clone(),
            self.apply_tx.clone(),
            notify.clone(),
        );
        workers.insert(
            channel,
            ChannelWorker {
                notify: notify.clone(),
                handle,
            },
        );
        notify
    }

    fn spawn_worker(
        channel: u8,
        channels: Arc<RwLock<HashMap<u8, ChannelState>>>,
        apply_tx: mpsc::Sender<ApplySetpointCmd>,
        notify: Arc<Notify>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                notify.notified().await;

                let (snapshot_epoch, value14, eff_delay) = {
                    // Write lock so we can `take()` the per-call override —
                    // it applies on exactly one cycle, then reverts to the
                    // extreme-aware default.
                    let mut write = channels.write().unwrap();
                    let Some(state) = write.get_mut(&channel) else {
                        continue;
                    };
                    let is_extreme = state.desired14 == 0 || state.desired14 == 16383;
                    let delay = state.override_delay_ms.take().unwrap_or(if is_extreme {
                        0
                    } else {
                        DEBOUNCE_DELAY_MS
                    });
                    (state.epoch, state.desired14, delay)
                };

                if eff_delay > 0 {
                    tokio::time::sleep(Duration::from_millis(eff_delay)).await;
                }

                // Re-check epoch after the sleep. A newer schedule during the
                // debounce window would have bumped it and queued another
                // notify; we yield to that cycle instead.
                let still_current = {
                    let read = channels.read().unwrap();
                    read.get(&channel)
                        .is_some_and(|s| s.epoch == snapshot_epoch)
                };
                if !still_current {
                    trace!(
                        "FaderSetpoint apply SKIPPED (obsolete): ch={} epoch={}",
                        channel,
                        snapshot_epoch
                    );
                    continue;
                }

                let cmd = ApplySetpointCmd {
                    channel,
                    value14,
                    epoch: snapshot_epoch,
                };
                // Await capacity instead of dropping. A per-channel worker
                // blocking here only throttles its own channel (back-pressure),
                // and it guarantees the final last-wins setpoint is delivered
                // even if the main loop briefly stalls — otherwise a drop on the
                // *last* schedule of a gesture leaves the fader at the wrong
                // position. `Err` means the receiver is gone (scheduler torn
                // down), so the worker exits.
                if apply_tx.send(cmd).await.is_err() {
                    debug!(
                        "FaderSetpoint apply_tx closed (ch={}, epoch={}); worker exiting",
                        channel, snapshot_epoch
                    );
                    break;
                }
            }
        })
    }

    /// Get the current desired value for a channel if it's still valid.
    ///
    /// Returns `None` if the stored setpoint was created for a different page
    /// epoch, preventing stale values from leaking across rapid page changes.
    pub fn get_desired(&self, channel: u8) -> Option<u16> {
        let current_page_epoch = *self.current_page_epoch.read().unwrap();
        let channels = self.channels.read().unwrap();
        channels.get(&channel).and_then(|state| {
            if state.page_epoch == current_page_epoch {
                Some(state.desired14)
            } else {
                trace!(
                    "FaderSetpoint get_desired SKIPPED (stale): ch={} stored_page_epoch={} current_page_epoch={}",
                    channel,
                    state.page_epoch,
                    current_page_epoch
                );
                None
            }
        })
    }

    /// Get the current epoch for a channel (for debugging)
    #[allow(dead_code)] // diagnostics helper; symmetric with `is_epoch_current`
    pub fn get_epoch(&self, channel: u8) -> Option<u32> {
        let channels = self.channels.read().unwrap();
        channels.get(&channel).map(|state| state.epoch)
    }

    /// Check if an epoch is still current
    pub fn is_epoch_current(&self, channel: u8, epoch: u32) -> bool {
        let channels = self.channels.read().unwrap();
        channels
            .get(&channel)
            .map(|state| state.epoch == epoch)
            .unwrap_or(false)
    }
}

impl Default for FaderSetpoint {
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_epoch_increments() {
        let (setpoint, _rx) = FaderSetpoint::new();

        setpoint.schedule(1, 100, Some(1000)); // Long delay
        let epoch1 = setpoint.get_epoch(1).unwrap();

        setpoint.schedule(1, 200, Some(1000)); // New value
        let epoch2 = setpoint.get_epoch(1).unwrap();

        assert_eq!(epoch2, epoch1 + 1);
    }

    #[tokio::test]
    async fn test_rapid_movements_only_apply_last() {
        let (setpoint, mut rx) = FaderSetpoint::new();

        // Simulate rapid A→B→C movements; the resident worker collapses bursts
        // because `notify_one` buffers a single permit and the worker reads
        // the latest desired value after waking.
        setpoint.schedule(1, 100, Some(50)); // A
        setpoint.schedule(1, 200, Some(50)); // B
        setpoint.schedule(1, 300, Some(50)); // C (final)

        tokio::time::sleep(Duration::from_millis(200)).await;

        let mut commands = vec![];
        while let Ok(cmd) = rx.try_recv() {
            commands.push(cmd);
        }

        // Worker may emit one or two cycles depending on scheduling, but the
        // last one observed must reflect the final desired value.
        assert!(!commands.is_empty(), "expected at least one apply command");
        let last = commands.last().unwrap();
        assert_eq!(last.value14, 300);
    }

    #[tokio::test]
    async fn test_page_epoch_invalidates_stale_setpoints() {
        let (setpoint, _rx) = FaderSetpoint::new();

        setpoint.schedule(1, 5000, Some(1000));
        assert_eq!(setpoint.get_desired(1), Some(5000));

        setpoint.set_page_epoch(1);
        assert_eq!(setpoint.get_desired(1), None);

        setpoint.schedule(1, 8000, Some(1000));
        assert_eq!(setpoint.get_desired(1), Some(8000));
    }

    #[tokio::test]
    async fn test_rapid_page_changes_no_stale_values() {
        let (setpoint, _rx) = FaderSetpoint::new();

        setpoint.schedule(1, 1000, Some(1000));
        assert_eq!(setpoint.get_desired(1), Some(1000));

        setpoint.set_page_epoch(1);
        setpoint.set_page_epoch(2);

        assert_eq!(setpoint.get_desired(1), None);

        setpoint.schedule(1, 2000, Some(1000));
        assert_eq!(setpoint.get_desired(1), Some(2000));

        setpoint.set_page_epoch(3);
        assert_eq!(setpoint.get_desired(1), None);
    }

    #[tokio::test]
    async fn test_burst_under_capacity_never_panics() {
        // Audit #54: under a USB stall the previous implementation grew
        // task and channel queues without bound. With a bounded apply_tx
        // and a resident worker, a 1000-event burst must complete without
        // panicking and without the scheduler dropping its `Sender`.
        let (setpoint, mut rx) = FaderSetpoint::new();
        for i in 0..1000u16 {
            setpoint.schedule(1, i, Some(0));
        }
        // Let the worker run a few cycles.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Drain whatever the worker has produced; the test asserts liveness
        // (no panics, no deadlock), not exact count.
        while rx.try_recv().is_ok() {}
    }

    #[tokio::test]
    async fn test_worker_aborts_on_drop() {
        // Resident workers must not outlive their `FaderSetpoint`. We can't
        // observe the JoinHandle directly from outside, so we verify that
        // dropping the scheduler also closes its `Sender`, which the
        // receiver observes by returning `None`.
        let mut rx = {
            let (setpoint, rx) = FaderSetpoint::new();
            setpoint.schedule(1, 100, Some(0));
            tokio::time::sleep(Duration::from_millis(20)).await;
            rx
        };

        // After the scheduler drops, the worker task's clone of `apply_tx`
        // is also dropped (ChannelWorker::Drop aborts the task). Eventually
        // recv() must observe channel closure.
        let outcome = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        match outcome {
            Ok(Some(_)) => {
                // First read may be the pending apply; the next must close.
                let second = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
                assert!(matches!(second, Ok(None)), "channel must close after drop");
            },
            Ok(None) => {},
            Err(_) => panic!("recv timed out — worker did not abort or sender leaked"),
        }
    }
}
