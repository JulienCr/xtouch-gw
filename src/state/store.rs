//! StateStore - In-memory MIDI state per application with subscription support
//!
//! Stores MIDI state for each application (Voicemeeter, QLC+, OBS, etc.)
//! and notifies subscribers on updates.

use super::types::{addr_key, AppKey, MidiStateEntry, MidiStatus, Origin};
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

    /// List all known state entries for an application
    pub fn list_states_for_app(&self, app: AppKey) -> Vec<MidiStateEntry> {
        let states = self.app_states.read().unwrap();
        states
            .get(&app)
            .map(|app_state| app_state.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the latest known value for (status, channel, data1) regardless of port
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

            // Keep the most recent
            if best.is_none() || entry.ts > best.unwrap().ts {
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
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MidiAddr, MidiValue};
    

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
    fn test_update_and_list() {
        let store = StateStore::new();
        let entry = make_test_entry(MidiStatus::CC, 1, 7, 100);

        store.update_from_feedback(AppKey::Voicemeeter, entry.clone());

        let states = store.list_states_for_app(AppKey::Voicemeeter);
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].value.as_number(), Some(100));
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

}

