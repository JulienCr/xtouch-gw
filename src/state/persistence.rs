//! State persistence to JSON snapshots
//!
//! Allows saving and loading StateStore state to/from disk for recovery
//! after restarts.

use super::store::StateStore;
use super::types::{AppKey, MidiStateEntry};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tracing::debug;

/// State snapshot for JSON serialization
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    /// Timestamp of snapshot creation (milliseconds since epoch)
    pub timestamp: u64,
    /// Version of the snapshot format
    pub version: String,
    /// State entries per application
    pub states: HashMap<AppKey, Vec<MidiStateEntry>>,
}

impl StateSnapshot {
    /// Current snapshot format version
    pub const VERSION: &'static str = "1.0.0";

    /// Create a new snapshot from current StateStore
    pub fn from_store(store: &StateStore) -> Self {
        let mut states = HashMap::new();
        for app in AppKey::all() {
            let app_states = store.list_states_for_app(*app);
            if !app_states.is_empty() {
                states.insert(*app, app_states);
            }
        }

        Self {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            version: Self::VERSION.to_string(),
            states,
        }
    }

    /// Load snapshot into a StateStore
    pub fn load_into_store(&self, store: &StateStore) {
        for (app, entries) in &self.states {
            debug!("Loading {} state entries for app: {}", entries.len(), app);
            store.hydrate_from_snapshot(*app, entries.clone());
        }
    }

    /// Save snapshot to JSON file
    pub async fn save_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self)
            .context("Failed to serialize state snapshot")?;

        fs::write(path, json)
            .await
            .context("Failed to write state snapshot to file")?;

        //info!("State snapshot saved to: {}", path.display());
        Ok(())
    }

    /// Load snapshot from JSON file
    pub async fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let json = fs::read_to_string(path)
            .await
            .context("Failed to read state snapshot file")?;

        let snapshot: StateSnapshot = serde_json::from_str(&json)
            .context("Failed to parse state snapshot JSON")?;

        debug!(
            "State snapshot loaded (version: {}, timestamp: {})",
            snapshot.version, snapshot.timestamp
        );

        Ok(snapshot)
    }
}

impl StateStore {
    /// Save current state to JSON file
    pub async fn save_snapshot(&self, path: impl AsRef<Path>) -> Result<()> {
        let snapshot = StateSnapshot::from_store(self);
        snapshot.save_to_file(path).await
    }

    /// Load state from JSON file
    pub async fn load_snapshot(&self, path: impl AsRef<Path>) -> Result<()> {
        let snapshot = StateSnapshot::load_from_file(path).await?;
        snapshot.load_into_store(self);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::{MidiAddr, MidiStatus, MidiValue, Origin};
    use tempfile::NamedTempFile;

    fn make_test_entry(app: AppKey, status: MidiStatus, value: u16) -> MidiStateEntry {
        MidiStateEntry {
            addr: MidiAddr {
                port_id: app.as_str().to_string(),
                status,
                channel: Some(1),
                data1: Some(7),
            },
            value: MidiValue::Number(value),
            ts: 1000,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        }
    }

    #[tokio::test]
    async fn test_snapshot_save_load() {
        let store = StateStore::new();

        // Add some state
        let entry1 = make_test_entry(AppKey::Voicemeeter, MidiStatus::CC, 100);
        let entry2 = make_test_entry(AppKey::Obs, MidiStatus::PB, 8192);
        store.update_from_feedback(AppKey::Voicemeeter, entry1);
        store.update_from_feedback(AppKey::Obs, entry2);

        // Save to file
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();
        store.save_snapshot(path).await.unwrap();

        // Create new store and load
        let new_store = StateStore::new();
        new_store.load_snapshot(path).await.unwrap();

        // Verify state was loaded
        let loaded_vm = new_store.list_states_for_app(AppKey::Voicemeeter);
        assert_eq!(loaded_vm.len(), 1);
        assert_eq!(loaded_vm[0].value.as_number(), Some(100));
        assert!(loaded_vm[0].stale); // Should be marked stale

        let loaded_obs = new_store.list_states_for_app(AppKey::Obs);
        assert_eq!(loaded_obs.len(), 1);
        assert_eq!(loaded_obs[0].value.as_number(), Some(8192));
    }

    #[tokio::test]
    async fn test_snapshot_version() {
        let store = StateStore::new();
        let snapshot = StateSnapshot::from_store(&store);
        assert_eq!(snapshot.version, StateSnapshot::VERSION);
    }

    #[tokio::test]
    async fn test_empty_snapshot() {
        let store = StateStore::new();
        let snapshot = StateSnapshot::from_store(&store);
        assert!(snapshot.states.is_empty());
    }
}

