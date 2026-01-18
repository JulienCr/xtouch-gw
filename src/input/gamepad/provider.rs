//! GilRs gamepad provider with hot-plug support
//!
//! **Legacy provider** - This module is kept for reference. The production codebase uses
//! [`HybridGamepadProvider`](super::hybrid_provider::HybridGamepadProvider) which combines
//! XInput and gilrs backends with improved change detection and sequence numbering.

use super::axis::gilrs_axis_to_control_id;
use super::normalize::normalize_gilrs_stick;
use super::slot::SlotManager;
use super::stick_buffer::{StickBuffer, StickId};
use crate::config::AnalogConfig;
use anyhow::Result;
use gilrs::{Axis, Button, Event, EventType, Gilrs};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Standardized gamepad event
#[derive(Debug, Clone)]
pub enum GamepadEvent {
    /// Button press/release
    Button {
        control_id: String, // Full ID: "gamepad1.btn.a"
        pressed: bool,
    },
    /// Analog axis movement
    Axis {
        control_id: String, // Full ID: "gamepad1.axis.lx"
        value: f32,
        analog_config: Option<AnalogConfig>, // Per-slot config
        /// Monotonic sequence number for ordering (prevents race conditions under CPU load)
        sequence: u64,
    },
}

/// Callback type for gamepad events
pub type EventCallback = Arc<dyn Fn(GamepadEvent) + Send + Sync>;

/// GilRs-based gamepad provider with hot-plug support
pub struct GilrsProvider {
    event_listeners: Arc<RwLock<Vec<EventCallback>>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl GilrsProvider {
    /// Create and start a new gamepad provider
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
        // Initialize gilrs in this thread (not Send-safe)
        let mut gilrs = match Gilrs::new() {
            Ok(g) => {
                info!("GilRs initialized");
                g
            },
            Err(e) => {
                warn!("Failed to initialize GilRs: {:?}", e);
                return;
            },
        };

        // Create slot manager (or use legacy mode if empty)
        let mut slot_manager = if slot_configs.is_empty() {
            None
        } else {
            Some(SlotManager::new(slot_configs))
        };

        let mut last_reconnect_check = std::time::Instant::now();
        let reconnect_interval = Duration::from_secs(2);

        // Stick buffer for radial normalization (per gamepad, per stick)
        let mut stick_buffers: HashMap<(gilrs::GamepadId, StickId), StickBuffer> = HashMap::new();

        // Wait for gamepads to initialize (Windows Bluetooth controllers need time)
        info!("Scanning for gamepads...");
        info!("⏳ Waiting for gamepad enumeration (3 seconds)...");

        let scan_start = std::time::Instant::now();
        let scan_duration = Duration::from_secs(3);

        // Poll events during initial scan to trigger connection detection
        while scan_start.elapsed() < scan_duration {
            // Process events to allow gilrs to detect gamepads
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                if event == EventType::Connected {
                    debug!("Gamepad connected during initial scan: {:?}", id);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // Now scan for connected gamepads
        let all_gamepads: Vec<_> = gilrs
            .gamepads()
            .filter(|(_, gp)| gp.is_connected())
            .map(|(id, gp)| (id, gp.name().to_string()))
            .collect();

        if all_gamepads.is_empty() {
            warn!("⚠️  No gamepads detected at all");
        } else {
            info!("Found {} connected gamepad(s):", all_gamepads.len());
            for (id, name) in &all_gamepads {
                info!("  - {:?}: \"{}\"", id, name);
            }
        }

        // Initial gamepad slot assignment
        if let Some(ref mut manager) = slot_manager {
            for (id, gamepad) in gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
                let name = gamepad.name();
                use super::hybrid_id::HybridControllerId;
                let hybrid_id = HybridControllerId::from_gilrs(id);
                manager.try_connect(hybrid_id, name);
            }
        }

        loop {
            // Check for shutdown signal (non-blocking)
            match shutdown_rx.try_recv() {
                Ok(_) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    info!("Gamepad provider shutting down");
                    break;
                },
                Err(mpsc::error::TryRecvError::Empty) => {},
            }

            // Reconnection check (every 2 seconds)
            if last_reconnect_check.elapsed() >= reconnect_interval {
                last_reconnect_check = std::time::Instant::now();

                if let Some(ref mut manager) = slot_manager {
                    // Check for disconnections
                    manager.check_gilrs_disconnections(&gilrs);

                    // Try to reconnect empty slots
                    for (id, gamepad) in gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
                        let name = gamepad.name();
                        use super::hybrid_id::HybridControllerId;
                        let hybrid_id = HybridControllerId::from_gilrs(id);
                        manager.try_connect(hybrid_id, name);
                    }
                }
            }

            // Process gilrs events
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                use super::hybrid_id::HybridControllerId;

                // Find which slot this event belongs to
                let (slot_prefix, analog_config) = if let Some(ref manager) = slot_manager {
                    let hybrid_id = HybridControllerId::from_gilrs(id);
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

                // Handle disconnection - clean up stick buffer
                if event == EventType::Disconnected {
                    stick_buffers.retain(|(gp_id, _), _| *gp_id != id);
                }

                // Convert event with slot prefix and radial normalization
                let events =
                    Self::convert_event(id, event, &slot_prefix, analog_config, &mut stick_buffers);

                for gamepad_event in events {
                    debug!("Gamepad event: {:?}", gamepad_event);

                    if event_tx.send(gamepad_event).is_err() {
                        warn!("Event receiver dropped, shutting down gamepad loop");
                        return;
                    }
                }
            }

            // Sleep briefly to avoid busy-waiting
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    /// Convert GilRs event to standardized gamepad event(s) with radial normalization
    ///
    /// Returns Vec because radial normalization couples X/Y axes - updating one
    /// may require emitting events for both.
    fn convert_event(
        id: gilrs::GamepadId,
        event: EventType,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
        stick_buffers: &mut HashMap<(gilrs::GamepadId, StickId), StickBuffer>,
    ) -> Vec<GamepadEvent> {
        match event {
            EventType::ButtonPressed(button, _) | EventType::ButtonReleased(button, _) => {
                let pressed = matches!(event, EventType::ButtonPressed(_, _));
                match Self::button_to_id(button, prefix) {
                    Some(control_id) => vec![GamepadEvent::Button {
                        control_id,
                        pressed,
                    }],
                    None => vec![],
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
                    Self::process_stick_axis(
                        id,
                        axis,
                        value,
                        stick,
                        prefix,
                        analog_config,
                        stick_buffers,
                    )
                } else {
                    // Non-stick axis (triggers, etc.): pass through directly
                    vec![GamepadEvent::Axis {
                        control_id: gilrs_axis_to_control_id(axis, prefix),
                        value,
                        analog_config,
                        sequence: 0, // Legacy provider - sequence not used
                    }]
                }
            },
            EventType::Connected => {
                debug!("Gamepad connected event");
                vec![]
            },
            EventType::Disconnected => {
                debug!("Gamepad disconnected event");
                vec![]
            },
            _ => vec![],
        }
    }

    /// Process stick axis with radial normalization
    fn process_stick_axis(
        id: gilrs::GamepadId,
        axis: Axis,
        value: f32,
        stick: StickId,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
        stick_buffers: &mut HashMap<(gilrs::GamepadId, StickId), StickBuffer>,
    ) -> Vec<GamepadEvent> {
        let buffer_key = (id, stick);

        // Get or create stick buffer
        let buffer = stick_buffers
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

        // Invert Y to match HID convention
        let final_y = -norm_y;

        // Determine axis IDs
        let (x_axis, y_axis) = match stick {
            StickId::Left => (Axis::LeftStickX, Axis::LeftStickY),
            StickId::Right => (Axis::RightStickX, Axis::RightStickY),
        };

        // NOTE: This legacy provider emits both axes unconditionally, even if only
        // one axis changed. This can cause redundant event processing. The modern
        // HybridGamepadProvider uses `emit_axis_with_zero_detection` to filter out
        // redundant events when an axis hasn't meaningfully changed.
        vec![
            GamepadEvent::Axis {
                control_id: gilrs_axis_to_control_id(x_axis, prefix),
                value: norm_x,
                analog_config: analog_config.clone(),
                sequence: 0, // Legacy provider - sequence not used
            },
            GamepadEvent::Axis {
                control_id: gilrs_axis_to_control_id(y_axis, prefix),
                value: final_y,
                analog_config,
                sequence: 0, // Legacy provider - sequence not used
            },
        ]
    }

    /// Map GilRs button to standardized control ID
    ///
    /// Uses the shared `buttons` module for consistent Nintendo-layout mapping
    /// across all gilrs-based code paths.
    fn button_to_id(button: Button, prefix: &str) -> Option<String> {
        super::buttons::gilrs_button_to_control_id(button, prefix)
    }

    /// Shutdown the provider
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
            info!("Gamepad provider shutdown requested");
        }
        Ok(())
    }
}

impl Drop for GilrsProvider {
    fn drop(&mut self) {
        // Attempt to send shutdown signal if not already sent
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
    }
}
