//! GilRs gamepad provider with hot-plug support

use anyhow::Result;
use gilrs::{Gilrs, Event, EventType, Button, Axis};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{info, warn, debug};
use std::time::Duration;
use crate::config::AnalogConfig;
use super::slot::SlotManager;

/// Standardized gamepad event
#[derive(Debug, Clone)]
pub enum GamepadEvent {
    /// Button press/release
    Button {
        control_id: String,  // Full ID: "gamepad1.btn.a"
        pressed: bool
    },
    /// Analog axis movement
    Axis {
        control_id: String,  // Full ID: "gamepad1.axis.lx"
        value: f32,
        analog_config: Option<AnalogConfig>,  // Per-slot config
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
            }
            Err(e) => {
                warn!("Failed to initialize GilRs: {:?}", e);
                return;
            }
        };

        // Create slot manager (or use legacy mode if empty)
        let mut slot_manager = if slot_configs.is_empty() {
            None
        } else {
            Some(SlotManager::new(slot_configs))
        };

        let mut last_reconnect_check = std::time::Instant::now();
        let reconnect_interval = Duration::from_secs(2);

        // Wait for gamepads to initialize (Windows Bluetooth controllers need time)
        info!("Scanning for gamepads...");
        info!("⏳ Waiting for gamepad enumeration (3 seconds)...");

        let scan_start = std::time::Instant::now();
        let scan_duration = Duration::from_secs(3);

        // Poll events during initial scan to trigger connection detection
        while scan_start.elapsed() < scan_duration {
            // Process events to allow gilrs to detect gamepads
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                match event {
                    EventType::Connected => {
                        debug!("Gamepad connected during initial scan: {:?}", id);
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // Now scan for connected gamepads
        let all_gamepads: Vec<_> = gilrs.gamepads()
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
                manager.try_connect(id, name);
            }
        }

        loop {
            // Check for shutdown signal (non-blocking)
            match shutdown_rx.try_recv() {
                Ok(_) | Err(mpsc::error::TryRecvError::Disconnected) => {
                    info!("Gamepad provider shutting down");
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }

            // Reconnection check (every 2 seconds)
            if last_reconnect_check.elapsed() >= reconnect_interval {
                last_reconnect_check = std::time::Instant::now();

                if let Some(ref mut manager) = slot_manager {
                    // Check for disconnections
                    manager.check_disconnections(&gilrs);

                    // Try to reconnect empty slots
                    for (id, gamepad) in gilrs.gamepads().filter(|(_, gp)| gp.is_connected()) {
                        let name = gamepad.name();
                        manager.try_connect(id, name);
                    }
                }
            }

            // Process gilrs events
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                // Find which slot this event belongs to
                let (slot_prefix, analog_config) = if let Some(ref manager) = slot_manager {
                    if let Some(slot) = manager.get_slot_by_id(id) {
                        (slot.control_id_prefix(), slot.analog_config.clone())
                    } else {
                        // Event from unregistered gamepad, ignore
                        continue;
                    }
                } else {
                    // Legacy mode: no slot manager, use "gamepad" prefix
                    ("gamepad".to_string(), None)
                };

                // Convert event with slot prefix
                if let Some(gamepad_event) = Self::convert_event(event, &slot_prefix, analog_config) {
                    debug!("Gamepad event: {:?}", gamepad_event);

                    if event_tx.send(gamepad_event).is_err() {
                        warn!("Event receiver dropped, shutting down gamepad loop");
                        break;
                    }
                }
            }

            // Sleep briefly to avoid busy-waiting
            std::thread::sleep(Duration::from_millis(4));
        }
    }


    /// Convert GilRs event to standardized gamepad event
    fn convert_event(
        event: EventType,
        prefix: &str,
        analog_config: Option<AnalogConfig>
    ) -> Option<GamepadEvent> {
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
                // Normalize Y-axis convention to match HID behavior:
                // - gilrs: up=positive, down=negative
                // - HID: up=negative, down=positive
                // Negate Y axes to match HID convention for consistency
                let normalized_value = match axis {
                    Axis::LeftStickY | Axis::RightStickY => -value,
                    _ => value,
                };

                Some(GamepadEvent::Axis {
                    control_id: Self::axis_to_id(axis, prefix),
                    value: normalized_value,
                    analog_config,
                })
            }
            EventType::Connected => {
                debug!("Gamepad connected event");
                None
            }
            EventType::Disconnected => {
                debug!("Gamepad disconnected event");
                None
            }
            _ => None,
        }
    }

    /// Map GilRs button to standardized control ID
    fn button_to_id(button: Button, prefix: &str) -> String {
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
                warn!("Unknown button: {:?}", button);
                "unknown"
            }
        };

        format!("{}.btn.{}", prefix, name)
    }

    /// Map GilRs axis to standardized control ID
    fn axis_to_id(axis: Axis, prefix: &str) -> String {
        let name = match axis {
            Axis::LeftStickX => "lx",
            Axis::LeftStickY => "ly",
            Axis::RightStickX => "rx",
            Axis::RightStickY => "ry",
            Axis::LeftZ => "zl",
            Axis::RightZ => "zr",
            _ => {
                warn!("Unknown axis: {:?}", axis);
                "unknown"
            }
        };

        format!("{}.axis.{}", prefix, name)
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
