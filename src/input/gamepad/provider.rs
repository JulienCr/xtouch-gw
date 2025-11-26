//! GilRs gamepad provider with hot-plug support

use anyhow::Result;
use gilrs::{Gilrs, Gamepad, Event, EventType, Button, Axis};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{info, warn, debug, error};
use std::time::Duration;

/// Standardized gamepad event
#[derive(Debug, Clone)]
pub enum GamepadEvent {
    /// Button press/release
    Button { id: String, pressed: bool },
    /// Analog axis movement
    Axis { id: String, value: f32 },
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
    /// * `product_match` - Optional substring to match against gamepad name
    ///
    /// # Returns
    /// Running provider instance
    pub async fn start(product_match: Option<String>) -> Result<Self> {
        let event_listeners = Arc::new(RwLock::new(Vec::<EventCallback>::new()));
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

        // Create a channel for sending events from blocking thread to async world
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<GamepadEvent>();

        // Spawn blocking event loop in a dedicated thread
        std::thread::spawn(move || {
            Self::event_loop_blocking(product_match, event_tx, shutdown_rx);
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
        product_match: Option<String>,
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

        let mut connected_gamepad_id = None;
        let mut last_reconnect_check = std::time::Instant::now();
        let reconnect_interval = Duration::from_secs(2);

        // Log all detected gamepads for debugging
        info!("Scanning for gamepads...");
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

        // Check for initial gamepad
        if let Some((id, gp)) = Self::find_gamepad(&gilrs, &product_match) {
            info!("✅ Gamepad connected: {} (ID: {:?})", gp.name(), id);
            connected_gamepad_id = Some(id);
        } else {
            if let Some(pattern) = &product_match {
                warn!("⚠️  No gamepad matching pattern \"{}\" found", pattern);
                warn!("    Available gamepad names: {:?}",
                    all_gamepads.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>());
            } else {
                warn!("⚠️  No matching gamepad found at startup");
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

                // Check if current gamepad is still connected
                if let Some(gp_id) = connected_gamepad_id {
                    if gilrs.connected_gamepad(gp_id).is_none() {
                        warn!("🔌 Gamepad disconnected");
                        connected_gamepad_id = None;
                    }
                }

                // Try to find/reconnect to gamepad
                if connected_gamepad_id.is_none() {
                    if let Some((id, gp)) = Self::find_gamepad(&gilrs, &product_match) {
                        info!("✅ Gamepad connected: {} (ID: {:?})", gp.name(), id);
                        connected_gamepad_id = Some(id);
                    }
                }
            }

            // Process all available events
            while let Some(Event { id, event, .. }) = gilrs.next_event() {
                // Only process events from our connected gamepad
                if Some(id) != connected_gamepad_id {
                    continue;
                }

                // Convert gilrs event to our standardized event
                if let Some(gamepad_event) = Self::convert_event(event) {
                    debug!("Gamepad event: {:?}", gamepad_event);

                    // Send event to async handler via channel
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

    /// Find a matching gamepad
    fn find_gamepad<'a>(gilrs: &'a Gilrs, product_match: &Option<String>) -> Option<(gilrs::GamepadId, Gamepad<'a>)> {
        let mut first_gamepad = None;

        for (id, gamepad) in gilrs.gamepads() {
            if !gamepad.is_connected() {
                continue;
            }

            // Store first gamepad as fallback
            if first_gamepad.is_none() {
                first_gamepad = Some((id, gamepad.clone()));
            }

            let name = gamepad.name();

            // If product_match is specified, check substring match (case-insensitive)
            if let Some(pattern) = product_match {
                if name.to_lowercase().contains(&pattern.to_lowercase()) {
                    debug!("Gamepad name \"{}\" matches pattern \"{}\"", name, pattern);
                    return Some((id, gamepad));
                } else {
                    debug!("Gamepad name \"{}\" does NOT match pattern \"{}\"", name, pattern);
                }
            } else {
                // No filter - return first connected gamepad
                return Some((id, gamepad));
            }
        }

        // If no match found but product_match was specified, return None
        // (don't use first_gamepad as fallback when user specified a pattern)
        None
    }

    /// Convert GilRs event to standardized gamepad event
    fn convert_event(event: EventType) -> Option<GamepadEvent> {
        match event {
            EventType::ButtonPressed(button, _) => {
                Some(GamepadEvent::Button {
                    id: Self::button_to_id(button),
                    pressed: true,
                })
            }
            EventType::ButtonReleased(button, _) => {
                Some(GamepadEvent::Button {
                    id: Self::button_to_id(button),
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
                    id: Self::axis_to_id(axis),
                    value: normalized_value,
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
    fn button_to_id(button: Button) -> String {
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
            Button::DPadUp => return "gamepad.dpad.up".to_string(),
            Button::DPadDown => return "gamepad.dpad.down".to_string(),
            Button::DPadLeft => return "gamepad.dpad.left".to_string(),
            Button::DPadRight => return "gamepad.dpad.right".to_string(),
            Button::C => "c",
            Button::Z => "capture",
            _ => {
                warn!("Unknown button: {:?}", button);
                "unknown"
            }
        };

        format!("gamepad.btn.{}", name)
    }

    /// Map GilRs axis to standardized control ID
    fn axis_to_id(axis: Axis) -> String {
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

        format!("gamepad.axis.{}", name)
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
