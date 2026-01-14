//! StateActorHandle - Public API for the StateActor
//!
//! Provides an ergonomic async interface for interacting with the StateActor
//! through message passing. Fire-and-forget methods for hot paths, and
//! async methods with oneshot channels for queries.

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};

use super::commands::StateCommand;
use super::types::{AppKey, MidiAddr, MidiStateEntry, MidiStatus};

/// Handle for interacting with the StateActor
///
/// This is the public API for state operations. It wraps message passing
/// with ergonomic methods. All methods are non-blocking for the caller.
///
/// # Hot Path Methods (fire-and-forget)
/// - `update_state` - Update app state from feedback
/// - `update_shadow` - Update shadow state for X-Touch
/// - `mark_user_action` - Record user action timestamp
///
/// # Query Methods (async with response)
/// - `get_state` - Get exact state entry
/// - `get_known_latest` - Get latest matching entry
/// - `list_states` - List all states for an app
/// - `list_states_for_apps` - List states for multiple apps
///
/// # Anti-Echo Methods (async with response)
/// - `should_suppress_anti_echo` - Check anti-echo suppression
/// - `should_suppress_lww` - Check last-write-wins suppression
#[derive(Clone)]
pub struct StateActorHandle {
    /// Command channel to the StateActor
    cmd_tx: mpsc::UnboundedSender<StateCommand>,
}

impl StateActorHandle {
    /// Create a new StateActorHandle with the given command sender
    pub fn new(cmd_tx: mpsc::UnboundedSender<StateCommand>) -> Self {
        Self { cmd_tx }
    }

    /// Spawn a new StateActor and return a handle
    ///
    /// This is a convenience wrapper around `StateActor::spawn`.
    ///
    /// # Arguments
    ///
    /// * `persistence_tx` - Channel for sending persistence commands
    ///
    /// # Returns
    ///
    /// A `StateActorHandle` for interacting with the actor
    pub fn spawn(
        persistence_tx: tokio::sync::mpsc::Sender<super::persistence_actor::PersistenceCommand>,
    ) -> Self {
        super::actor::StateActor::spawn(persistence_tx)
    }

    // =========================================================================
    // Hot path methods (fire-and-forget, no await)
    // =========================================================================

    /// Update state from application feedback
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    /// Used in the hot path for MIDI feedback processing.
    pub fn update_state(&self, app: AppKey, entry: MidiStateEntry) {
        let _ = self.cmd_tx.send(StateCommand::UpdateState { app, entry });
    }

    /// Update shadow state for X-Touch output tracking
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    /// Used to track what values were last sent to X-Touch.
    pub fn update_shadow(&self, app: String, entry: MidiStateEntry) {
        let _ = self.cmd_tx.send(StateCommand::UpdateShadow { app, entry });
    }

    /// Mark a user action timestamp for LWW conflict resolution
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    /// Records when the user physically touched a control.
    pub fn mark_user_action(&self, key: String, ts: u64) {
        let _ = self.cmd_tx.send(StateCommand::MarkUserAction { key, ts });
    }

    // =========================================================================
    // Query methods (async with response)
    // =========================================================================

    /// Get exact state entry for an app and address
    ///
    /// Returns None if no matching entry exists or if the entry is not known.
    pub async fn get_state(&self, app: AppKey, addr: MidiAddr) -> Option<MidiStateEntry> {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = StateCommand::GetState {
            app,
            addr,
            response: response_tx,
        };

        if self.cmd_tx.send(cmd).is_err() {
            return None;
        }

        response_rx.await.ok().flatten()
    }

    /// Get the latest known value for (status, channel, data1) regardless of port
    ///
    /// Useful for finding the current value of a control across all ports.
    /// Prioritizes non-stale entries over stale ones.
    pub async fn get_known_latest(
        &self,
        app: AppKey,
        status: MidiStatus,
        channel: Option<u8>,
        data1: Option<u8>,
    ) -> Option<MidiStateEntry> {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = StateCommand::GetKnownLatest {
            app,
            status,
            channel,
            data1,
            response: response_tx,
        };

        if self.cmd_tx.send(cmd).is_err() {
            return None;
        }

        response_rx.await.ok().flatten()
    }

    /// List all known state entries for an application
    pub async fn list_states(&self, app: AppKey) -> Vec<MidiStateEntry> {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = StateCommand::ListStates {
            app,
            response: response_tx,
        };

        if self.cmd_tx.send(cmd).is_err() {
            return Vec::new();
        }

        response_rx.await.ok().unwrap_or_default()
    }

    /// List state entries for multiple applications
    pub async fn list_states_for_apps(
        &self,
        apps: Vec<AppKey>,
    ) -> HashMap<AppKey, Vec<MidiStateEntry>> {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = StateCommand::ListStatesForApps {
            apps,
            response: response_tx,
        };

        if self.cmd_tx.send(cmd).is_err() {
            return HashMap::new();
        }

        response_rx.await.ok().unwrap_or_default()
    }

    // =========================================================================
    // Anti-echo methods (async with response)
    // =========================================================================

    /// Check if an incoming event should be suppressed by anti-echo logic
    ///
    /// Returns true if the event matches a recently-sent value and should
    /// be suppressed to prevent feedback loops.
    pub async fn should_suppress_anti_echo(&self, app: String, entry: MidiStateEntry) -> bool {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = StateCommand::CheckSuppressAntiEcho {
            app,
            entry,
            response: response_tx,
        };

        if self.cmd_tx.send(cmd).is_err() {
            return false;
        }

        response_rx.await.ok().unwrap_or(false)
    }

    /// Check if an incoming event should be suppressed by last-write-wins logic
    ///
    /// Returns true if there was a recent user action on this control
    /// and the incoming value should be ignored.
    pub async fn should_suppress_lww(&self, entry: MidiStateEntry) -> bool {
        let (response_tx, response_rx) = oneshot::channel();
        let cmd = StateCommand::CheckSuppressLWW {
            entry,
            response: response_tx,
        };

        if self.cmd_tx.send(cmd).is_err() {
            return false;
        }

        response_rx.await.ok().unwrap_or(false)
    }

    // =========================================================================
    // Persistence methods
    // =========================================================================

    /// Hydrate state from a snapshot (e.g., loaded from disk)
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    /// Entries are marked as stale until fresh feedback arrives.
    pub fn hydrate_from_snapshot(&self, app: AppKey, entries: Vec<MidiStateEntry>) {
        let _ = self
            .cmd_tx
            .send(StateCommand::HydrateFromSnapshot { app, entries });
    }

    /// Clear all states for a specific application
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    pub fn clear_states_for_app(&self, app: AppKey) {
        let _ = self.cmd_tx.send(StateCommand::ClearStatesForApp { app });
    }

    /// Clear all states for all applications
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    pub fn clear_all_states(&self) {
        let _ = self.cmd_tx.send(StateCommand::ClearAllStates);
    }

    /// Clear all shadow states (for page refresh)
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    /// Clears the anti-echo shadow state to allow re-emission during page refresh.
    pub fn clear_shadows(&self) {
        let _ = self.cmd_tx.send(StateCommand::ClearShadows);
    }

    // =========================================================================
    // Lifecycle methods
    // =========================================================================

    /// Check if the actor is still alive
    ///
    /// Returns false if the command channel is closed.
    pub fn is_alive(&self) -> bool {
        !self.cmd_tx.is_closed()
    }

    /// Signal the actor to shut down gracefully
    ///
    /// Fire-and-forget: Does not wait for confirmation.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(StateCommand::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<StateActorHandle>();
    }

    #[tokio::test]
    async fn test_is_alive_when_channel_open() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = StateActorHandle::new(tx);
        assert!(handle.is_alive());
    }

    #[tokio::test]
    async fn test_is_alive_when_channel_closed() {
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx); // Close the receiver
        let handle = StateActorHandle::new(tx);
        assert!(!handle.is_alive());
    }
}
