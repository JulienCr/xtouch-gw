//! StateStore - In-memory MIDI state per application with subscription support
//!
//! Stores MIDI state for each application (Voicemeeter, QLC+, OBS, etc.)
//! and notifies subscribers on updates.

use super::types::{addr_key, AppKey, MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

type StateMap = HashMap<String, MidiStateEntry>;
type SubscriberFn = Arc<dyn Fn(&MidiStateEntry, AppKey) + Send + Sync>;

/// Stores MIDI state per application and notifies subscribers on updates
#[derive(Clone)]
pub struct StateStore {
    /// State storage per application
    app_states: Arc<RwLock<HashMap<AppKey, StateMap>>>,
    /// Subscribers to state updates
    subscribers: Arc<RwLock<Vec<SubscriberFn>>>,
}

impl StateStore {
    /// Create a new StateStore with initialized app states
    pub fn new() -> Self {
        let mut app_states = HashMap::new();
        for app in AppKey::all() {
            app_states.insert(*app, StateMap::new());
        }

        Self {
            app_states: Arc::new(RwLock::new(app_states)),
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register feedback from an application and publish to subscribers
    pub fn update_from_feedback(&self, app: AppKey, entry: MidiStateEntry) {
        let key = addr_key(&entry.addr);

        // Update stored entry
        let stored = MidiStateEntry {
            origin: Origin::App,
            known: true,
            stale: false,
            ..entry.clone()
        };

        {
            let mut states = self.app_states.write().unwrap();
            if let Some(app_state) = states.get_mut(&app) {
                app_state.insert(key, stored.clone());
            }
        }

        // Notify subscribers
        let subscribers = self.subscribers.read().unwrap();
        for subscriber in subscribers.iter() {
            subscriber(&stored, app);
        }
    }

    /// Get exact state entry for an app (requires full address match including port)
    pub fn get_state_for_app(&self, app: AppKey, addr: &MidiAddr) -> Option<MidiStateEntry> {
        let key = addr_key(addr);
        let states = self.app_states.read().unwrap();
        states
            .get(&app)
            .and_then(|app_state| app_state.get(&key))
            .filter(|entry| entry.known)
            .cloned()
    }

    /// List all known state entries for an application
    pub fn list_states_for_app(&self, app: AppKey) -> Vec<MidiStateEntry> {
        let states = self.app_states.read().unwrap();
        states
            .get(&app)
            .map(|app_state| app_state.values().cloned().collect())
            .unwrap_or_default()
    }

    /// List state entries for multiple applications
    pub fn list_states_for_apps(&self, apps: &[AppKey]) -> HashMap<AppKey, Vec<MidiStateEntry>> {
        let mut result = HashMap::new();
        for app in apps {
            result.insert(*app, self.list_states_for_app(*app));
        }
        result
    }

    /// Subscribe to state update notifications
    ///
    /// Returns an unsubscribe handle (currently just the subscriber ID)
    pub fn subscribe<F>(&self, listener: F) -> usize
    where
        F: Fn(&MidiStateEntry, AppKey) + Send + Sync + 'static,
    {
        let mut subscribers = self.subscribers.write().unwrap();
        subscribers.push(Arc::new(listener));
        subscribers.len() - 1
    }

    /// Get the latest known value for (status, channel, data1) regardless of port
    ///
    /// BUG-005 FIX: Prioritizes non-stale entries over stale ones.
    /// Entries restored from snapshot are marked `stale: true` and will be
    /// superseded by fresh feedback from the application.
    ///
    /// Priority order:
    /// 1. Non-stale (fresh feedback) with most recent timestamp
    /// 2. Stale (from snapshot) with most recent timestamp
    pub fn get_known_latest_for_app(
        &self,
        app: AppKey,
        status: MidiStatus,
        channel: Option<u8>,
        data1: Option<u8>,
    ) -> Option<MidiStateEntry> {
        let states = self.app_states.read().unwrap();
        let app_state = states.get(&app)?;

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

    /// Hydrate state from a snapshot (marks entries as stale)
    ///
    /// Used to restore state from disk without notifying subscribers.
    /// All entries are marked `stale: true` to indicate potential obsolescence.
    pub fn hydrate_from_snapshot(&self, app: AppKey, entries: Vec<MidiStateEntry>) {
        let mut states = self.app_states.write().unwrap();
        if let Some(app_state) = states.get_mut(&app) {
            for entry in entries {
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
    }

    /// Clear all states for a specific application
    pub fn clear_states_for_app(&self, app: AppKey) {
        let mut states = self.app_states.write().unwrap();
        if let Some(app_state) = states.get_mut(&app) {
            app_state.clear();
        }
    }

    /// Clear all states for all applications
    pub fn clear_all_states(&self) {
        let mut states = self.app_states.write().unwrap();
        for app_state in states.values_mut() {
            app_state.clear();
        }
    }
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn make_test_entry(status: MidiStatus, channel: u8, data1: u8, value: u16) -> MidiStateEntry {
        MidiStateEntry {
            addr: MidiAddr {
                port_id: "test".to_string(),
                status,
                channel: Some(channel),
                data1: Some(data1),
            },
            value: MidiValue::Number(value),
            ts: 1000,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        }
    }

    #[test]
    fn test_update_and_get() {
        let store = StateStore::new();
        let entry = make_test_entry(MidiStatus::CC, 1, 7, 100);

        store.update_from_feedback(AppKey::Voicemeeter, entry.clone());

        let retrieved = store
            .get_state_for_app(AppKey::Voicemeeter, &entry.addr)
            .unwrap();
        assert_eq!(retrieved.value.as_number(), Some(100));
    }

    #[test]
    fn test_list_states() {
        let store = StateStore::new();
        let entry1 = make_test_entry(MidiStatus::CC, 1, 7, 100);
        let entry2 = make_test_entry(MidiStatus::CC, 1, 8, 50);

        store.update_from_feedback(AppKey::Voicemeeter, entry1);
        store.update_from_feedback(AppKey::Voicemeeter, entry2);

        let states = store.list_states_for_app(AppKey::Voicemeeter);
        assert_eq!(states.len(), 2);
    }

    #[test]
    fn test_get_known_latest() {
        let store = StateStore::new();

        // Insert entry with ts=1000
        let entry1 = make_test_entry(MidiStatus::CC, 1, 7, 50);
        store.update_from_feedback(AppKey::Voicemeeter, entry1);

        // Insert newer entry with ts=2000
        let mut entry2 = make_test_entry(MidiStatus::CC, 1, 7, 100);
        entry2.ts = 2000;
        store.update_from_feedback(AppKey::Voicemeeter, entry2);

        // Should get the newer one
        let latest = store
            .get_known_latest_for_app(AppKey::Voicemeeter, MidiStatus::CC, Some(1), Some(7))
            .unwrap();
        assert_eq!(latest.value.as_number(), Some(100));
        assert_eq!(latest.ts, 2000);
    }

    #[test]
    fn test_subscribe() {
        let store = StateStore::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        store.subscribe(move |_entry, _app| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let entry = make_test_entry(MidiStatus::CC, 1, 7, 100);
        store.update_from_feedback(AppKey::Voicemeeter, entry.clone());
        store.update_from_feedback(AppKey::Voicemeeter, entry);

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_hydrate_from_snapshot() {
        let store = StateStore::new();
        let entry = make_test_entry(MidiStatus::CC, 1, 7, 100);

        store.hydrate_from_snapshot(AppKey::Voicemeeter, vec![entry.clone()]);

        let retrieved = store
            .get_state_for_app(AppKey::Voicemeeter, &entry.addr)
            .unwrap();
        assert!(retrieved.stale); // Should be marked stale
        assert_eq!(retrieved.value.as_number(), Some(100));
    }

    #[test]
    fn test_clear_states() {
        let store = StateStore::new();
        let entry = make_test_entry(MidiStatus::CC, 1, 7, 100);

        store.update_from_feedback(AppKey::Voicemeeter, entry.clone());
        assert_eq!(store.list_states_for_app(AppKey::Voicemeeter).len(), 1);

        store.clear_states_for_app(AppKey::Voicemeeter);
        assert_eq!(store.list_states_for_app(AppKey::Voicemeeter).len(), 0);
    }

    /// BUG-005 test: Verify that non-stale entries are preferred over stale ones
    #[test]
    fn test_stale_flag_priority() {
        let store = StateStore::new();

        // First, hydrate a stale entry from snapshot (ts=1000, value=50)
        let mut stale_entry = make_test_entry(MidiStatus::CC, 1, 7, 50);
        stale_entry.ts = 1000;
        store.hydrate_from_snapshot(AppKey::Voicemeeter, vec![stale_entry.clone()]);

        // Verify stale entry is returned when no fresh data exists
        let result = store
            .get_known_latest_for_app(AppKey::Voicemeeter, MidiStatus::CC, Some(1), Some(7))
            .unwrap();
        assert!(result.stale, "Hydrated entry should be stale");
        assert_eq!(result.value.as_number(), Some(50));

        // Now add fresh feedback with OLDER timestamp (ts=500, value=100)
        // Fresh should win even with older timestamp
        let mut fresh_entry = make_test_entry(MidiStatus::CC, 1, 7, 100);
        fresh_entry.ts = 500; // Older than stale entry!
        store.update_from_feedback(AppKey::Voicemeeter, fresh_entry);

        let result = store
            .get_known_latest_for_app(AppKey::Voicemeeter, MidiStatus::CC, Some(1), Some(7))
            .unwrap();
        assert!(!result.stale, "Fresh feedback should not be stale");
        assert_eq!(
            result.value.as_number(),
            Some(100),
            "Fresh entry should win over stale even with older timestamp"
        );
    }

    /// BUG-005 test: Among same stale status, prefer most recent timestamp
    #[test]
    fn test_stale_flag_same_status_uses_timestamp() {
        let store = StateStore::new();

        // Hydrate two stale entries with different timestamps
        let mut stale1 = make_test_entry(MidiStatus::CC, 1, 7, 50);
        stale1.ts = 1000;
        stale1.addr.port_id = "port1".to_string();

        let mut stale2 = make_test_entry(MidiStatus::CC, 1, 7, 100);
        stale2.ts = 2000; // More recent
        stale2.addr.port_id = "port2".to_string();

        store.hydrate_from_snapshot(AppKey::Voicemeeter, vec![stale1, stale2]);

        // Should get the more recent stale entry
        let result = store
            .get_known_latest_for_app(AppKey::Voicemeeter, MidiStatus::CC, Some(1), Some(7))
            .unwrap();
        assert!(result.stale);
        assert_eq!(
            result.value.as_number(),
            Some(100),
            "More recent stale entry should win"
        );
        assert_eq!(result.ts, 2000);
    }
}

