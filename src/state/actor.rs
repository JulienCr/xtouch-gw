//! StateActor - Actor-based MIDI state management
//!
//! Provides a message-passing based state management system that encapsulates
//! all state operations behind a channel-based interface. This design:
//! - Eliminates lock contention by serializing all state access
//! - Provides clear ownership semantics
//! - Enables async-first design patterns
//! - Simplifies testing through message inspection

use super::actor_handle::StateActorHandle;
use super::commands::{StateCommand, SubscriberFn};
use super::persistence_actor::PersistenceCommand;
use super::types::{addr_key, AppKey, MidiAddr, MidiStateEntry, MidiStatus, Origin};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, info, trace};

/// Anti-echo time windows (in milliseconds) per MIDI status type
///
/// These windows prevent feedback loops by suppressing events that match
/// recently sent values within the time window.
pub const ANTI_ECHO_WINDOW_PB: u64 = 250; // Pitch Bend: motors need time to settle
pub const ANTI_ECHO_WINDOW_CC: u64 = 100; // Control Change: encoders can generate rapid changes
pub const ANTI_ECHO_WINDOW_NOTE: u64 = 10; // Note On/Off: buttons are discrete events

/// Last-Write-Wins grace periods (in milliseconds)
///
/// During these windows, feedback from applications is suppressed to allow
/// user actions to "win" over application state updates.
pub const LWW_GRACE_PERIOD_PB: u64 = 300;
pub const LWW_GRACE_PERIOD_CC: u64 = 50;

/// Shadow state entry tracking value and timestamp
///
/// Used for anti-echo suppression to track what values were recently sent
/// to each application.
#[derive(Debug, Clone)]
pub struct ShadowEntry {
    /// The MIDI value that was sent
    pub value: u16,
    /// Timestamp when the value was sent (milliseconds since epoch)
    pub ts: u64,
}

impl ShadowEntry {
    /// Create a new shadow entry with the current timestamp
    pub fn new(value: u16) -> Self {
        Self {
            value,
            ts: StateActor::now_ms(),
        }
    }
}

/// Type alias for state storage map
type StateMap = HashMap<String, MidiStateEntry>;

/// Type alias for shadow storage map
type ShadowMap = HashMap<String, ShadowEntry>;

/// Actor responsible for managing all MIDI state
///
/// The StateActor owns all state data and processes commands sequentially,
/// eliminating the need for locks and providing predictable behavior.
///
/// # Architecture
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │                      StateActor                              │
/// │  ┌─────────────────────────────────────────────────────────┐│
/// │  │ app_states: HashMap<AppKey, HashMap<String, Entry>>     ││
/// │  │ app_shadows: HashMap<String, HashMap<String, Shadow>>   ││
/// │  │ last_user_action_ts: HashMap<String, u64>               ││
/// │  │ subscribers: Vec<SubscriberFn>                          ││
/// │  └─────────────────────────────────────────────────────────┘│
/// │                           ▲                                  │
/// │                           │ commands                         │
/// │  ┌─────────────────────────────────────────────────────────┐│
/// │  │              command_rx (UnboundedReceiver)             ││
/// │  └─────────────────────────────────────────────────────────┘│
/// └─────────────────────────────────────────────────────────────┘
/// ```
pub struct StateActor {
    /// State storage per application
    /// Key: AppKey, Value: HashMap of addr_key -> MidiStateEntry
    app_states: HashMap<AppKey, StateMap>,

    /// Shadow state per application for anti-echo
    /// Key: app name (string), Value: HashMap of shadow_key -> ShadowEntry
    app_shadows: HashMap<String, ShadowMap>,

    /// Timestamps of last user actions for Last-Write-Wins
    /// Key: shadow_key, Value: timestamp in milliseconds
    last_user_action_ts: HashMap<String, u64>,

    /// Subscribers to state change notifications
    subscribers: Vec<SubscriberFn>,

    /// Receiver for incoming commands
    command_rx: mpsc::UnboundedReceiver<StateCommand>,

    /// Sender for persistence commands
    persistence_tx: mpsc::Sender<PersistenceCommand>,

    /// Counter for tracking total updates processed
    update_count: u64,
}

impl StateActor {
    /// Spawn a new StateActor and return a handle for interacting with it
    ///
    /// This creates the actor, initializes all state, and spawns the actor's
    /// run loop as a tokio task.
    ///
    /// # Arguments
    ///
    /// * `persistence_tx` - Channel for sending persistence commands
    ///
    /// # Returns
    ///
    /// A `StateActorHandle` that can be used to interact with the actor
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (persist_tx, persist_rx) = mpsc::channel(16);
    /// let handle = StateActor::spawn(persist_tx);
    /// ```
    pub fn spawn(persistence_tx: mpsc::Sender<PersistenceCommand>) -> StateActorHandle {
        // Create unbounded channel for commands
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        // Initialize state storage for all known apps
        let mut app_states = HashMap::new();
        for app in AppKey::all() {
            app_states.insert(*app, StateMap::new());
        }

        // Create the actor
        let actor = StateActor {
            app_states,
            app_shadows: HashMap::new(),
            last_user_action_ts: HashMap::new(),
            subscribers: Vec::new(),
            command_rx: cmd_rx,
            persistence_tx,
            update_count: 0,
        };

        // Spawn the actor's run loop
        tokio::spawn(actor.run());

        info!("StateActor spawned");

        // Return handle for interacting with the actor
        StateActorHandle::new(cmd_tx)
    }

    /// Main run loop for the actor
    ///
    /// Processes commands from the channel until the channel is closed.
    /// Each command is processed sequentially, ensuring thread-safe access
    /// to all internal state without locks.
    async fn run(mut self) {
        debug!("StateActor run loop started");

        while let Some(cmd) = self.command_rx.recv().await {
            trace!(?cmd, "Processing command");

            match cmd {
                // Hot path commands (no response)
                StateCommand::UpdateState { app, entry } => {
                    self.handle_update_state(app, entry);
                }
                StateCommand::UpdateShadow { app, entry } => {
                    self.handle_update_shadow(&app, &entry);
                }
                StateCommand::MarkUserAction { key, ts } => {
                    self.handle_mark_user_action(key, ts);
                }

                // Request-response commands
                StateCommand::GetState {
                    app,
                    addr,
                    response,
                } => {
                    let result = self.handle_get_state(app, &addr);
                    let _ = response.send(result);
                }
                StateCommand::GetKnownLatest {
                    app,
                    status,
                    channel,
                    data1,
                    response,
                } => {
                    let result = self.handle_get_known_latest(app, status, channel, data1);
                    let _ = response.send(result);
                }
                StateCommand::ListStates { app, response } => {
                    let result = self.handle_list_states(app);
                    let _ = response.send(result);
                }
                StateCommand::ListStatesForApps { apps, response } => {
                    let mut result = HashMap::new();
                    for app in apps {
                        result.insert(app, self.handle_list_states(app));
                    }
                    let _ = response.send(result);
                }
                StateCommand::CheckSuppressAntiEcho {
                    app,
                    entry,
                    response,
                } => {
                    let suppress = self.handle_check_suppress_anti_echo(&app, &entry);
                    let _ = response.send(suppress);
                }
                StateCommand::CheckSuppressLWW { entry, response } => {
                    let suppress = self.handle_check_suppress_lww(&entry);
                    let _ = response.send(suppress);
                }

                // Lifecycle commands
                StateCommand::HydrateFromSnapshot { app, entries } => {
                    self.handle_hydrate(app, entries);
                }
                StateCommand::ClearStatesForApp { app } => {
                    if let Some(app_state) = self.app_states.get_mut(&app) {
                        app_state.clear();
                    }
                    debug!(?app, "Cleared states for app");
                }
                StateCommand::ClearAllStates => {
                    for app_state in self.app_states.values_mut() {
                        app_state.clear();
                    }
                    debug!("Cleared all states");
                }
                StateCommand::ClearShadows => {
                    self.app_shadows.clear();
                    debug!("Cleared all shadow states");
                }
                StateCommand::Subscribe { listener, response } => {
                    self.subscribers.push(listener);
                    let id = self.subscribers.len() - 1;
                    let _ = response.send(id);
                    debug!(subscriber_id = id, "Added subscriber");
                }
                StateCommand::Shutdown => {
                    info!("StateActor received shutdown command");
                    break;
                }
            }
        }

        info!(
            update_count = self.update_count,
            "StateActor run loop terminated"
        );
    }

    /// Handle a state update command
    ///
    /// Updates the state for a given application and notifies subscribers.
    ///
    /// # Arguments
    ///
    /// * `app` - The application key
    /// * `entry` - The MIDI state entry to store
    fn handle_update_state(&mut self, app: AppKey, entry: MidiStateEntry) {
        let key = addr_key(&entry.addr);

        // Normalize entry for storage
        let stored = MidiStateEntry {
            origin: Origin::App,
            known: true,
            stale: false,
            ..entry.clone()
        };

        // Insert into app state
        if let Some(app_state) = self.app_states.get_mut(&app) {
            app_state.insert(key, stored.clone());
        }

        // Track update count
        self.update_count += 1;

        trace!(
            ?app,
            status = ?stored.addr.status,
            channel = ?stored.addr.channel,
            data1 = ?stored.addr.data1,
            value = ?stored.value,
            "State updated"
        );

        // Notify subscribers
        self.notify_subscribers(&stored, app);
    }

    /// Handle a get state query
    ///
    /// Retrieves the state entry for an exact address match.
    ///
    /// # Arguments
    ///
    /// * `app` - The application key
    /// * `addr` - The MIDI address to look up
    ///
    /// # Returns
    ///
    /// The state entry if found, None otherwise
    fn handle_get_state(&self, app: AppKey, addr: &MidiAddr) -> Option<MidiStateEntry> {
        let key = addr_key(addr);
        self.app_states
            .get(&app)
            .and_then(|app_state| app_state.get(&key))
            .filter(|entry| entry.known)
            .cloned()
    }

    /// Handle a query for the latest known state matching criteria
    ///
    /// Finds the most recent known entry matching the given status, channel,
    /// and data1 parameters, regardless of port.
    ///
    /// # Arguments
    ///
    /// * `app` - The application key
    /// * `status` - The MIDI status type to match
    /// * `channel` - Optional channel filter
    /// * `data1` - Optional data1 filter
    ///
    /// # Returns
    ///
    /// The most recent matching entry, preferring non-stale over stale
    fn handle_get_known_latest(
        &self,
        app: AppKey,
        status: MidiStatus,
        channel: Option<u8>,
        data1: Option<u8>,
    ) -> Option<MidiStateEntry> {
        let app_state = self.app_states.get(&app)?;

        let mut best: Option<&MidiStateEntry> = None;

        for entry in app_state.values() {
            let addr = &entry.addr;

            // Match status
            if addr.status != status {
                continue;
            }

            // Match channel if specified
            if let Some(ch) = channel {
                if addr.channel != Some(ch) {
                    continue;
                }
            }

            // Match data1 if specified
            if let Some(d1) = data1 {
                if addr.data1 != Some(d1) {
                    continue;
                }
            }

            // Must be known
            if !entry.known {
                continue;
            }

            // BUG-005 FIX: Prefer non-stale over stale, then most recent timestamp
            let dominated = match best {
                None => false,
                Some(current) => {
                    // Prefer non-stale over stale
                    if entry.stale && !current.stale {
                        true // current is non-stale, entry is stale -> current wins
                    } else if !entry.stale && current.stale {
                        false // entry is non-stale, current is stale -> entry should win
                    } else {
                        // Same stale status: prefer more recent timestamp
                        entry.ts <= current.ts
                    }
                }
            };

            if !dominated {
                best = Some(entry);
            }
        }

        best.cloned()
    }

    /// Handle a list states query
    ///
    /// Returns all state entries for an application.
    ///
    /// # Arguments
    ///
    /// * `app` - The application key
    ///
    /// # Returns
    ///
    /// Vector of all state entries for the application
    fn handle_list_states(&self, app: AppKey) -> Vec<MidiStateEntry> {
        self.app_states
            .get(&app)
            .map(|app_state| app_state.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Check if an event should be suppressed due to anti-echo
    ///
    /// Compares the entry against shadow state to determine if this is
    /// an echo of a recently sent value.
    ///
    /// # Arguments
    ///
    /// * `app` - The application identifier string
    /// * `entry` - The MIDI state entry to check
    ///
    /// # Returns
    ///
    /// `true` if the event should be suppressed, `false` otherwise
    fn handle_check_suppress_anti_echo(&self, app: &str, entry: &MidiStateEntry) -> bool {
        let app_shadow = match self.app_shadows.get(app) {
            Some(shadow) => shadow,
            None => return false,
        };

        let key = make_shadow_key_from_entry(entry);

        if let Some(prev) = app_shadow.get(&key) {
            let value = entry.value.as_number().unwrap_or(0);
            let window = Self::get_anti_echo_window(entry.addr.status);
            let elapsed = Self::now_ms().saturating_sub(prev.ts);

            // Suppress if value matches and within time window
            if prev.value == value && elapsed < window {
                trace!(
                    status = ?entry.addr.status,
                    elapsed_ms = elapsed,
                    window_ms = window,
                    "Anti-echo suppression"
                );
                return true;
            }
        }

        false
    }

    /// Check if an event should be suppressed due to Last-Write-Wins
    ///
    /// Determines if a user action occurred recently enough that application
    /// feedback should be ignored.
    ///
    /// # Arguments
    ///
    /// * `entry` - The MIDI state entry to check
    ///
    /// # Returns
    ///
    /// `true` if the event should be suppressed, `false` otherwise
    fn handle_check_suppress_lww(&self, entry: &MidiStateEntry) -> bool {
        let key = make_shadow_key_from_entry(entry);

        let last_user_ts = self.last_user_action_ts.get(&key).copied().unwrap_or(0);

        let grace_period = match entry.addr.status {
            MidiStatus::PB => LWW_GRACE_PERIOD_PB,
            MidiStatus::CC => LWW_GRACE_PERIOD_CC,
            _ => 0,
        };

        let elapsed = Self::now_ms().saturating_sub(last_user_ts);

        if grace_period > 0 && elapsed < grace_period {
            trace!(
                status = ?entry.addr.status,
                elapsed_ms = elapsed,
                grace_period_ms = grace_period,
                "LWW suppression"
            );
            return true;
        }

        false
    }

    /// Update shadow state after sending to an application
    ///
    /// Records the value and timestamp for future anti-echo checks.
    ///
    /// # Arguments
    ///
    /// * `app` - The application identifier string
    /// * `entry` - The MIDI state entry that was sent
    fn handle_update_shadow(&mut self, app: &str, entry: &MidiStateEntry) {
        let key = make_shadow_key_from_entry(entry);
        let value = entry.value.as_number().unwrap_or(0);
        let shadow_entry = ShadowEntry::new(value);

        let app_shadow = self
            .app_shadows
            .entry(app.to_string())
            .or_insert_with(HashMap::new);
        app_shadow.insert(key, shadow_entry);

        trace!(
            app,
            status = ?entry.addr.status,
            channel = ?entry.addr.channel,
            data1 = ?entry.addr.data1,
            value,
            "Shadow updated"
        );
    }

    /// Mark a user action timestamp for Last-Write-Wins
    ///
    /// Records that a user action occurred for the given key.
    ///
    /// # Arguments
    ///
    /// * `key` - The shadow key for the action
    /// * `ts` - The timestamp of the action
    fn handle_mark_user_action(&mut self, key: String, ts: u64) {
        self.last_user_action_ts.insert(key.clone(), ts);
        trace!(key, ts, "User action marked");
    }

    /// Hydrate state from a snapshot
    ///
    /// Loads state entries from persistence, marking them as stale.
    ///
    /// # Arguments
    ///
    /// * `app` - The application key
    /// * `entries` - The entries to load
    fn handle_hydrate(&mut self, app: AppKey, entries: Vec<MidiStateEntry>) {
        if let Some(app_state) = self.app_states.get_mut(&app) {
            for entry in entries {
                // Normalize entry with stale=true
                let normalized = MidiStateEntry {
                    origin: Origin::App,
                    known: true,
                    stale: true,
                    ..entry
                };
                let key = addr_key(&normalized.addr);
                app_state.insert(key, normalized);
            }
        }
        debug!(?app, "Hydrated state from snapshot");
    }

    /// Notify all subscribers of a state change
    ///
    /// # Arguments
    ///
    /// * `entry` - The state entry that changed
    /// * `app` - The application key
    fn notify_subscribers(&self, entry: &MidiStateEntry, app: AppKey) {
        for subscriber in &self.subscribers {
            subscriber(entry, app);
        }
    }

    /// Get current timestamp in milliseconds since epoch
    ///
    /// # Returns
    ///
    /// Current time in milliseconds
    pub fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Get anti-echo window for a MIDI status type
    ///
    /// Returns the appropriate time window in milliseconds based on the MIDI
    /// message type. Different types have different windows to account for
    /// their physical characteristics (e.g., motorized faders need more time).
    ///
    /// # Arguments
    ///
    /// * `status` - The MIDI status type
    ///
    /// # Returns
    ///
    /// Time window in milliseconds
    fn get_anti_echo_window(status: MidiStatus) -> u64 {
        match status {
            MidiStatus::PB => ANTI_ECHO_WINDOW_PB,
            MidiStatus::CC => ANTI_ECHO_WINDOW_CC,
            MidiStatus::Note => ANTI_ECHO_WINDOW_NOTE,
            MidiStatus::SysEx => 60, // Fallback for SysEx
        }
    }
}

/// Generate a consistent shadow key for MIDI state tracking
///
/// Format: "{status_lowercase}|{channel}|{data1}"
///
/// # Arguments
///
/// * `status` - The MIDI status type
/// * `channel` - The MIDI channel (1-16)
/// * `data1` - The first data byte
///
/// # Returns
///
/// A string key for shadow state lookup
pub fn make_shadow_key(status: MidiStatus, channel: u8, data1: u8) -> String {
    let status_str = match status {
        MidiStatus::Note => "note",
        MidiStatus::CC => "cc",
        MidiStatus::PB => "pb",
        MidiStatus::SysEx => "sysex",
    };
    format!("{}|{}|{}", status_str, channel, data1)
}

/// Generate shadow key from a MidiStateEntry
///
/// Convenience function that extracts fields from an entry.
///
/// # Arguments
///
/// * `entry` - The MIDI state entry
///
/// # Returns
///
/// A string key for shadow state lookup
pub fn make_shadow_key_from_entry(entry: &MidiStateEntry) -> String {
    make_shadow_key(
        entry.addr.status,
        entry.addr.channel.unwrap_or(0),
        entry.addr.data1.unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shadow_key_generation() {
        let key = make_shadow_key(MidiStatus::CC, 1, 7);
        assert_eq!(key, "cc|1|7");

        let key = make_shadow_key(MidiStatus::PB, 3, 0);
        assert_eq!(key, "pb|3|0");

        let key = make_shadow_key(MidiStatus::Note, 10, 60);
        assert_eq!(key, "note|10|60");
    }

    #[test]
    fn test_shadow_entry_creation() {
        let entry = ShadowEntry::new(8192);
        assert_eq!(entry.value, 8192);
        assert!(entry.ts > 0);
    }

    #[test]
    fn test_now_ms() {
        let ts1 = StateActor::now_ms();
        let ts2 = StateActor::now_ms();
        assert!(ts2 >= ts1);
        // Should be a reasonable timestamp (after year 2020)
        assert!(ts1 > 1_577_836_800_000);
    }
}
