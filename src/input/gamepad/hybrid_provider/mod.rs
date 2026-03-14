//! Hybrid gamepad provider combining XInput and gilrs (WGI) backends
//!
//! This provider polls both XInput (for Xbox controllers) and gilrs with WGI backend
//! (for non-XInput controllers like FaceOff) simultaneously, enabling support for
//! multiple controller types in a headless tray application.

mod gilrs_events;
mod scan;
mod xinput;

use anyhow::Result;
use gilrs::Gilrs;
use parking_lot::Mutex;
use rusty_xinput::XInputHandle;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::provider::GamepadEvent;
use super::slot::SlotManager;
use super::stick_buffer::{StickBuffer, StickId};
use super::xinput_convert::CachedXInputState;
use crate::config::AnalogConfig;

/// Callback type for gamepad events
pub type EventCallback = Arc<dyn Fn(GamepadEvent) + Send + Sync>;

/// Hybrid gamepad provider with XInput and gilrs (WGI) support
pub struct HybridGamepadProvider {
    event_listeners: Arc<RwLock<Vec<EventCallback>>>,
    shutdown_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl HybridGamepadProvider {
    /// Create and start a new hybrid gamepad provider
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
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
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
        let mut state = match HybridProviderState::new(slot_configs) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to initialize hybrid gamepad provider: {}", e);
                return;
            },
        };

        // Initial scan (3 seconds for Bluetooth controllers)
        state.initial_scan();

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
    pub async fn shutdown(&self) -> Result<()> {
        let tx = self.shutdown_tx.lock().take();
        if let Some(tx) = tx {
            if tx.send(()).await.is_err() {
                warn!("Gamepad shutdown signal failed (provider thread may have already exited)");
            }
            debug!("Hybrid gamepad provider shutdown requested");
        }
        Ok(())
    }
}

impl Drop for HybridGamepadProvider {
    fn drop(&mut self) {
        // Attempt to send shutdown signal if not already sent
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.try_send(());
        }
    }
}

/// Internal state for the blocking event loop
pub(super) struct HybridProviderState {
    pub(super) gilrs: Gilrs,
    pub(super) xinput_handle: Option<XInputHandle>,
    pub(super) xinput_available: bool,
    pub(super) slot_manager: Option<SlotManager>,
    pub(super) last_xinput_state: [Option<CachedXInputState>; 4],
    pub(super) xinput_connected: [bool; 4],
    pub(super) last_reconnect_check: Instant,
    /// Track last gilrs axis values to detect return-to-zero
    pub(super) last_gilrs_axis_values: HashMap<(gilrs::GamepadId, gilrs::Axis), f32>,
    /// Monotonic sequence counter for axis events
    pub(super) axis_sequence: u64,
    /// Buffer gilrs stick X/Y pairs for radial normalization
    pub(super) gilrs_stick_buffer: HashMap<(gilrs::GamepadId, StickId), StickBuffer>,
}

impl HybridProviderState {
    /// Initialize the hybrid provider state
    pub(super) fn new(slot_configs: Vec<(String, Option<AnalogConfig>)>) -> Result<Self> {
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

    /// Check if a gamepad name indicates an Xbox controller
    pub(super) fn is_xbox_name(name: &str) -> bool {
        let name_lower = name.to_lowercase();
        name_lower.contains("xbox")
            || name_lower.contains("xinput")
            || name_lower.contains("x-box")
            || name_lower.contains("microsoft")
    }

    /// Check if it's time for a reconnection check
    pub(super) fn should_check_reconnection(&self) -> bool {
        self.last_reconnect_check.elapsed() >= Duration::from_secs(2)
    }
}
