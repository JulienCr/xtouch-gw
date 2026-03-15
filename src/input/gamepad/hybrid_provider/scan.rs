//! Gamepad scanning, connection management, and reconnection logic

use gilrs::{Event, EventType};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

use super::HybridProviderState;
use crate::input::gamepad::hybrid_id::HybridControllerId;
use crate::input::gamepad::xinput_convert::poll_xinput_controller;

impl HybridProviderState {
    /// Initial scan for gamepads (wait for Bluetooth enumeration)
    pub(super) fn initial_scan(&mut self) {
        debug!("Scanning for gamepads...");
        debug!("Waiting for gamepad enumeration (3 seconds)...");

        let scan_start = Instant::now();
        let scan_duration = Duration::from_secs(3);

        while scan_start.elapsed() < scan_duration {
            while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
                if event == EventType::Connected {
                    trace!("gilrs gamepad connected during initial scan: {:?}", id);
                }
            }

            if self.xinput_available {
                self.scan_xinput_controllers();
            }

            std::thread::sleep(Duration::from_millis(100));
        }

        self.report_connected_gamepads();
        self.assign_initial_slots();
    }

    /// Assign initial slot connections after scan completes
    fn assign_initial_slots(&mut self) {
        let Some(ref mut manager) = self.slot_manager else {
            return;
        };

        let xinput_has_controllers = self.xinput_connected.iter().any(|&c| c);

        // Connect gilrs gamepads
        for (id, gamepad) in self.gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
            let name = gamepad.name();
            let hybrid_id = HybridControllerId::from_gilrs(id);

            if xinput_has_controllers && Self::is_xbox_name(name) {
                debug!(
                    "Skipping gilrs detection of Xbox controller (using XInput instead): {}",
                    name
                );
                continue;
            }

            manager.try_connect(hybrid_id, name);
        }

        // Connect XInput gamepads
        if let Some(ref handle) = self.xinput_handle {
            for user_index in 0..4u32 {
                if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                    let idx = user_index as usize;
                    let name = format!("XInput Controller {}", user_index + 1);
                    let hybrid_id = HybridControllerId::from_xinput(idx);
                    manager.try_connect(hybrid_id, &name);
                    self.xinput_connected[idx] = true;
                }
            }
        }
    }

    /// Scan XInput controllers during initial enumeration
    fn scan_xinput_controllers(&mut self) {
        if let Some(ref handle) = self.xinput_handle {
            for user_index in 0..4u32 {
                if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                    let idx = user_index as usize;
                    if !self.xinput_connected[idx] {
                        debug!("XInput controller {} detected during scan", user_index);
                    }
                }
            }
        }
    }

    /// Report connected gamepads after initial scan
    fn report_connected_gamepads(&mut self) {
        let gilrs_count = self
            .gilrs
            .gamepads()
            .filter(|(_, gp)| gp.is_connected())
            .count();

        let mut xinput_count = 0;
        if let Some(ref handle) = self.xinput_handle {
            for user_index in 0..4u32 {
                if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                    xinput_count += 1;
                }
            }
        }

        if gilrs_count == 0 && xinput_count == 0 {
            tracing::warn!("No gamepads detected at all");
        } else {
            debug!(
                "Found {} gilrs gamepad(s) and {} XInput gamepad(s):",
                gilrs_count, xinput_count
            );

            let xinput_has_controllers = self.xinput_connected.iter().any(|&c| c);
            for (id, gamepad) in self.gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
                let name = gamepad.name();
                if xinput_has_controllers && Self::is_xbox_name(name) {
                    debug!("  - gilrs {:?}: \"{}\" (will use XInput instead)", id, name);
                } else {
                    debug!("  - gilrs {:?}: \"{}\"", id, name);
                }
            }

            if let Some(ref handle) = self.xinput_handle {
                for user_index in 0..4u32 {
                    if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                        debug!(
                            "  - XInput {}: \"XInput Controller {}\"",
                            user_index,
                            user_index + 1
                        );
                    }
                }
            }
        }
    }

    /// Check all connections for both backends (called every 2 seconds)
    pub(super) fn check_all_connections(&mut self) {
        self.last_reconnect_check = Instant::now();

        let Some(ref mut manager) = self.slot_manager else {
            return;
        };

        // Check gilrs disconnections
        manager.check_gilrs_disconnections(&self.gilrs);

        // Check XInput disconnections
        let mut active_indices = Vec::new();
        if let Some(ref handle) = self.xinput_handle {
            for user_index in 0..4u32 {
                if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                    active_indices.push(user_index as usize);
                } else {
                    let idx = user_index as usize;
                    if self.xinput_connected[idx] {
                        self.xinput_connected[idx] = false;
                        self.last_xinput_state[idx] = None;
                    }
                }
            }
        }
        manager.check_xinput_disconnections(&active_indices);

        // Try to reconnect empty slots with gilrs
        let xinput_has_controllers = self.xinput_connected.iter().any(|&c| c);
        for (id, gamepad) in self.gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
            let name = gamepad.name();
            let hybrid_id = HybridControllerId::from_gilrs(id);

            if xinput_has_controllers && Self::is_xbox_name(name) {
                continue;
            }

            manager.try_connect(hybrid_id, name);
        }

        // Try to reconnect empty slots with XInput
        if let Some(ref handle) = self.xinput_handle {
            for user_index in 0..4u32 {
                if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                    let idx = user_index as usize;
                    if !self.xinput_connected[idx] {
                        let name = format!("XInput Controller {}", user_index + 1);
                        let hybrid_id = HybridControllerId::from_xinput(idx);
                        manager.try_connect(hybrid_id, &name);
                        self.xinput_connected[idx] = true;
                    }
                }
            }
        }
    }
}
