//! Camera target state management for dynamic gamepad-to-camera mapping
//!
//! This module manages which camera each gamepad controls at runtime,
//! with persistence to sled for crash recovery.

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

/// Prefix for camera target keys in sled database
const CAMERA_TARGET_PREFIX: &str = "camera_target:";

/// Camera target state for dynamic gamepad-to-camera mapping
///
/// Stores which camera each gamepad controls. Persisted to sled for crash recovery.
/// Also tracks transient PTZ modifier state (not persisted).
pub struct CameraTargetState {
    /// Current camera target per gamepad slot (e.g., "gamepad1" -> "Main")
    targets: RwLock<HashMap<String, String>>,
    /// PTZ modifier held state per gamepad slot (e.g., "gamepad1" -> true when LT held)
    /// Transient state - not persisted to sled
    ptz_modifier_held: RwLock<HashMap<String, bool>>,
    /// Sled database handle for persistence
    db: sled::Db,
}

/// Serialized camera target for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CameraTargetEntry {
    pub gamepad_slot: String,
    pub camera_id: String,
    pub timestamp: u64,
}

impl CameraTargetState {
    /// Create a new CameraTargetState with the given sled database
    pub fn new(db: sled::Db) -> Self {
        let state = Self {
            targets: RwLock::new(HashMap::new()),
            ptz_modifier_held: RwLock::new(HashMap::new()),
            db,
        };

        // Load persisted targets on creation
        state.load_from_db();
        state
    }

    /// Load all persisted camera targets from sled
    fn load_from_db(&self) {
        let mut targets = self.targets.write();

        for result in self.db.scan_prefix(CAMERA_TARGET_PREFIX) {
            match result {
                Ok((key, value)) => {
                    if let Ok(key_str) = std::str::from_utf8(&key) {
                        // Extract gamepad_slot from key (after prefix)
                        let gamepad_slot = key_str
                            .strip_prefix(CAMERA_TARGET_PREFIX)
                            .unwrap_or(key_str);

                        if let Ok(entry) = serde_json::from_slice::<CameraTargetEntry>(&value) {
                            debug!(
                                "Restored camera target: {} -> {}",
                                gamepad_slot, entry.camera_id
                            );
                            targets.insert(gamepad_slot.to_string(), entry.camera_id);
                        }
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read camera target from sled: {}", e);
                },
            }
        }

        info!("Loaded {} camera target(s) from persistence", targets.len());
    }

    /// Set the camera target for a gamepad slot
    ///
    /// Persists to sled immediately.
    pub fn set_target(&self, gamepad_slot: &str, camera_id: &str) -> Result<()> {
        // Update in-memory state
        {
            let mut targets = self.targets.write();
            targets.insert(gamepad_slot.to_string(), camera_id.to_string());
        }

        // Persist to sled
        let entry = CameraTargetEntry {
            gamepad_slot: gamepad_slot.to_string(),
            camera_id: camera_id.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };

        let key = format!("{}{}", CAMERA_TARGET_PREFIX, gamepad_slot);
        let value = serde_json::to_vec(&entry).context("Failed to serialize camera target")?;

        self.db
            .insert(key.as_bytes(), value)
            .context("Failed to persist camera target to sled")?;

        Ok(())
    }

    /// Get the current camera target for a gamepad slot
    pub fn get_target(&self, gamepad_slot: &str) -> Option<String> {
        let targets = self.targets.read();
        targets.get(gamepad_slot).cloned()
    }

    /// Get all current camera targets
    pub fn get_all_targets(&self) -> HashMap<String, String> {
        self.targets.read().clone()
    }

    /// Clear the camera target for a gamepad slot
    pub fn clear_target(&self, gamepad_slot: &str) -> Result<()> {
        // Remove from in-memory state
        {
            let mut targets = self.targets.write();
            targets.remove(gamepad_slot);
        }

        // Remove from sled
        let key = format!("{}{}", CAMERA_TARGET_PREFIX, gamepad_slot);
        self.db
            .remove(key.as_bytes())
            .context("Failed to remove camera target from sled")?;

        debug!("Cleared camera target for {}", gamepad_slot);
        Ok(())
    }

    /// Set the PTZ modifier state for a gamepad slot
    ///
    /// This is transient state (not persisted). Used to track when the
    /// modifier button (e.g., LT) is held to switch camera selection to preview mode.
    pub fn set_ptz_modifier(&self, gamepad_slot: &str, held: bool) {
        let mut modifiers = self.ptz_modifier_held.write();
        modifiers.insert(gamepad_slot.to_string(), held);
        debug!("PTZ modifier: {} = {}", gamepad_slot, held);
    }

    /// Check if the PTZ modifier is currently held for a gamepad slot
    pub fn is_ptz_modifier_held(&self, gamepad_slot: &str) -> bool {
        let modifiers = self.ptz_modifier_held.read();
        modifiers.get(gamepad_slot).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_set_and_get_target() {
        let temp = tempdir().unwrap();
        let db = sled::open(temp.path().join("test.sled")).unwrap();
        let state = CameraTargetState::new(db);

        state.set_target("gamepad1", "Main").unwrap();
        assert_eq!(state.get_target("gamepad1"), Some("Main".to_string()));
        assert_eq!(state.get_target("gamepad2"), None);
    }

    #[test]
    fn test_persistence_across_restarts() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sled");

        // First instance - set target
        {
            let db = sled::open(&db_path).unwrap();
            let state = CameraTargetState::new(db);
            state.set_target("gamepad1", "Jardin").unwrap();
        }

        // Second instance - should restore target
        {
            let db = sled::open(&db_path).unwrap();
            let state = CameraTargetState::new(db);
            assert_eq!(state.get_target("gamepad1"), Some("Jardin".to_string()));
        }
    }

    #[test]
    fn test_clear_target() {
        let temp = tempdir().unwrap();
        let db = sled::open(temp.path().join("test.sled")).unwrap();
        let state = CameraTargetState::new(db);

        state.set_target("gamepad1", "Main").unwrap();
        assert!(state.get_target("gamepad1").is_some());

        state.clear_target("gamepad1").unwrap();
        assert!(state.get_target("gamepad1").is_none());
    }
}
