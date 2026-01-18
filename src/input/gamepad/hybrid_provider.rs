//! Hybrid gamepad provider combining XInput and gilrs (WGI) backends
//!
//! This provider polls both XInput (for Xbox controllers) and gilrs with WGI backend
//! (for non-XInput controllers like FaceOff) simultaneously, enabling support for
//! multiple controller types in a headless tray application.

use anyhow::Result;
use gilrs::{Event, EventType, Gilrs};
use rusty_xinput::XInputHandle;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};

use super::hybrid_id::HybridControllerId;
use super::normalize::normalize_gilrs_stick;
use super::provider::GamepadEvent;
use super::slot::SlotManager;
use super::xinput_convert::{
    convert_xinput_axes, convert_xinput_buttons, poll_xinput_controller, CachedXInputState,
};
use crate::config::AnalogConfig;

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
            },
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
                },
                Err(mpsc::error::TryRecvError::Empty) => {},
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

/// Stick identifier for buffering X/Y pairs
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
enum StickId {
    Left,
    Right,
}

/// Buffered stick state for radial normalization
#[derive(Debug, Clone, Default)]
struct StickBuffer {
    x: f32,
    y: f32,
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
    /// Buffer gilrs stick X/Y pairs for radial normalization
    /// Key: (gamepad_id, stick_id), Value: (raw_x, raw_y) before normalization
    gilrs_stick_buffer: HashMap<(gilrs::GamepadId, StickId), StickBuffer>,
}

impl HybridProviderState {
    /// Initialize the hybrid provider state
    fn new(slot_configs: Vec<(String, Option<AnalogConfig>)>) -> Result<Self> {
        // Initialize gilrs (always required)
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                debug!("gilrs initialized (WGI backend enabled)");
                g
            },
            Err(e) => {
                warn!("Failed to initialize gilrs: {:?}", e);
                return Err(anyhow::anyhow!("gilrs initialization failed: {}", e));
            },
        };

        // Try to initialize XInput (optional, graceful fallback)
        let (xinput_handle, xinput_available) = match XInputHandle::load_default() {
            Ok(handle) => {
                debug!("XInput initialized successfully");
                (Some(handle), true)
            },
            Err(e) => {
                warn!(
                    "XInput library not available (falling back to WGI-only): {:?}",
                    e
                );
                (None, false)
            },
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
            gilrs_stick_buffer: HashMap::new(),
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
                if event == EventType::Connected {
                    trace!("gilrs gamepad connected during initial scan: {:?}", id);
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
        name_lower.contains("xbox")
            || name_lower.contains("xinput")
            || name_lower.contains("x-box")
            || name_lower.contains("microsoft")
    }

    /// Report connected gamepads after initial scan
    fn report_connected_gamepads(&mut self) {
        // Count gilrs gamepads
        let gilrs_count = self
            .gilrs
            .gamepads()
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
            debug!(
                "Found {} gilrs gamepad(s) and {} XInput gamepad(s):",
                gilrs_count, xinput_count
            );

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
            // Note: convert_gilrs_event returns Vec because radial normalization couples X/Y
            for gamepad_event in self.convert_gilrs_event(id, event, &prefix, analog_config) {
                debug!("gilrs event: {:?}", gamepad_event);

                if event_tx.send(gamepad_event).is_err() {
                    warn!("Event receiver dropped, shutting down gamepad loop");
                    return;
                }
            }
        }
    }

    /// Convert gilrs event to GamepadEvent(s) with radial normalization for sticks
    ///
    /// Returns Vec because radial normalization couples X/Y axes - updating one
    /// may require emitting events for both.
    fn convert_gilrs_event(
        &mut self,
        id: gilrs::GamepadId,
        event: EventType,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Vec<GamepadEvent> {
        use gilrs::Axis;

        match event {
            EventType::ButtonPressed(button, _) => {
                if let Some(control_id) = Self::button_to_id(button, prefix) {
                    vec![GamepadEvent::Button {
                        control_id,
                        pressed: true,
                    }]
                } else {
                    vec![]
                }
            },
            EventType::ButtonReleased(button, _) => {
                if let Some(control_id) = Self::button_to_id(button, prefix) {
                    vec![GamepadEvent::Button {
                        control_id,
                        pressed: false,
                    }]
                } else {
                    vec![]
                }
            },
            EventType::AxisChanged(axis, value, _) => {
                // Determine if this is a stick axis that needs radial normalization
                let stick_id = match axis {
                    Axis::LeftStickX | Axis::LeftStickY => Some(StickId::Left),
                    Axis::RightStickX | Axis::RightStickY => Some(StickId::Right),
                    _ => None,
                };

                if let Some(stick) = stick_id {
                    // Stick axis: use radial normalization
                    self.process_stick_axis(id, axis, value, stick, prefix, analog_config)
                } else {
                    // Non-stick axis (triggers, etc.): pass through directly
                    self.process_non_stick_axis(id, axis, value, prefix, analog_config)
                }
            },
            EventType::Connected => {
                trace!("gilrs gamepad connected event");
                vec![]
            },
            EventType::Disconnected => {
                debug!("gilrs gamepad disconnected event");
                // Clean up axis tracking for disconnected gamepad
                self.last_gilrs_axis_values
                    .retain(|(gp_id, _), _| *gp_id != id);
                // Clean up stick buffer for disconnected gamepad
                self.gilrs_stick_buffer.retain(|(gp_id, _), _| *gp_id != id);
                vec![]
            },
            _ => vec![],
        }
    }

    /// Process stick axis with radial normalization
    ///
    /// Buffers X/Y values and applies square_to_circle to ensure diagonal
    /// movements can reach magnitude 1.0 (fixes the 0.707 issue).
    fn process_stick_axis(
        &mut self,
        id: gilrs::GamepadId,
        axis: gilrs::Axis,
        value: f32,
        stick: StickId,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Vec<GamepadEvent> {
        use gilrs::Axis;

        let buffer_key = (id, stick);

        // Get or create stick buffer
        let buffer = self
            .gilrs_stick_buffer
            .entry(buffer_key)
            .or_insert_with(StickBuffer::default);

        // Update the appropriate axis in the buffer (raw value, before Y inversion)
        match axis {
            Axis::LeftStickX | Axis::RightStickX => buffer.x = value,
            Axis::LeftStickY | Axis::RightStickY => buffer.y = value,
            _ => unreachable!(),
        }

        // Apply radial normalization (configurable via GILRS_USE_RADIAL_CLAMP_ONLY flag)
        let (norm_x, norm_y) = normalize_gilrs_stick(buffer.x, buffer.y);

        // Invert Y to match HID convention (up=negative in gilrs, we want up=negative output)
        let final_y = -norm_y;

        // Determine axis IDs
        let (x_axis, y_axis) = match stick {
            StickId::Left => (Axis::LeftStickX, Axis::LeftStickY),
            StickId::Right => (Axis::RightStickX, Axis::RightStickY),
        };

        // Check for zero-crossing and emit events
        let mut events = Vec::new();
        const ZERO_THRESHOLD: f32 = 0.05;

        // Process X axis
        let x_key = (id, x_axis);
        let last_x = self.last_gilrs_axis_values.get(&x_key).copied();
        let x_near_zero = norm_x.abs() < ZERO_THRESHOLD;
        let x_was_nonzero = last_x.is_some_and(|v| v.abs() >= ZERO_THRESHOLD);

        if x_near_zero {
            if x_was_nonzero {
                self.last_gilrs_axis_values.remove(&x_key);
                self.axis_sequence += 1;
                events.push(GamepadEvent::Axis {
                    control_id: Self::axis_to_id(x_axis, prefix),
                    value: 0.0,
                    analog_config: analog_config.clone(),
                    sequence: self.axis_sequence,
                });
            }
        } else {
            let should_emit = last_x.is_none() || (norm_x - last_x.unwrap()).abs() > 0.001;
            if should_emit {
                self.last_gilrs_axis_values.insert(x_key, norm_x);
                self.axis_sequence += 1;
                events.push(GamepadEvent::Axis {
                    control_id: Self::axis_to_id(x_axis, prefix),
                    value: norm_x,
                    analog_config: analog_config.clone(),
                    sequence: self.axis_sequence,
                });
            }
        }

        // Process Y axis
        let y_key = (id, y_axis);
        let last_y = self.last_gilrs_axis_values.get(&y_key).copied();
        let y_near_zero = final_y.abs() < ZERO_THRESHOLD;
        let y_was_nonzero = last_y.is_some_and(|v| v.abs() >= ZERO_THRESHOLD);

        if y_near_zero {
            if y_was_nonzero {
                self.last_gilrs_axis_values.remove(&y_key);
                self.axis_sequence += 1;
                events.push(GamepadEvent::Axis {
                    control_id: Self::axis_to_id(y_axis, prefix),
                    value: 0.0,
                    analog_config: analog_config.clone(),
                    sequence: self.axis_sequence,
                });
            }
        } else {
            let should_emit = last_y.is_none() || (final_y - last_y.unwrap()).abs() > 0.001;
            if should_emit {
                self.last_gilrs_axis_values.insert(y_key, final_y);
                self.axis_sequence += 1;
                events.push(GamepadEvent::Axis {
                    control_id: Self::axis_to_id(y_axis, prefix),
                    value: final_y,
                    analog_config,
                    sequence: self.axis_sequence,
                });
            }
        }

        events
    }

    /// Process non-stick axis (triggers, etc.) without radial normalization
    fn process_non_stick_axis(
        &mut self,
        id: gilrs::GamepadId,
        axis: gilrs::Axis,
        value: f32,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Vec<GamepadEvent> {
        let key = (id, axis);
        let last_value = self.last_gilrs_axis_values.get(&key).copied();

        const ZERO_THRESHOLD: f32 = 0.05;
        let is_near_zero = value.abs() < ZERO_THRESHOLD;
        let was_non_zero = last_value.is_some_and(|v| v.abs() >= ZERO_THRESHOLD);

        if is_near_zero {
            if was_non_zero {
                self.last_gilrs_axis_values.remove(&key);
                self.axis_sequence += 1;
                return vec![GamepadEvent::Axis {
                    control_id: Self::axis_to_id(axis, prefix),
                    value: 0.0,
                    analog_config,
                    sequence: self.axis_sequence,
                }];
            }
            vec![]
        } else {
            self.last_gilrs_axis_values.insert(key, value);
            self.axis_sequence += 1;
            vec![GamepadEvent::Axis {
                control_id: Self::axis_to_id(axis, prefix),
                value,
                analog_config,
                sequence: self.axis_sequence,
            }]
        }
    }

    /// Map gilrs button to standardized control ID
    ///
    /// Uses the shared `buttons` module for consistent Nintendo-layout mapping
    /// across all gilrs-based code paths.
    fn button_to_id(button: gilrs::Button, prefix: &str) -> Option<String> {
        super::buttons::gilrs_button_to_control_id(button, prefix)
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
            },
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
                    },
                    Err(e) => {
                        warn!("XInput error for user {}: {:?}", user_index, e);
                        continue;
                    },
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
        event_tx: &mpsc::UnboundedSender<GamepadEvent>,
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
        let old_buttons = self.last_xinput_state[user_index]
            .as_ref()
            .map(|s| s.buttons);
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
            &mut self.axis_sequence,
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
