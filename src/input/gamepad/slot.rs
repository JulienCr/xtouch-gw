//! Gamepad slot management for multi-gamepad support
//!
//! Each slot represents a configured gamepad position with:
//! - Slot index (0-based) → control ID prefix (gamepad1, gamepad2, etc.)
//! - Product pattern to match
//! - Connection state (current gamepad ID or None)
//! - Per-slot analog configuration

use gilrs::Gilrs;
use std::time::Instant;
use crate::config::AnalogConfig;
use tracing::{debug, info, trace, warn};
use super::hybrid_id::HybridControllerId;

/// Represents a configured gamepad slot
#[derive(Debug, Clone)]
pub struct GamepadSlot {
    /// Slot index (0-based, used to generate gamepad1, gamepad2, etc.)
    pub slot_index: usize,

    /// Product pattern to match for this slot (substring, case-insensitive)
    pub product_match: String,

    /// Currently connected gamepad ID (None if disconnected)
    /// Uses HybridControllerId to support both gilrs and XInput backends
    pub connected_id: Option<HybridControllerId>,

    /// Product name of currently connected gamepad
    pub connected_name: Option<String>,

    /// Timestamp of last connection/disconnection event
    pub last_change: Instant,

    /// Analog configuration for this slot
    pub analog_config: Option<AnalogConfig>,
}

impl GamepadSlot {
    /// Create a new gamepad slot
    pub fn new(slot_index: usize, product_match: String, analog_config: Option<AnalogConfig>) -> Self {
        Self {
            slot_index,
            product_match,
            connected_id: None,
            connected_name: None,
            last_change: Instant::now(),
            analog_config,
        }
    }

    /// Check if this slot matches the given gamepad name
    pub fn matches(&self, gamepad_name: &str) -> bool {
        gamepad_name.to_lowercase().contains(&self.product_match.to_lowercase())
    }

    /// Get control ID prefix for this slot (e.g., "gamepad1", "gamepad2")
    pub fn control_id_prefix(&self) -> String {
        format!("gamepad{}", self.slot_index + 1)
    }

    /// Connect a gamepad to this slot
    pub fn connect(&mut self, id: HybridControllerId, name: String) {
        self.connected_id = Some(id);
        self.connected_name = Some(name);
        self.last_change = Instant::now();
    }

    /// Disconnect gamepad from this slot
    pub fn disconnect(&mut self) {
        self.connected_id = None;
        self.connected_name = None;
        self.last_change = Instant::now();
    }

    /// Check if gamepad is currently connected
    pub fn is_connected(&self) -> bool {
        self.connected_id.is_some()
    }
}

/// Manages gamepad slot assignments
pub struct SlotManager {
    slots: Vec<GamepadSlot>,
}

impl SlotManager {
    /// Create a new slot manager with the given slot configurations
    ///
    /// # Arguments
    /// * `slot_configs` - Vector of (product_match, analog_config) tuples
    pub fn new(slot_configs: Vec<(String, Option<AnalogConfig>)>) -> Self {
        let slots = slot_configs
            .into_iter()
            .enumerate()
            .map(|(idx, (product_match, analog))| {
                GamepadSlot::new(idx, product_match, analog)
            })
            .collect();

        Self { slots }
    }

    /// Find which slot (if any) should handle this gamepad
    ///
    /// # Returns
    /// * `Some((slot_index, is_already_connected))` if a matching slot is found
    /// * `None` if no matching slot exists
    pub fn find_slot_for_gamepad(&self, id: HybridControllerId, name: &str) -> Option<(usize, bool)> {
        // Check if already connected to a slot (shouldn't happen, but handle it)
        if let Some(idx) = self.slots.iter().position(|s| s.connected_id == Some(id)) {
            trace!("Gamepad {} already connected to slot {}", id.to_string(), idx);
            return Some((idx, true));
        }

        // Find first matching disconnected slot
        if let Some(idx) = self.slots.iter().position(|s| !s.is_connected() && s.matches(name)) {
            debug!("Gamepad \"{}\" matches slot {} (pattern: \"{}\")",
                name, idx, self.slots[idx].product_match);
            return Some((idx, false));
        }

        // No matching slot found
        debug!("Gamepad \"{}\" doesn't match any configured slot", name);
        None
    }

    /// Get slot by gamepad ID
    pub fn get_slot_by_id(&self, id: HybridControllerId) -> Option<&GamepadSlot> {
        self.slots.iter().find(|s| s.connected_id == Some(id))
    }

    /// Get mutable slot by index
    pub fn get_slot_mut(&mut self, idx: usize) -> Option<&mut GamepadSlot> {
        self.slots.get_mut(idx)
    }

    /// Get all slots (immutable)
    pub fn slots(&self) -> &[GamepadSlot] {
        &self.slots
    }

    /// Check if gilrs-managed gamepads are still present and update slots
    ///
    /// # Returns
    /// Vector of (slot_index, gamepad_name) for disconnected gamepads
    pub fn check_gilrs_disconnections(&mut self, gilrs: &Gilrs) -> Vec<(usize, String)> {
        let mut disconnected = Vec::new();

        for slot in &mut self.slots {
            // Only check gilrs-managed controllers
            if let Some(HybridControllerId::Gilrs(id)) = slot.connected_id {
                if gilrs.connected_gamepad(id).is_none() {
                    let name = slot.connected_name.clone().unwrap_or_else(|| "Unknown".to_string());
                    warn!("🔌 Gamepad {} disconnected: {}", slot.control_id_prefix(), name);
                    slot.disconnect();
                    disconnected.push((slot.slot_index, name));
                }
            }
        }

        disconnected
    }

    /// Check if XInput-managed gamepads are still present and update slots
    ///
    /// # Arguments
    /// * `active_indices` - List of currently connected XInput user indices (0-3)
    ///
    /// # Returns
    /// Vector of (slot_index, gamepad_name) for disconnected gamepads
    pub fn check_xinput_disconnections(&mut self, active_indices: &[usize]) -> Vec<(usize, String)> {
        let mut disconnected = Vec::new();

        for slot in &mut self.slots {
            // Only check XInput-managed controllers
            if let Some(HybridControllerId::XInput(idx)) = slot.connected_id {
                if !active_indices.contains(&idx) {
                    let name = slot.connected_name.clone().unwrap_or_else(|| "Unknown".to_string());
                    warn!("🔌 Gamepad {} disconnected: {}", slot.control_id_prefix(), name);
                    slot.disconnect();
                    disconnected.push((slot.slot_index, name));
                }
            }
        }

        disconnected
    }

    /// Attempt to connect a gamepad to an appropriate slot
    ///
    /// # Returns
    /// `Some(slot_index)` if successfully connected, `None` otherwise
    pub fn try_connect(&mut self, id: HybridControllerId, name: &str) -> Option<usize> {
        if let Some((slot_idx, is_reconnect)) = self.find_slot_for_gamepad(id, name) {
            if let Some(slot) = self.get_slot_mut(slot_idx) {
                if !slot.is_connected() {
                    slot.connect(id, name.to_string());
                    info!("✅ Gamepad {} {}connected: {} (ID: {})",
                        slot.control_id_prefix(),
                        if is_reconnect { "re" } else { "" },
                        name,
                        id.to_string()
                    );
                    return Some(slot_idx);
                } else if slot.connected_id == Some(id) {
                    // Already connected to this slot, silently ignore
                    return Some(slot_idx);
                } else {
                    warn!("⚠️  Gamepad \"{}\" matches slot {} but already occupied by \"{}\"",
                        name, slot_idx + 1, slot.connected_name.as_deref().unwrap_or("Unknown"));
                }
            }
        } else {
            debug!("Gamepad \"{}\" doesn't match any configured slot", name);
        }

        None
    }

    /// Get the number of slots
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Check if there are no slots
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilrs::GamepadId;

    #[test]
    fn test_slot_matching() {
        let slot = GamepadSlot::new(0, "Xbox".to_string(), None);
        assert!(slot.matches("Xbox Wireless Controller"));
        assert!(slot.matches("Microsoft Xbox One"));
        assert!(slot.matches("xbox elite")); // Case insensitive
        assert!(!slot.matches("Nintendo Switch"));
    }

    #[test]
    fn test_slot_control_id_prefix() {
        let slot1 = GamepadSlot::new(0, "Xbox".to_string(), None);
        let slot2 = GamepadSlot::new(1, "Nintendo".to_string(), None);

        assert_eq!(slot1.control_id_prefix(), "gamepad1");
        assert_eq!(slot2.control_id_prefix(), "gamepad2");
    }

    #[test]
    fn test_slot_manager_assignment() {
        let mut manager = SlotManager::new(vec![
            ("Xbox".to_string(), None),
            ("Nintendo".to_string(), None),
        ]);

        // Simulate hybrid gamepad IDs
        let xbox_id = HybridControllerId::from_xinput(0);
        let switch_id = HybridControllerId::from_gilrs(GamepadId::from(0));
        let ps_id = HybridControllerId::from_gilrs(GamepadId::from(1));

        // Xbox should match slot 0
        let (slot_idx, _) = manager.find_slot_for_gamepad(xbox_id, "XInput Controller 1").unwrap();
        assert_eq!(slot_idx, 0);
        manager.get_slot_mut(slot_idx).unwrap().connect(xbox_id, "XInput Controller 1".to_string());

        // Switch should match slot 1
        let (slot_idx, _) = manager.find_slot_for_gamepad(switch_id, "Nintendo Switch Pro").unwrap();
        assert_eq!(slot_idx, 1);

        // PS4 should find no slot
        assert!(manager.find_slot_for_gamepad(ps_id, "PS4 Controller").is_none());
    }

    #[test]
    fn test_slot_preservation() {
        let mut manager = SlotManager::new(vec![
            ("XInput".to_string(), None),
        ]);

        let id = HybridControllerId::from_xinput(0);

        // Connect
        let (idx, _) = manager.find_slot_for_gamepad(id, "XInput Controller 1").unwrap();
        manager.get_slot_mut(idx).unwrap().connect(id, "XInput Controller 1".to_string());
        assert!(manager.get_slot_by_id(id).unwrap().is_connected());

        // Disconnect
        manager.get_slot_mut(idx).unwrap().disconnect();
        assert!(!manager.get_slot_mut(idx).unwrap().is_connected());

        // Reconnect to same slot
        let (new_idx, _) = manager.find_slot_for_gamepad(id, "XInput Controller 1").unwrap();
        assert_eq!(idx, new_idx);
    }
}
