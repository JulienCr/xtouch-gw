//! OBS Driver core struct and initialization
//!
//! Defines the ObsDriver struct with all its state and provides constructors.

use obws::Client as ObsClient;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::debug;

use super::analog::AnalogRate;
use super::camera::CameraControlState;
use super::encoder::EncoderSpeedTracker;
use super::transform::ObsItemState;

/// OBS Studio WebSocket driver
pub struct ObsDriver {
    pub(super) name: String,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) password: Option<String>,

    // OBS client (wrapped for interior mutability)
    pub(super) client: Arc<RwLock<Option<ObsClient>>>,

    // State tracking (using parking_lot for sync access)
    pub(super) studio_mode: Arc<parking_lot::RwLock<bool>>,
    pub(super) program_scene: Arc<parking_lot::RwLock<String>>,
    pub(super) preview_scene: Arc<parking_lot::RwLock<String>>,

    // Transform cache (scene::source -> state)
    pub(super) transform_cache: Arc<parking_lot::RwLock<HashMap<String, ObsItemState>>>,
    pub(super) item_id_cache: Arc<parking_lot::RwLock<HashMap<String, i64>>>,

    // Indicator emission (using parking_lot for sync access)
    pub(super) indicator_emitters: Arc<parking_lot::RwLock<Vec<super::IndicatorCallback>>>,
    pub(super) last_selected_sent: Arc<parking_lot::RwLock<Option<String>>>,
    /// Last value emitted per signal, keyed by `&'static` signal name (e.g.
    /// `signals::CURRENT_PROGRAM_SCENE`). Used by `emit_and_debounce` as a
    /// change-detection guard so identical scene/studio events don't allocate
    /// or re-fan-out downstream.
    pub(super) last_emitted: Arc<parking_lot::RwLock<HashMap<&'static str, serde_json::Value>>>,

    // Connection status tracking
    pub(super) status_callbacks: Arc<parking_lot::RwLock<Vec<crate::tray::StatusCallback>>>,
    pub(super) current_status: Arc<parking_lot::RwLock<crate::tray::ConnectionStatus>>,

    // Activity tracking
    pub(super) activity_tracker:
        Arc<parking_lot::RwLock<Option<Arc<crate::tray::ActivityTracker>>>>,

    // Reconnection state
    pub(super) reconnect_count: Arc<Mutex<usize>>,
    pub(super) shutdown_flag: Arc<Mutex<bool>>,
    /// Single-flight guard for the reconnect task. Prevents action-fired
    /// reconnects from racing with the listener-fired reconnect task.
    /// True while a `schedule_reconnect` loop is running.
    pub(super) reconnecting: Arc<AtomicBool>,

    // Gamepad analog configuration
    pub(super) analog_pan_gain: Arc<parking_lot::RwLock<f64>>,
    pub(super) analog_zoom_gain: Arc<parking_lot::RwLock<f64>>,
    pub(super) analog_deadzone: Arc<parking_lot::RwLock<f64>>,
    pub(super) analog_gamma: Arc<parking_lot::RwLock<f64>>,

    // Analog motion state (velocity-based for gamepad)
    pub(super) analog_rates: Arc<parking_lot::RwLock<HashMap<String, AnalogRate>>>,
    pub(super) analog_timer_active: Arc<Mutex<bool>>,
    /// Generation counter for the 60 Hz analog timer task. Bumped when a new
    /// timer is spawned; a lingering older task whose captured generation no
    /// longer matches exits, preventing two timers from a spawn racing the
    /// old task's self-stop.
    pub(super) analog_timer_generation: Arc<AtomicU64>,
    pub(super) last_analog_tick: Arc<Mutex<Instant>>,

    // Error tracking to prevent infinite retry loops
    pub(super) analog_error_count: Arc<parking_lot::RwLock<HashMap<String, usize>>>,

    // Encoder acceleration
    pub(super) encoder_tracker: Arc<Mutex<EncoderSpeedTracker>>,

    // Camera control state (for split views)
    pub(super) camera_control_state: Arc<parking_lot::RwLock<CameraControlState>>,
    pub(super) camera_control_config:
        Arc<parking_lot::RwLock<Option<crate::config::CameraControlConfig>>>,

    // Optional live event broadcaster (editor `/api/live` WS).
    // Best-effort: emitters check the option and ignore errors.
    pub(super) live_tx: Arc<parking_lot::RwLock<Option<crate::event_bus::LiveEventTx>>>,
}

impl ObsDriver {
    /// Create a new OBS driver
    pub fn new(host: String, port: u16, password: Option<String>) -> Self {
        Self {
            name: "obs".to_string(),
            host,
            port,
            password,
            client: Arc::new(RwLock::new(None)),
            studio_mode: Arc::new(parking_lot::RwLock::new(false)),
            program_scene: Arc::new(parking_lot::RwLock::new(String::new())),
            preview_scene: Arc::new(parking_lot::RwLock::new(String::new())),
            transform_cache: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            item_id_cache: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            indicator_emitters: Arc::new(parking_lot::RwLock::new(Vec::new())),
            last_selected_sent: Arc::new(parking_lot::RwLock::new(None)),
            last_emitted: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            status_callbacks: Arc::new(parking_lot::RwLock::new(Vec::new())),
            current_status: Arc::new(parking_lot::RwLock::new(
                crate::tray::ConnectionStatus::Disconnected,
            )),
            activity_tracker: Arc::new(parking_lot::RwLock::new(None)),
            reconnect_count: Arc::new(Mutex::new(0)),
            shutdown_flag: Arc::new(Mutex::new(false)),
            reconnecting: Arc::new(AtomicBool::new(false)),
            // Analog config (defaults matching config file)
            analog_pan_gain: Arc::new(parking_lot::RwLock::new(15.0)),
            analog_zoom_gain: Arc::new(parking_lot::RwLock::new(3.0)),
            analog_deadzone: Arc::new(parking_lot::RwLock::new(0.02)),
            analog_gamma: Arc::new(parking_lot::RwLock::new(1.5)),
            // Analog motion state
            analog_rates: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            analog_timer_active: Arc::new(Mutex::new(false)),
            analog_timer_generation: Arc::new(AtomicU64::new(0)),
            last_analog_tick: Arc::new(Mutex::new(Instant::now())),
            // Error tracking
            analog_error_count: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            // Encoder acceleration
            encoder_tracker: Arc::new(Mutex::new(EncoderSpeedTracker::new())),
            // Camera control state
            camera_control_state: Arc::new(parking_lot::RwLock::new(CameraControlState::default())),
            camera_control_config: Arc::new(parking_lot::RwLock::new(None)),
            live_tx: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Inject the live event bus sender (editor `/api/live` WS).
    pub fn set_live_tx(&self, tx: crate::event_bus::LiveEventTx) {
        *self.live_tx.write() = Some(tx);
    }

    /// Create from config
    pub fn from_config(config: &crate::config::ObsConfig) -> Self {
        let driver = Self::new(config.host.clone(), config.port, config.password.clone());

        // Load camera control config if present
        if let Some(camera_control) = &config.camera_control {
            *driver.camera_control_config.write() = Some(camera_control.clone());

            // Initialize last_camera to first camera if available
            if let Some(first_camera) = camera_control.cameras.first() {
                driver.camera_control_state.write().last_camera = first_camera.id.clone();
            }
        }

        driver
    }

    /// Load analog config from gamepad settings
    #[allow(dead_code)] // reserved for hot-reload of gamepad analog tuning (currently set at init)
    pub fn load_analog_config(&self, gamepad_config: Option<&crate::config::GamepadConfig>) {
        if let Some(gamepad) = gamepad_config {
            if let Some(analog) = &gamepad.analog {
                *self.analog_pan_gain.write() = analog.pan_gain as f64;
                *self.analog_zoom_gain.write() = analog.zoom_gain as f64;
                *self.analog_deadzone.write() = analog.deadzone as f64;
                *self.analog_gamma.write() = analog.gamma as f64;
                debug!(
                    "OBS: analog config loaded (pan_gain={}, zoom_gain={}, deadzone={}, gamma={})",
                    analog.pan_gain, analog.zoom_gain, analog.deadzone, analog.gamma
                );
            }
        }
    }

    /// Check if OBS is currently in studio mode
    pub fn is_studio_mode(&self) -> bool {
        *self.studio_mode.read()
    }

    /// Clone all Arc fields for spawning background tasks (timer, reconnect, etc.)
    ///
    /// All fields are Arc-wrapped, so this creates a cheap clone that shares
    /// the same underlying data with the original instance.
    pub(super) fn clone_for_task(&self) -> Self {
        Self {
            name: self.name.clone(),
            host: self.host.clone(),
            port: self.port,
            password: self.password.clone(),
            client: Arc::clone(&self.client),
            studio_mode: Arc::clone(&self.studio_mode),
            program_scene: Arc::clone(&self.program_scene),
            preview_scene: Arc::clone(&self.preview_scene),
            transform_cache: Arc::clone(&self.transform_cache),
            item_id_cache: Arc::clone(&self.item_id_cache),
            indicator_emitters: Arc::clone(&self.indicator_emitters),
            last_selected_sent: Arc::clone(&self.last_selected_sent),
            last_emitted: Arc::clone(&self.last_emitted),
            status_callbacks: Arc::clone(&self.status_callbacks),
            current_status: Arc::clone(&self.current_status),
            activity_tracker: Arc::clone(&self.activity_tracker),
            reconnect_count: Arc::clone(&self.reconnect_count),
            shutdown_flag: Arc::clone(&self.shutdown_flag),
            reconnecting: Arc::clone(&self.reconnecting),
            analog_pan_gain: Arc::clone(&self.analog_pan_gain),
            analog_zoom_gain: Arc::clone(&self.analog_zoom_gain),
            analog_deadzone: Arc::clone(&self.analog_deadzone),
            analog_gamma: Arc::clone(&self.analog_gamma),
            analog_rates: Arc::clone(&self.analog_rates),
            analog_timer_active: Arc::clone(&self.analog_timer_active),
            analog_timer_generation: Arc::clone(&self.analog_timer_generation),
            last_analog_tick: Arc::clone(&self.last_analog_tick),
            analog_error_count: Arc::clone(&self.analog_error_count),
            encoder_tracker: Arc::clone(&self.encoder_tracker),
            camera_control_state: Arc::clone(&self.camera_control_state),
            camera_control_config: Arc::clone(&self.camera_control_config),
            live_tx: Arc::clone(&self.live_tx),
        }
    }
}
