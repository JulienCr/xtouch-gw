//! Hybrid gamepad provider combining XInput and gilrs (WGI) backends
//!
//! This provider polls both XInput (for Xbox controllers) and gilrs with WGI backend
//! (for non-XInput controllers like FaceOff) simultaneously, enabling support for
//! multiple controller types in a headless tray application.

use anyhow::Result;
use gilrs::{Gilrs, Event, EventType};
use rusty_xinput::XInputHandle;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{warn, debug, trace};

use crate::config::AnalogConfig;
use super::hybrid_id::HybridControllerId;
use super::provider::GamepadEvent;
use super::slot::SlotManager;
use super::xinput_convert::{
    CachedXInputState, convert_xinput_buttons, convert_xinput_axes, poll_xinput_controller
};

/// Callback type for gamepad events
pub type EventCallback = Arc<dyn Fn(GamepadEvent) + Send + Sync>;

/// Hybrid gamepad provider with XInput and gilrs (WGI) support
pub struct HybridGamepadProvider {
    event_listeners: Arc<RwLock<Vec<EventCallback>>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl HybridGamepadProvider {
    /// Create and start a new hybrid gamepad provider
    ///
    /// # Arguments
    /// * `slot_configs` - Vector of (product_match, analog_config) tuples for each slot
    ///
    /// # Returns
    /// Running provider instance
    pub async fn start(slot_configs: Vec<(String, Option<AnalogConfig>)>) -> Result<Self> {
        let event_listeners = Arc::new(RwLock::new(Vec::<EventCallback>::new()));
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

        // Create a channel for sending events from blocking thread to async world
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<GamepadEvent>();

        // Spawn blocking event loop in a dedicated thread
        std::thread::spawn(move || {
            Self::event_loop_blocking(slot_configs, event_tx, shutdown_rx);
        });

        // Spawn async task to forward events to listeners
        let listeners_clone = event_listeners.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let listeners = listeners_clone.read().await;
                for callback in listeners.iter() {
                    callback(event.clone());
                }
            }
        });

        Ok(Self {
            event_listeners,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Subscribe to gamepad events
    pub async fn subscribe(&self, callback: EventCallback) {
        let mut listeners = self.event_listeners.write().await;
        listeners.push(callback);
    }

    /// Main event loop (runs in dedicated blocking thread)
    fn event_loop_blocking(
        slot_configs: Vec<(String, Option<AnalogConfig>)>,
        event_tx: mpsc::UnboundedSender<GamepadEvent>,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) {
        // Initialize state
        let mut state = match HybridProviderState::new(slot_configs) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to initialize hybrid gamepad provider: {}", e);
                return;
            }
        };

        // Initial scan (3 seconds for Bluetooth controllers)
        state.initial_scan();

        // Event loop timing
        let loop_interval = Duration::from_millis(16); // ~60 Hz
        let mut last_loop = Instant::now();

        loop {
            // Check for shutdown signal (non-blocking)
            match shutdown_rx.try_recv() {
                Ok(_) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    debug!("Hybrid gamepad provider shutting down");
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }

            // Reconnection check (every 2 seconds)
            if state.should_check_reconnection() {
                state.check_all_connections();
            }

            // Poll XInput controllers
            if state.xinput_available {
                state.poll_xinput_events(&event_tx);
            }

            // Poll gilrs events (non-blocking)
            state.poll_gilrs_events(&event_tx);

            // Sleep to maintain ~60 Hz
            let elapsed = last_loop.elapsed();
            if elapsed < loop_interval {
                std::thread::sleep(loop_interval - elapsed);
            }
            last_loop = Instant::now();
        }
    }

    /// Shutdown the provider
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
            debug!("Hybrid gamepad provider shutdown requested");
        }
        Ok(())
    }
}

impl Drop for HybridGamepadProvider {
    fn drop(&mut self) {
        // Attempt to send shutdown signal if not already sent
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
    }
}

/// Internal state for the blocking event loop
struct HybridProviderState {
    gilrs: Gilrs,
    xinput_handle: Option<XInputHandle>,
    xinput_available: bool,
    slot_manager: Option<SlotManager>,
    last_xinput_state: [Option<CachedXInputState>; 4],
    xinput_connected: [bool; 4],
    last_reconnect_check: Instant,
    /// Track last gilrs axis values to detect return-to-zero
    last_gilrs_axis_values: HashMap<(gilrs::GamepadId, gilrs::Axis), f32>,
    /// Monotonic sequence counter for axis events (prevents race conditions)
    axis_sequence: u64,
}

impl HybridProviderState {
    /// Initialize the hybrid provider state
    fn new(slot_configs: Vec<(String, Option<AnalogConfig>)>) -> Result<Self> {
        // Initialize gilrs (always required)
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                debug!("gilrs initialized (WGI backend enabled)");
                g
            }
            Err(e) => {
                warn!("Failed to initialize gilrs: {:?}", e);
                return Err(anyhow::anyhow!("gilrs initialization failed: {}", e));
            }
        };

        // Try to initialize XInput (optional, graceful fallback)
        let (xinput_handle, xinput_available) = match XInputHandle::load_default() {
            Ok(handle) => {
                debug!("XInput initialized successfully");
                (Some(handle), true)
            }
            Err(e) => {
                warn!("XInput library not available (falling back to WGI-only): {:?}", e);
                (None, false)
            }
        };

        // Create slot manager (or use legacy mode if empty)
        let slot_manager = if slot_configs.is_empty() {
            None
        } else {
            Some(SlotManager::new(slot_configs))
        };

        Ok(Self {
            gilrs,
            xinput_handle,
            xinput_available,
            slot_manager,
            last_xinput_state: [None, None, None, None],
            xinput_connected: [false; 4],
            last_reconnect_check: Instant::now(),
            last_gilrs_axis_values: HashMap::new(),
            axis_sequence: 0,
        })
    }

    /// Initial scan for gamepads (wait for Bluetooth enumeration)
    fn initial_scan(&mut self) {
        debug!("Scanning for gamepads...");
        debug!("Waiting for gamepad enumeration (3 seconds)...");

        let scan_start = Instant::now();
        let scan_duration = Duration::from_secs(3);

        // Poll events during initial scan to trigger connection detection
        while scan_start.elapsed() < scan_duration {
            // Process gilrs events
            while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
                match event {
                    EventType::Connected => {
                        trace!("gilrs gamepad connected during initial scan: {:?}", id);
                    }
                    _ => {}
                }
            }

            // Check XInput controllers
            if self.xinput_available {
                self.scan_xinput_controllers();
            }

            std::thread::sleep(Duration::from_millis(100));
        }

        // Report what was found
        self.report_connected_gamepads();

        // Attempt initial slot assignments
        if let Some(ref mut manager) = self.slot_manager {
            // Check which XInput slots are connected (for duplicate detection)
            let xinput_has_controllers = self.xinput_connected.iter().any(|&c| c);

            // Connect gilrs gamepads
            for (id, gamepad) in self.gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
                let name = gamepad.name();
                let hybrid_id = HybridControllerId::from_gilrs(id);

                // Check if this is a duplicate Xbox controller (prefer XInput)
                if xinput_has_controllers && Self::is_xbox_name(name) {
                    debug!("Skipping gilrs detection of Xbox controller (using XInput instead): {}", name);
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

    /// Check if a gamepad name indicates an Xbox controller (static helper)
    fn is_xbox_name(name: &str) -> bool {
        let name_lower = name.to_lowercase();

        // Check if name suggests Xbox controller
        name_lower.contains("xbox") ||
        name_lower.contains("xinput") ||
        name_lower.contains("x-box") ||
        name_lower.contains("microsoft")
    }

    /// Report connected gamepads after initial scan
    fn report_connected_gamepads(&mut self) {
        // Count gilrs gamepads
        let gilrs_count = self.gilrs.gamepads()
            .filter(|(_, gp)| gp.is_connected())
            .count();

        // Count XInput gamepads
        let mut xinput_count = 0;
        if let Some(ref handle) = self.xinput_handle {
            for user_index in 0..4u32 {
                if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                    xinput_count += 1;
                }
            }
        }

        if gilrs_count == 0 && xinput_count == 0 {
            warn!("⚠️  No gamepads detected at all");
        } else {
            debug!("Found {} gilrs gamepad(s) and {} XInput gamepad(s):", gilrs_count, xinput_count);

            // List gilrs gamepads
            let xinput_has_controllers = self.xinput_connected.iter().any(|&c| c);
            for (id, gamepad) in self.gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
                let name = gamepad.name();
                if xinput_has_controllers && Self::is_xbox_name(name) {
                    debug!("  - gilrs {:?}: \"{}\" (will use XInput instead)", id, name);
                } else {
                    debug!("  - gilrs {:?}: \"{}\"", id, name);
                }
            }

            // List XInput gamepads
            if let Some(ref handle) = self.xinput_handle {
                for user_index in 0..4u32 {
                    if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                        debug!("  - XInput {}: \"XInput Controller {}\"", user_index, user_index + 1);
                    }
                }
            }
        }
    }

    /// Check if it's time for a reconnection check
    fn should_check_reconnection(&self) -> bool {
        self.last_reconnect_check.elapsed() >= Duration::from_secs(2)
    }

    /// Check all connections for both backends
    fn check_all_connections(&mut self) {
        self.last_reconnect_check = Instant::now();

        if let Some(ref mut manager) = self.slot_manager {
            // Check gilrs disconnections
            manager.check_gilrs_disconnections(&self.gilrs);

            // Check XInput disconnections
            let mut active_indices = Vec::new();
            if let Some(ref handle) = self.xinput_handle {
                for user_index in 0..4u32 {
                    if let Ok(Some(_)) = poll_xinput_controller(handle, user_index) {
                        active_indices.push(user_index as usize);
                    } else {
                        // Controller disconnected
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

                // Skip Xbox duplicates
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

    /// Poll gilrs events (non-blocking)
    fn poll_gilrs_events(&mut self, event_tx: &mpsc::UnboundedSender<GamepadEvent>) {
        // Process all available gilrs events
        while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
            let hybrid_id = HybridControllerId::from_gilrs(id);

            // Find slot for this controller
            let (prefix, analog_config) = if let Some(ref manager) = self.slot_manager {
                if let Some(slot) = manager.get_slot_by_id(hybrid_id) {
                    (slot.control_id_prefix(), slot.analog_config.clone())
                } else {
                    // Event from unregistered gamepad, ignore
                    continue;
                }
            } else {
                // Legacy mode: no slot manager, use "gamepad" prefix
                ("gamepad".to_string(), None)
            };

            // Convert event with slot prefix (with zero-detection and sequence)
            if let Some(gamepad_event) = self.convert_gilrs_event(id, event, &prefix, analog_config) {
                debug!("gilrs event: {:?}", gamepad_event);

                if event_tx.send(gamepad_event).is_err() {
                    warn!("Event receiver dropped, shutting down gamepad loop");
                    break;
                }
            }
        }
    }

    /// Convert gilrs event to GamepadEvent with axis return-to-zero detection
    fn convert_gilrs_event(
        &mut self,
        id: gilrs::GamepadId,
        event: EventType,
        prefix: &str,
        analog_config: Option<AnalogConfig>
    ) -> Option<GamepadEvent> {
        use gilrs::Axis;

        match event {
            EventType::ButtonPressed(button, _) => {
                Some(GamepadEvent::Button {
                    control_id: Self::button_to_id(button, prefix),
                    pressed: true,
                })
            }
            EventType::ButtonReleased(button, _) => {
                Some(GamepadEvent::Button {
                    control_id: Self::button_to_id(button, prefix),
                    pressed: false,
                })
            }
            EventType::AxisChanged(axis, value, _) => {
                // Normalize Y-axis convention to match HID behavior
                let normalized_value = match axis {
                    Axis::LeftStickY | Axis::RightStickY => -value,
                    _ => value,
                };

                // Track axis value for zero-detection
                let key = (id, axis);
                let last_value = self.last_gilrs_axis_values.get(&key).copied();

                // Check if axis is returning to zero (within threshold)
                const ZERO_THRESHOLD: f32 = 0.05;
                let is_near_zero = normalized_value.abs() < ZERO_THRESHOLD;
                let was_non_zero = last_value.map_or(false, |v| v.abs() >= ZERO_THRESHOLD);

                // If axis is returning to center from non-zero position, emit explicit 0.0
                if is_near_zero {
                    if was_non_zero {
                        debug!("gilrs axis {} returning to zero (was {:.3}, now {:.3})",
                               Self::axis_to_id(axis, prefix),
                               last_value.unwrap_or(0.0),
                               normalized_value);
                        // Remove from tracking (axis is now at rest)
                        self.last_gilrs_axis_values.remove(&key);
                        // Increment sequence for ordering
                        self.axis_sequence += 1;
                        // Emit explicit 0.0 event
                        return Some(GamepadEvent::Axis {
                            control_id: Self::axis_to_id(axis, prefix),
                            value: 0.0,
                            analog_config,
                            sequence: self.axis_sequence,
                        });
                    }
                    // Already at zero, no event needed
                    return None;
                } else {
                    // Non-zero value, track it
                    self.last_gilrs_axis_values.insert(key, normalized_value);
                    // Increment sequence for ordering
                    self.axis_sequence += 1;
                    Some(GamepadEvent::Axis {
                        control_id: Self::axis_to_id(axis, prefix),
                        value: normalized_value,
                        analog_config,
                        sequence: self.axis_sequence,
                    })
                }
            }
            EventType::Connected => {
                trace!("gilrs gamepad connected event");
                None
            }
            EventType::Disconnected => {
                debug!("gilrs gamepad disconnected event");
                // Clean up axis tracking for disconnected gamepad
                self.last_gilrs_axis_values.retain(|(gp_id, _), _| *gp_id != id);
                None
            }
            _ => None,
        }
    }

    /// Map gilrs button to standardized control ID (same as provider.rs)
    fn button_to_id(button: gilrs::Button, prefix: &str) -> String {
        use gilrs::Button;

        let name = match button {
            Button::South => "a",
            Button::East => "b",
            Button::West => "x",
            Button::North => "y",
            Button::LeftTrigger => "lt",
            Button::RightTrigger => "rt",
            Button::LeftTrigger2 => "lb",
            Button::RightTrigger2 => "rb",
            Button::Select => "minus",
            Button::Start => "plus",
            Button::Mode => "home",
            Button::LeftThumb => "l3",
            Button::RightThumb => "r3",
            Button::DPadUp => return format!("{}.dpad.up", prefix),
            Button::DPadDown => return format!("{}.dpad.down", prefix),
            Button::DPadLeft => return format!("{}.dpad.left", prefix),
            Button::DPadRight => return format!("{}.dpad.right", prefix),
            Button::C => "c",
            Button::Z => "capture",
            _ => {
                warn!("Unknown gilrs button: {:?}", button);
                "unknown"
            }
        };

        format!("{}.btn.{}", prefix, name)
    }

    /// Map gilrs axis to standardized control ID (same as provider.rs)
    fn axis_to_id(axis: gilrs::Axis, prefix: &str) -> String {
        use gilrs::Axis;

        let name = match axis {
            Axis::LeftStickX => "lx",
            Axis::LeftStickY => "ly",
            Axis::RightStickX => "rx",
            Axis::RightStickY => "ry",
            Axis::LeftZ => "zl",
            Axis::RightZ => "zr",
            _ => {
                warn!("Unknown gilrs axis: {:?}", axis);
                "unknown"
            }
        };

        format!("{}.axis.{}", prefix, name)
    }

    /// Poll XInput events
    fn poll_xinput_events(&mut self, event_tx: &mpsc::UnboundedSender<GamepadEvent>) {
        // Check if XInput is available
        if self.xinput_handle.is_none() {
            return;
        }

        for user_index in 0..4u32 {
            let idx = user_index as usize;

            // Try to get state (borrow handle temporarily)
            let state = {
                let handle = self.xinput_handle.as_ref().unwrap();
                match poll_xinput_controller(handle, user_index) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        // Controller disconnected
                        if self.xinput_connected[idx] {
                            self.xinput_connected[idx] = false;
                            self.last_xinput_state[idx] = None;
                        }
                        continue;
                    }
                    Err(e) => {
                        warn!("XInput error for user {}: {:?}", user_index, e);
                        continue;
                    }
                }
            }; // handle borrow ends here

            // Check packet number for changes
            if let Some(last_state) = &self.last_xinput_state[idx] {
                if last_state.packet_number == state.raw.dwPacketNumber {
                    continue; // No changes
                }
            }

            // New connection or state changed (now we can mutably borrow self)
            self.handle_xinput_update(idx, state, event_tx);
        }
    }

    /// Handle XInput controller update
    fn handle_xinput_update(
        &mut self,
        user_index: usize,
        state: rusty_xinput::XInputState,
        event_tx: &mpsc::UnboundedSender<GamepadEvent>
    ) {
        let hybrid_id = HybridControllerId::from_xinput(user_index);

        // Find slot (may trigger connection event)
        let (prefix, analog_config) = if let Some(ref mut manager) = self.slot_manager {
            if let Some(slot) = manager.get_slot_by_id(hybrid_id) {
                (slot.control_id_prefix(), slot.analog_config.clone())
            } else {
                // Try to connect this XInput controller
                let name = format!("XInput Controller {}", user_index + 1);
                if manager.try_connect(hybrid_id, &name).is_some() {
                    self.xinput_connected[user_index] = true;
                    // Get slot info after connection
                    if let Some(slot) = manager.get_slot_by_id(hybrid_id) {
                        (slot.control_id_prefix(), slot.analog_config.clone())
                    } else {
                        return; // Shouldn't happen
                    }
                } else {
                    return; // No matching slot
                }
            }
        } else {
            // Legacy mode
            ("gamepad".to_string(), None)
        };

        // Generate button events
        let old_buttons = self.last_xinput_state[user_index].as_ref().map(|s| s.buttons);
        let new_buttons = state.raw.Gamepad.wButtons;
        let button_events = convert_xinput_buttons(old_buttons, new_buttons, &prefix);

        for event in button_events {
            debug!("XInput button event: {:?}", event);
            let _ = event_tx.send(event);
        }

        // Generate axis events
        let axis_events = convert_xinput_axes(
            self.last_xinput_state[user_index].as_ref(),
            &state,
            &prefix,
            analog_config,
            &mut self.axis_sequence
        );

        for event in axis_events {
            debug!("XInput axis event: {:?}", event);
            let _ = event_tx.send(event);
        }

        // Update cached state
        self.last_xinput_state[user_index] = Some(CachedXInputState::from(&state));
        self.xinput_connected[user_index] = true;
    }
}
