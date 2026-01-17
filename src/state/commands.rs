//! Command enums for StateStore actor and persistence actor
//!
//! These commands enable message-passing architectures for state management,
//! separating the hot path (fire-and-forget updates) from request-response
//! operations that need acknowledgment.

use super::persistence::StateSnapshot;
use super::types::{AppKey, MidiAddr, MidiStateEntry, MidiStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

// ============================================================================
// Type Aliases
// ============================================================================

/// Subscriber callback function type
///
/// Called when state is updated, receives the entry and the application key.
/// Must be Send + Sync for cross-thread usage.
pub type SubscriberFn = Arc<dyn Fn(&MidiStateEntry, AppKey) + Send + Sync>;

// ============================================================================
// StateCommand
// ============================================================================

/// Commands for the StateStore actor
///
/// Commands are divided into two categories:
/// - **Hot path** (no response): Fire-and-forget updates that don't block the sender
/// - **Request-response**: Operations that return data via oneshot channel
///
/// # Hot Path Commands
///
/// These commands are designed for the critical MIDI processing path where
/// latency is paramount. They complete without waiting for acknowledgment:
///
/// - `UpdateState`: Store state from application feedback
/// - `UpdateShadow`: Update shadow state for anti-echo
/// - `MarkUserAction`: Record user action timestamp for LWW
///
/// # Request-Response Commands
///
/// These commands require a response and use oneshot channels:
///
/// - `GetState`: Retrieve exact state entry
/// - `GetKnownLatest`: Find best matching entry
/// - `ListStates`: List all entries for an app
/// - `ListStatesForApps`: Batch list for multiple apps
/// - `CheckSuppressAntiEcho`: Query anti-echo suppression
/// - `CheckSuppressLWW`: Query last-write-wins suppression
/// - `Subscribe`: Register state change listener
pub enum StateCommand {
    // -------------------------------------------------------------------------
    // Hot path commands (no response - fire and forget)
    // -------------------------------------------------------------------------
    /// Update state from application feedback
    ///
    /// This is the primary way to record state changes from drivers.
    /// Does not block the sender.
    UpdateState {
        /// Application that sent the feedback
        app: AppKey,
        /// State entry to store
        entry: MidiStateEntry,
    },

    /// Update shadow state for anti-echo tracking
    ///
    /// Called after sending to an application to record what was sent
    /// for echo suppression.
    UpdateShadow {
        /// Application key (as string for flexibility)
        app: String,
        /// Entry that was sent
        entry: MidiStateEntry,
    },

    /// Mark a user action timestamp for Last-Write-Wins
    ///
    /// Called when X-Touch sends input to record the action time.
    /// Used to suppress application feedback that arrives during user interaction.
    MarkUserAction {
        /// Shadow key for the control
        key: String,
        /// Timestamp in milliseconds
        ts: u64,
    },

    // -------------------------------------------------------------------------
    // Request-response commands (require oneshot channel)
    // -------------------------------------------------------------------------
    /// Get exact state entry for an address
    ///
    /// Requires full address match including port_id.
    GetState {
        /// Application to query
        app: AppKey,
        /// Full address to match
        addr: MidiAddr,
        /// Response channel
        response: oneshot::Sender<Option<MidiStateEntry>>,
    },

    /// Get the latest known value matching criteria
    ///
    /// Finds the best matching entry regardless of port_id.
    /// Prioritizes non-stale entries over stale ones.
    GetKnownLatest {
        /// Application to query
        app: AppKey,
        /// MIDI status type to match
        status: MidiStatus,
        /// Optional channel filter (1-16)
        channel: Option<u8>,
        /// Optional data1 filter (CC number, note number, etc.)
        data1: Option<u8>,
        /// Response channel
        response: oneshot::Sender<Option<MidiStateEntry>>,
    },

    /// List all state entries for an application
    ListStates {
        /// Application to query
        app: AppKey,
        /// Response channel
        response: oneshot::Sender<Vec<MidiStateEntry>>,
    },

    /// List state entries for multiple applications
    ///
    /// More efficient than multiple `ListStates` calls.
    ListStatesForApps {
        /// Applications to query
        apps: Vec<AppKey>,
        /// Response channel
        response: oneshot::Sender<HashMap<AppKey, Vec<MidiStateEntry>>>,
    },

    /// Check if an entry should be suppressed by anti-echo
    ///
    /// Returns true if the entry matches a recently sent value
    /// and should not be forwarded to avoid feedback loops.
    CheckSuppressAntiEcho {
        /// Application key (as string)
        app: String,
        /// Entry to check
        entry: MidiStateEntry,
        /// Response channel
        response: oneshot::Sender<bool>,
    },

    /// Check if an entry should be suppressed by Last-Write-Wins
    ///
    /// Returns true if a user action was recent enough that
    /// application feedback should be ignored.
    CheckSuppressLWW {
        /// Entry to check
        entry: MidiStateEntry,
        /// Response channel
        response: oneshot::Sender<bool>,
    },

    // -------------------------------------------------------------------------
    // Lifecycle commands
    // -------------------------------------------------------------------------
    /// Load state from a snapshot (typically at startup)
    ///
    /// Entries are marked as stale until fresh feedback arrives.
    /// If `response` is provided, sends `()` when hydration is complete.
    HydrateFromSnapshot {
        /// Application to hydrate
        app: AppKey,
        /// Entries to load
        entries: Vec<MidiStateEntry>,
        /// Optional response channel for sync waiting
        response: Option<oneshot::Sender<()>>,
    },

    /// Clear all state for a specific application
    ClearStatesForApp {
        /// Application to clear
        app: AppKey,
    },

    /// Clear all state for all applications
    ClearAllStates,

    /// Clear all shadow states (for page refresh)
    ///
    /// Clears the anti-echo shadow state to allow re-emission during page refresh.
    ClearShadows,

    /// Subscribe to state change notifications
    ///
    /// Returns a subscriber ID that can be used for unsubscription.
    Subscribe {
        /// Callback function to invoke on state changes
        listener: SubscriberFn,
        /// Response channel returning subscriber ID
        response: oneshot::Sender<usize>,
    },

    /// Gracefully shut down the state actor
    Shutdown,
}

// Manual Debug implementation because SubscriberFn doesn't implement Debug
impl std::fmt::Debug for StateCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateCommand::UpdateState { app, entry } => f
                .debug_struct("UpdateState")
                .field("app", app)
                .field("entry", entry)
                .finish(),
            StateCommand::UpdateShadow { app, entry } => f
                .debug_struct("UpdateShadow")
                .field("app", app)
                .field("entry", entry)
                .finish(),
            StateCommand::MarkUserAction { key, ts } => f
                .debug_struct("MarkUserAction")
                .field("key", key)
                .field("ts", ts)
                .finish(),
            StateCommand::GetState { app, addr, .. } => f
                .debug_struct("GetState")
                .field("app", app)
                .field("addr", addr)
                .finish_non_exhaustive(),
            StateCommand::GetKnownLatest {
                app,
                status,
                channel,
                data1,
                ..
            } => f
                .debug_struct("GetKnownLatest")
                .field("app", app)
                .field("status", status)
                .field("channel", channel)
                .field("data1", data1)
                .finish_non_exhaustive(),
            StateCommand::ListStates { app, .. } => f
                .debug_struct("ListStates")
                .field("app", app)
                .finish_non_exhaustive(),
            StateCommand::ListStatesForApps { apps, .. } => f
                .debug_struct("ListStatesForApps")
                .field("apps", apps)
                .finish_non_exhaustive(),
            StateCommand::CheckSuppressAntiEcho { app, entry, .. } => f
                .debug_struct("CheckSuppressAntiEcho")
                .field("app", app)
                .field("entry", entry)
                .finish_non_exhaustive(),
            StateCommand::CheckSuppressLWW { entry, .. } => f
                .debug_struct("CheckSuppressLWW")
                .field("entry", entry)
                .finish_non_exhaustive(),
            StateCommand::HydrateFromSnapshot { app, entries, .. } => f
                .debug_struct("HydrateFromSnapshot")
                .field("app", app)
                .field("entries_count", &entries.len())
                .finish_non_exhaustive(),
            StateCommand::ClearStatesForApp { app } => f
                .debug_struct("ClearStatesForApp")
                .field("app", app)
                .finish(),
            StateCommand::ClearAllStates => write!(f, "ClearAllStates"),
            StateCommand::ClearShadows => write!(f, "ClearShadows"),
            StateCommand::Subscribe { .. } => f.debug_struct("Subscribe").finish_non_exhaustive(),
            StateCommand::Shutdown => write!(f, "Shutdown"),
        }
    }
}

// ============================================================================
// PersistenceCommand
// ============================================================================

/// Commands for the persistence actor
///
/// Handles asynchronous state persistence to disk. All operations are
/// fire-and-forget except for `LoadSnapshot` which returns data.
///
/// # Usage
///
/// The persistence actor runs in a separate task and handles:
/// - Periodic snapshot saves
/// - On-demand flush before shutdown
/// - State recovery on startup
#[derive(Debug)]
pub enum PersistenceCommand {
    /// Save a state snapshot to disk
    ///
    /// Fire-and-forget: the caller doesn't wait for completion.
    /// The actor handles file I/O asynchronously.
    SaveSnapshot(StateSnapshot),

    /// Load the most recent snapshot from disk
    ///
    /// Returns `None` if no snapshot exists or loading fails.
    LoadSnapshot {
        /// Response channel
        response: oneshot::Sender<Option<StateSnapshot>>,
    },

    /// Flush any pending writes to disk
    ///
    /// Used before shutdown to ensure all state is persisted.
    /// Fire-and-forget but blocks internally until complete.
    Flush,

    /// Gracefully shut down the persistence actor
    ///
    /// Performs a final flush before exiting.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::{MidiValue, Origin};

    fn make_test_entry() -> MidiStateEntry {
        MidiStateEntry {
            addr: MidiAddr {
                port_id: "test".to_string(),
                status: MidiStatus::CC,
                channel: Some(1),
                data1: Some(7),
            },
            value: MidiValue::Number(100),
            ts: 1000,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        }
    }

    #[test]
    fn test_state_command_debug() {
        let entry = make_test_entry();

        // Test hot path commands
        let cmd = StateCommand::UpdateState {
            app: AppKey::Voicemeeter,
            entry: entry.clone(),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("UpdateState"));
        assert!(debug_str.contains("Voicemeeter"));

        // Test request-response command
        let (tx, _rx) = oneshot::channel();
        let cmd = StateCommand::GetState {
            app: AppKey::Obs,
            addr: entry.addr.clone(),
            response: tx,
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetState"));
        assert!(debug_str.contains("Obs"));

        // Test lifecycle commands
        let cmd = StateCommand::Shutdown;
        assert_eq!(format!("{:?}", cmd), "Shutdown");
    }

    #[test]
    fn test_persistence_command_debug() {
        let cmd = PersistenceCommand::Flush;
        assert_eq!(format!("{:?}", cmd), "Flush");

        let cmd = PersistenceCommand::Shutdown;
        assert_eq!(format!("{:?}", cmd), "Shutdown");

        let (tx, _rx) = oneshot::channel();
        let cmd = PersistenceCommand::LoadSnapshot { response: tx };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("LoadSnapshot"));
    }

    #[tokio::test]
    async fn test_oneshot_response_channels() {
        // Verify response channels work correctly
        let (tx, rx) = oneshot::channel::<Option<MidiStateEntry>>();
        let entry = make_test_entry();
        tx.send(Some(entry.clone())).unwrap();
        let received = rx.await.unwrap();
        assert_eq!(received.unwrap().value.as_number(), Some(100));
    }
}
