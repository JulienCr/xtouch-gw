//! OBS Studio WebSocket Driver
//!
//! Provides integration with OBS Studio via WebSocket protocol for:
//! - Scene switching (program/preview based on studio mode)
//! - Item transformation (position, scale)
//! - Studio mode control
//! - Automatic reconnection

use async_trait::async_trait;
use anyhow::{Result, Context};
use obws::Client as ObsClient;
use parking_lot::Mutex;
use tokio::sync::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{sleep, interval, MissedTickBehavior};
use tracing::{info, debug, warn, trace};

use super::{Driver, ExecutionContext, IndicatorCallback};

// =============================================================================
// Analog Input Processing
// =============================================================================

/// Shape analog input with deadzone and gamma curve
///
/// Applies deadzone filtering and gamma curve for finer control near center.
/// Formula:
/// 1. If |value| < deadzone → return 0
/// 2. Extract sign and magnitude
/// 3. Apply gamma curve: shaped = magnitude^gamma
/// 4. Return sign × shaped
fn shape_analog(value: f64, deadzone: f64, gamma: f64) -> f64 {
    // Return 0 if not finite or within deadzone
    if !value.is_finite() || value.abs() < deadzone {
        return 0.0;
    }

    let sign = if value >= 0.0 { 1.0 } else { -1.0 };
    let magnitude = value.abs().min(1.0).max(0.0);

    // Apply gamma curve for finer control at low values
    let shaped = magnitude.powf(gamma);

    sign * shaped
}

// =============================================================================
// Encoder Acceleration
// =============================================================================

/// Encoder speed tracking state (per encoder)
#[derive(Debug, Clone)]
struct EncoderState {
    last_ts: Option<Instant>,
    velocity_ema: f64,
    last_direction: i8,
}

/// Encoder speed tracker with adaptive acceleration
///
/// Tracks encoder rotation velocity and applies acceleration multipliers
/// for fast movements. Uses Exponential Moving Average (EMA) for smooth
/// velocity tracking.
#[derive(Debug, Clone)]
struct EncoderSpeedTracker {
    // EMA smoothing weight (0-1, higher = more responsive)
    ema_alpha: f64,
    // Reference velocity in ticks/sec for acceleration calculation
    accel_vref: f64,
    // Acceleration coefficient
    accel_k: f64,
    // Acceleration curve exponent
    accel_gamma: f64,
    // Maximum acceleration multiplier
    max_multiplier: f64,
    // Minimum interval between ticks to count (ms)
    min_interval_ms: u64,
    // Damping factor on direction change
    direction_flip_dampen: f64,
    // Idle time before resetting EMA (ms)
    idle_reset_ms: u64,
    // Per-encoder state
    states: HashMap<String, EncoderState>,
}

impl EncoderSpeedTracker {
    /// Create with default parameters (matching TypeScript implementation)
    fn new() -> Self {
        Self {
            ema_alpha: 0.75,
            accel_vref: 9.0,
            accel_k: 3.9,
            accel_gamma: 1.4,
            max_multiplier: 15.0,
            min_interval_ms: 4,
            direction_flip_dampen: 0.5,
            idle_reset_ms: 700,
            states: HashMap::new(),
        }
    }

    /// Track an encoder event and return acceleration multiplier
    ///
    /// Returns the acceleration factor to apply to base_delta.
    /// Example: track_event("vpot1", 1.0) → 3.5 (multiply base delta by 3.5x)
    fn track_event(&mut self, encoder_id: &str, base_delta: f64) -> f64 {
        let direction = base_delta.signum() as i8;
        let now = Instant::now();

        // Get or create state
        let state = self.states.entry(encoder_id.to_string()).or_insert(EncoderState {
            last_ts: None,
            velocity_ema: 0.0,
            last_direction: 0,
        });

        // Calculate instantaneous velocity (ticks per second)
        if let Some(last_ts) = state.last_ts {
            if base_delta != 0.0 {
                let interval_ms = now.duration_since(last_ts).as_millis() as u64;

                if interval_ms >= self.min_interval_ms {
                    let inst_velocity = 1000.0 / interval_ms.max(1) as f64;

                    // Update EMA (Exponential Moving Average)
                    let is_bootstrap = state.velocity_ema == 0.0
                        || interval_ms > self.idle_reset_ms;

                    state.velocity_ema = if is_bootstrap {
                        inst_velocity
                    } else {
                        self.ema_alpha * inst_velocity
                            + (1.0 - self.ema_alpha) * state.velocity_ema
                    };
                }
            }
        }

        // Update timestamp if non-zero delta
        if base_delta != 0.0 {
            state.last_ts = Some(now);
        }

        // Calculate acceleration multiplier
        let v_norm = state.velocity_ema.max(0.0) / self.accel_vref;
        let mut accel = 1.0 + self.accel_k * v_norm.powf(self.accel_gamma);
        accel = accel.max(1.0).min(self.max_multiplier);

        // Dampen on direction flip
        if base_delta != 0.0
            && state.last_direction != 0
            && direction != 0
            && direction != state.last_direction
        {
            accel *= self.direction_flip_dampen;
        }

        // Update direction
        if base_delta != 0.0 && direction != 0 {
            state.last_direction = direction;
        }

        accel
    }
}

// =============================================================================
// Analog Motion State
// =============================================================================

/// Velocity state for analog motion (per scene/source)
#[derive(Debug, Clone, Default)]
struct AnalogRate {
    scene: String,
    source: String,
    vx: f64,  // pixels per tick (at 60Hz)
    vy: f64,  // pixels per tick
    vs: f64,  // scale delta per tick
}

/// OBS item transformation state
#[derive(Debug, Clone)]
struct ObsItemState {
    x: f64,
    y: f64,
    scale_x: f64,
    scale_y: f64,
    width: Option<f64>,
    height: Option<f64>,
    bounds_width: Option<f64>,
    bounds_height: Option<f64>,
    alignment: u32,  // OBS alignment flags (LEFT=1, RIGHT=2, TOP=4, BOTTOM=8, CENTER=0)
}

/// OBS Studio WebSocket driver
pub struct ObsDriver {
    name: String,
    host: String,
    port: u16,
    password: Option<String>,

    // OBS client (wrapped for interior mutability)
    client: Arc<RwLock<Option<ObsClient>>>,

    // State tracking (using parking_lot for sync access)
    studio_mode: Arc<parking_lot::RwLock<bool>>,
    program_scene: Arc<parking_lot::RwLock<String>>,
    preview_scene: Arc<parking_lot::RwLock<String>>,

    // Transform cache (scene::source -> state)
    transform_cache: Arc<parking_lot::RwLock<HashMap<String, ObsItemState>>>,
    item_id_cache: Arc<parking_lot::RwLock<HashMap<String, i64>>>,

    // Indicator emission (using parking_lot for sync access)
    indicator_emitters: Arc<parking_lot::RwLock<Vec<IndicatorCallback>>>,
    last_selected_sent: Arc<parking_lot::RwLock<Option<String>>>,

    // Reconnection state
    reconnect_count: Arc<Mutex<usize>>,
    shutdown_flag: Arc<Mutex<bool>>,

    // Gamepad analog configuration
    analog_pan_gain: Arc<parking_lot::RwLock<f64>>,
    analog_zoom_gain: Arc<parking_lot::RwLock<f64>>,
    analog_deadzone: Arc<parking_lot::RwLock<f64>>,
    analog_gamma: Arc<parking_lot::RwLock<f64>>,

    // Analog motion state (velocity-based for gamepad)
    analog_rates: Arc<parking_lot::RwLock<HashMap<String, AnalogRate>>>,
    analog_timer_active: Arc<Mutex<bool>>,
    last_analog_tick: Arc<Mutex<Instant>>,

    // Encoder acceleration
    encoder_tracker: Arc<Mutex<EncoderSpeedTracker>>,
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
            reconnect_count: Arc::new(Mutex::new(0)),
            shutdown_flag: Arc::new(Mutex::new(false)),
            // Analog config (defaults matching config file)
            analog_pan_gain: Arc::new(parking_lot::RwLock::new(15.0)),
            analog_zoom_gain: Arc::new(parking_lot::RwLock::new(3.0)),
            analog_deadzone: Arc::new(parking_lot::RwLock::new(0.02)),
            analog_gamma: Arc::new(parking_lot::RwLock::new(1.5)),
            // Analog motion state
            analog_rates: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            analog_timer_active: Arc::new(Mutex::new(false)),
            last_analog_tick: Arc::new(Mutex::new(Instant::now())),
            // Encoder acceleration
            encoder_tracker: Arc::new(Mutex::new(EncoderSpeedTracker::new())),
        }
    }

    /// Create from config
    pub fn from_config(config: &crate::config::ObsConfig) -> Self {
        Self::new(
            config.host.clone(),
            config.port,
            config.password.clone(),
        )
    }

    /// Load analog config from gamepad settings
    pub fn load_analog_config(&self, gamepad_config: Option<&crate::config::GamepadConfig>) {
        if let Some(gamepad) = gamepad_config {
            if let Some(analog) = &gamepad.analog {
                *self.analog_pan_gain.write() = analog.pan_gain as f64;
                *self.analog_zoom_gain.write() = analog.zoom_gain as f64;
                *self.analog_deadzone.write() = analog.deadzone as f64;
                *self.analog_gamma.write() = analog.gamma as f64;
                debug!("OBS: analog config loaded (pan_gain={}, zoom_gain={}, deadzone={}, gamma={})",
                    analog.pan_gain, analog.zoom_gain, analog.deadzone, analog.gamma);
            }
        }
    }

    /// Connect to OBS WebSocket
    async fn connect(&self) -> Result<()> {
        info!("🎬 Connecting to OBS at {}:{}", self.host, self.port);

        let client = ObsClient::connect(self.host.clone(), self.port, self.password.clone())
            .await
            .context("Failed to connect to OBS WebSocket")?;

        *self.client.write().await = Some(client);
        *self.reconnect_count.lock() = 0;

        // Refresh initial state
        self.refresh_state().await?;

        // Start event listener
        self.spawn_event_listener();

        info!("✅ OBS WebSocket connected");
        Ok(())
    }

    /// Spawn background task to listen to OBS events
    fn spawn_event_listener(&self) {
        let client = Arc::clone(&self.client);
        let studio_mode = Arc::clone(&self.studio_mode);
        let program_scene = Arc::clone(&self.program_scene);
        let preview_scene = Arc::clone(&self.preview_scene);
        let emitters = Arc::clone(&self.indicator_emitters);
        let last_selected = Arc::clone(&self.last_selected_sent);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);

        tokio::spawn(async move {
            loop {
                if *shutdown_flag.lock() {
                    debug!("OBS event listener shutting down");
                    break;
                }

                // Get event stream
                let mut events = {
                    let guard = client.read().await;
                    match guard.as_ref() {
                        Some(c) => match c.events() {
                            Ok(stream) => stream,
                            Err(e) => {
                                warn!("Failed to get OBS event stream: {}", e);
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                continue;
                            }
                        },
                        None => {
                            // Not connected, wait and retry
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    }
                };

                // Process events
                use obws::events::Event;
                use tokio_stream::StreamExt;

                tokio::pin!(events);
                while let Some(event) = events.next().await {
                    if *shutdown_flag.lock() {
                        break;
                    }

                    match event {
                        Event::CurrentProgramSceneChanged { name } => {
                            debug!("OBS program scene changed: {}", name);
                            *program_scene.write() = name.clone();

                            // Emit signals
                            let emitters_guard = emitters.read();
                            for emit in emitters_guard.iter() {
                                emit("obs.currentProgramScene".to_string(), Value::String(name.clone()));
                            }

                            // Schedule selectedScene emission (debounced)
                            Self::emit_selected_debounced(
                                *studio_mode.read(),
                                program_scene.read().clone(),
                                preview_scene.read().clone(),
                                Arc::clone(&emitters),
                                Arc::clone(&last_selected),
                            );
                        }

                        Event::StudioModeStateChanged { enabled } => {
                            debug!("OBS studio mode changed: {}", enabled);
                            *studio_mode.write() = enabled;

                            // Emit signal
                            let emitters_guard = emitters.read();
                            for emit in emitters_guard.iter() {
                                emit("obs.studioMode".to_string(), Value::Bool(enabled));
                            }

                            // Schedule selectedScene emission (debounced)
                            Self::emit_selected_debounced(
                                enabled,
                                program_scene.read().clone(),
                                preview_scene.read().clone(),
                                Arc::clone(&emitters),
                                Arc::clone(&last_selected),
                            );
                        }

                        Event::CurrentPreviewSceneChanged { name } => {
                            debug!("OBS preview scene changed: {}", name);
                            *preview_scene.write() = name.clone();

                            // Emit signal
                            let emitters_guard = emitters.read();
                            for emit in emitters_guard.iter() {
                                emit("obs.currentPreviewScene".to_string(), Value::String(name.clone()));
                            }

                            // Schedule selectedScene emission (debounced)
                            Self::emit_selected_debounced(
                                *studio_mode.read(),
                                program_scene.read().clone(),
                                preview_scene.read().clone(),
                                Arc::clone(&emitters),
                                Arc::clone(&last_selected),
                            );
                        }

                        _ => {
                            // Ignore other events
                        }
                    }
                }

                // Stream ended (disconnected), wait before retry
                warn!("OBS event stream closed, waiting for reconnection...");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// Static helper to emit selectedScene with debouncing
    fn emit_selected_debounced(
        studio_mode: bool,
        program_scene: String,
        preview_scene: String,
        emitters: Arc<parking_lot::RwLock<Vec<IndicatorCallback>>>,
        last_selected: Arc<parking_lot::RwLock<Option<String>>>,
    ) {
        tokio::spawn(async move {
            // Debounce for 80ms
            tokio::time::sleep(Duration::from_millis(80)).await;

            let selected = if studio_mode { preview_scene } else { program_scene };

            // Only emit if changed
            let mut last = last_selected.write();
            if last.as_ref() != Some(&selected) {
                let emitters_guard = emitters.read();
                for emit in emitters_guard.iter() {
                    emit("obs.selectedScene".to_string(), Value::String(selected.clone()));
                }
                *last = Some(selected);
            }
        });
    }

    /// Refresh OBS state (studio mode, current scenes)
    async fn refresh_state(&self) -> Result<()> {
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();

        // Get studio mode state
        let studio_mode = client.ui().studio_mode_enabled().await?;
        *self.studio_mode.write() = studio_mode;
        debug!("OBS studio mode: {}", studio_mode);

        // Get current program scene
        let program_scene = client.scenes().current_program_scene().await?;
        *self.program_scene.write() = program_scene.clone();
        debug!("OBS program scene: {}", program_scene);

        // Get current preview scene (only valid in studio mode)
        if studio_mode {
            let preview_scene = client.scenes().current_preview_scene().await?;
            *self.preview_scene.write() = preview_scene.clone();
            debug!("OBS preview scene: {}", preview_scene);
        }

        // Emit initial indicator signals
        self.emit_all_signals().await;

        Ok(())
    }

    /// Schedule reconnection with exponential backoff
    async fn schedule_reconnect(&self) {
        if *self.shutdown_flag.lock() {
            return;
        }

        let retry_count = {
            let mut count = self.reconnect_count.lock();
            *count += 1;
            *count
        };

        let delay_ms = std::cmp::min(30_000, 1000 * retry_count);
        info!("⏳ OBS reconnect #{} in {}ms", retry_count, delay_ms);

        sleep(Duration::from_millis(delay_ms as u64)).await;

        if *self.shutdown_flag.lock() {
            return;
        }

        match self.connect().await {
            Ok(_) => {},
            Err(e) => {
                warn!("OBS reconnect failed: {}", e);
                // The next reconnect will be triggered by operations that need the connection
            }
        }
    }

    /// Get the cache key for a scene item
    fn cache_key(&self, scene_name: &str, source_name: &str) -> String {
        format!("{}::{}", scene_name, source_name)
    }

    /// Resolve scene item ID with caching
    async fn resolve_item_id(&self, scene_name: &str, source_name: &str) -> Result<i64> {
        let cache_key = self.cache_key(scene_name, source_name);

        // Check cache first
        {
            let cache = self.item_id_cache.read();
            if let Some(&id) = cache.get(&cache_key) {
                trace!("OBS item ID cache hit: {} -> {}", cache_key, id);
                return Ok(id);
            }
        }

        // Cache miss, resolve from OBS
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();

        debug!("Resolving OBS item ID: scene='{}' source='{}'", scene_name, source_name);

        let item_id = client.scene_items()
            .id(obws::requests::scene_items::Id {
                scene: scene_name,
                source: source_name,
                search_offset: None,
            })
            .await
            .with_context(|| format!("Failed to get scene item ID for '{}/{}' - verify scene and source names in OBS", scene_name, source_name))?;

        // Cache for future use
        self.item_id_cache.write().insert(cache_key.clone(), item_id);
        debug!("OBS item ID resolved and cached: {} -> {}", cache_key, item_id);

        Ok(item_id)
    }

    /// Read current transform from OBS
    async fn read_transform(&self, scene_name: &str, item_id: i64) -> Result<ObsItemState> {
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();

        let transform = client.scene_items()
            .transform(scene_name, item_id)
            .await
            .context("Failed to get scene item transform")?;

        // Convert Alignment enum to u32 bits
        let alignment_bits = transform.alignment.bits() as u32;

        debug!("OBS read_transform: scene='{}' item={} → pos=({:.1},{:.1}) scale=({:.3},{:.3}) size=({:.0}×{:.0}) bounds=({:.0}×{:.0}) align={:?}",
            scene_name, item_id,
            transform.position_x, transform.position_y,
            transform.scale_x, transform.scale_y,
            transform.width, transform.height,
            transform.bounds_width, transform.bounds_height,
            transform.alignment
        );

        Ok(ObsItemState {
            x: transform.position_x as f64,
            y: transform.position_y as f64,
            scale_x: transform.scale_x as f64,
            scale_y: transform.scale_y as f64,
            width: Some(transform.width as f64),
            height: Some(transform.height as f64),
            bounds_width: Some(transform.bounds_width as f64),
            bounds_height: Some(transform.bounds_height as f64),
            alignment: alignment_bits,
        })
    }

    /// Get OBS canvas (base) dimensions
    async fn get_canvas_dimensions(&self) -> Result<(f64, f64)> {
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();

        let video_settings = client.config().video_settings().await
            .context("Failed to get OBS video settings")?;

        let width = video_settings.base_width as f64;
        let height = video_settings.base_height as f64;

        trace!("OBS canvas dimensions: {}×{}", width, height);
        Ok((width, height))
    }

    /// Emit a signal to all indicator subscribers
    fn emit_signal(&self, signal: &str, value: Value) {
        let emitters = self.indicator_emitters.read();
        for emit in emitters.iter() {
            emit(signal.to_string(), value.clone());
        }
    }

    /// Emit all indicator signals (studio mode, program/preview/selected scenes)
    async fn emit_all_signals(&self) {
        let studio_mode = *self.studio_mode.read();
        let program_scene = self.program_scene.read().clone();
        let preview_scene = self.preview_scene.read().clone();

        // Emit individual signals
        self.emit_signal("obs.studioMode", Value::Bool(studio_mode));
        self.emit_signal("obs.currentProgramScene", Value::String(program_scene.clone()));
        self.emit_signal("obs.currentPreviewScene", Value::String(preview_scene.clone()));

        // Emit composite selectedScene signal (studioMode ? preview : program)
        let selected = if studio_mode { preview_scene } else { program_scene };

        // Only emit if changed (deduplication)
        let mut last = self.last_selected_sent.write();
        if last.as_ref() != Some(&selected) {
            self.emit_signal("obs.selectedScene", Value::String(selected.clone()));
            *last = Some(selected);
        }
    }

    /// Emit selectedScene signal with 80ms debouncing
    /// Spawns a task that delays emission to coalesce rapid changes
    fn schedule_selected_scene_emit(&self) {
        let studio_mode = *self.studio_mode.read();
        let program_scene = self.program_scene.read().clone();
        let preview_scene = self.preview_scene.read().clone();
        let emitters = Arc::clone(&self.indicator_emitters);
        let last_selected = Arc::clone(&self.last_selected_sent);

        tokio::spawn(async move {
            // Debounce for 80ms
            tokio::time::sleep(Duration::from_millis(80)).await;

            let selected = if studio_mode { preview_scene } else { program_scene };

            // Only emit if changed
            let mut last = last_selected.write();
            if last.as_ref() != Some(&selected) {
                let emitters_guard = emitters.read();
                for emit in emitters_guard.iter() {
                    emit("obs.selectedScene".to_string(), Value::String(selected.clone()));
                }
                *last = Some(selected);
            }
        });
    }

    /// Apply position/scale delta to an item
    async fn apply_delta(
        &self,
        scene_name: &str,
        source_name: &str,
        dx: Option<f64>,
        dy: Option<f64>,
        ds: Option<f64>,
    ) -> Result<()> {
        trace!("OBS transform delta: scene='{}' source='{}' dx={:?} dy={:?} ds={:?}",
            scene_name, source_name, dx, dy, ds);

        // Resolve item ID
        let item_id = self.resolve_item_id(scene_name, source_name).await?;

        // Get current transform from cache or OBS
        let cache_key = self.cache_key(scene_name, source_name);

        // Try to get from cache first
        let cached_opt = {
            let cache = self.transform_cache.read();
            cache.get(&cache_key).cloned()
        };

        let current = if let Some(cached) = cached_opt {
            // Check if cached scale looks suspicious (too small)
            // This can happen if OBS was in a weird state when we first cached it
            if cached.scale_x < 0.5 || cached.scale_y < 0.5 {
                warn!("OBS transform cache has suspicious scale ({:.3},{:.3}) for '{}' - invalidating and re-reading from OBS",
                    cached.scale_x, cached.scale_y, cache_key);
                // Invalidate cache and re-read
                self.transform_cache.write().remove(&cache_key);
                let state = self.read_transform(scene_name, item_id).await?;
                self.transform_cache.write().insert(cache_key.clone(), state.clone());
                state
            } else {
                debug!("OBS transform cache HIT: '{}' scale=({:.3},{:.3})", cache_key, cached.scale_x, cached.scale_y);
                cached
            }
        } else {
            // Not in cache, read from OBS
            debug!("OBS transform cache MISS: '{}' - reading from OBS", cache_key);
            let state = self.read_transform(scene_name, item_id).await?;
            self.transform_cache.write().insert(cache_key.clone(), state.clone());
            state
        };

        // Apply deltas
        let mut new_state = current.clone();
        if let Some(dx_val) = dx {
            new_state.x += dx_val;
        }
        if let Some(dy_val) = dy {
            new_state.y += dy_val;
        }
        if let Some(ds_val) = ds {
            // Apply scale delta multiplicatively (matching TypeScript implementation)
            // Formula: new_scale/bounds = current × (1 + delta)
            let factor = 1.0 + ds_val;

            // Get canvas dimensions to calculate center-based zoom
            let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;
            let canvas_center_x = canvas_width / 2.0;
            let canvas_center_y = canvas_height / 2.0;

            // Determine if we should use bounds-based or scale-based transform
            let use_bounds = if let (Some(bw), Some(bh)) = (current.bounds_width, current.bounds_height) {
                bw > 0.0 && bh > 0.0
            } else {
                false
            };

            if use_bounds {
                // PATH 1: Bounds-based scaling
                let bounds_w = current.bounds_width.unwrap();
                let bounds_h = current.bounds_height.unwrap();

                let new_w = (bounds_w * factor).max(1.0).round();
                let new_h = (bounds_h * factor).max(1.0).round();

                // Zoom toward/from canvas center
                // Formula: new_pos = canvas_center + (current_pos - canvas_center) * factor
                // This makes canvas center the fixed pivot point for zoom
                new_state.x = canvas_center_x + (current.x - canvas_center_x) * factor;
                new_state.y = canvas_center_y + (current.y - canvas_center_y) * factor;
                new_state.bounds_width = Some(new_w);
                new_state.bounds_height = Some(new_h);

                debug!("OBS bounds zoom: {:.0}×{:.0} * {:.3} = {:.0}×{:.0} (canvas-centered pos {:.1},{:.1} → {:.1},{:.1})",
                    bounds_w, bounds_h, factor, new_w, new_h,
                    current.x, current.y, new_state.x, new_state.y);
            } else {
                // PATH 2: Scale-based scaling
                new_state.scale_x = (current.scale_x * factor).max(0.01).min(10.0);
                new_state.scale_y = (current.scale_y * factor).max(0.01).min(10.0);

                // Zoom toward/from canvas center (same formula as bounds-based)
                new_state.x = canvas_center_x + (current.x - canvas_center_x) * factor;
                new_state.y = canvas_center_y + (current.y - canvas_center_y) * factor;

                debug!("OBS scale zoom: {:.3} * {:.3} = {:.3} (canvas-centered pos {:.1},{:.1} → {:.1},{:.1})",
                    current.scale_x, factor, new_state.scale_x,
                    current.x, current.y, new_state.x, new_state.y);
            }
        }

        // Send update to OBS
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();

        // Build transform conditionally based on what changed
        let mut transform = obws::requests::scene_items::SceneItemTransform::default();

        // Include position if changed (pan, tilt, or zoom with position adjustment)
        if dx.is_some() || dy.is_some() || ds.is_some() {
            transform.position = Some(obws::requests::scene_items::Position {
                x: Some(new_state.x as f32),
                y: Some(new_state.y as f32),
                ..Default::default()
            });
        }

        // For scale changes: include EITHER bounds OR scale (not both!)
        if ds.is_some() {
            if let (Some(bw), Some(bh)) = (new_state.bounds_width, new_state.bounds_height) {
                if bw > 0.0 && bh > 0.0 {
                    // Use bounds-based transform (for camera sources)
                    transform.bounds = Some(obws::requests::scene_items::Bounds {
                        width: Some(bw as f32),
                        height: Some(bh as f32),
                        ..Default::default()
                    });
                    debug!("OBS sending BOUNDS transform: {}×{}", bw, bh);
                } else {
                    // Bounds exist but are zero - fall back to scale
                    transform.scale = Some(obws::requests::scene_items::Scale {
                        x: Some(new_state.scale_x as f32),
                        y: Some(new_state.scale_y as f32),
                        ..Default::default()
                    });
                }
            } else {
                // No bounds - use scale-based transform (for image sources)
                transform.scale = Some(obws::requests::scene_items::Scale {
                    x: Some(new_state.scale_x as f32),
                    y: Some(new_state.scale_y as f32),
                    ..Default::default()
                });
                debug!("OBS sending SCALE transform: {:.3}×{:.3}", new_state.scale_x, new_state.scale_y);
            }
        }

        let result = client.scene_items()
            .set_transform(obws::requests::scene_items::SetTransform {
                scene: scene_name,
                item_id,
                transform,
            })
            .await;

        match result {
            Ok(_) => {
                if let (Some(bw), Some(bh)) = (new_state.bounds_width, new_state.bounds_height) {
                    if bw > 0.0 && bh > 0.0 {
                        debug!("OBS set_transform SUCCESS: '{}' pos=({:.1},{:.1}) bounds=({:.0}×{:.0})",
                            cache_key, new_state.x, new_state.y, bw, bh);
                    } else {
                        debug!("OBS set_transform SUCCESS: '{}' pos=({:.1},{:.1}) scale=({:.3},{:.3})",
                            cache_key, new_state.x, new_state.y, new_state.scale_x, new_state.scale_y);
                    }
                } else {
                    debug!("OBS set_transform SUCCESS: '{}' pos=({:.1},{:.1}) scale=({:.3},{:.3})",
                        cache_key, new_state.x, new_state.y, new_state.scale_x, new_state.scale_y);
                }
                // Update cache
                self.transform_cache.write().insert(cache_key, new_state);
                Ok(())
            },
            Err(e) => {
                warn!("OBS set_transform FAILED: '{}' error: {}", cache_key, e);
                Err(e).context("Failed to set scene item transform")
            }
        }?;

        Ok(())
    }

    /// Set analog velocity for a scene/source
    fn set_analog_rate(&self, scene_name: &str, source_name: &str, vx: Option<f64>, vy: Option<f64>, vs: Option<f64>) {
        let cache_key = self.cache_key(scene_name, source_name);

        let mut rates = self.analog_rates.write();

        // Get existing rate or create default
        let current = rates.get(&cache_key).cloned().unwrap_or_else(|| AnalogRate {
            scene: scene_name.to_string(),
            source: source_name.to_string(),
            vx: 0.0,
            vy: 0.0,
            vs: 0.0,
        });

        // Apply partial updates (only update provided values)
        let new_vx = vx.unwrap_or(current.vx);
        let new_vy = vy.unwrap_or(current.vy);
        let new_vs = vs.unwrap_or(current.vs);

        debug!(
            "OBS analog rate: {}/{} → vx={:.3} ({}), vy={:.3} ({}), vs={:.3} ({})",
            scene_name, source_name,
            new_vx, if vx.is_some() { "new" } else { "keep" },
            new_vy, if vy.is_some() { "new" } else { "keep" },
            new_vs, if vs.is_some() { "new" } else { "keep" }
        );

        if new_vx == 0.0 && new_vy == 0.0 && new_vs == 0.0 {
            // Remove entry if all velocities are zero
            rates.remove(&cache_key);
        } else {
            // Update or insert velocity with merged values
            rates.insert(cache_key, AnalogRate {
                scene: scene_name.to_string(),
                source: source_name.to_string(),
                vx: new_vx,
                vy: new_vy,
                vs: new_vs,
            });
        }

        // Manage timer based on active rates
        if rates.is_empty() {
            self.stop_analog_timer();
        } else {
            self.ensure_analog_timer();
        }
    }

    /// Start analog motion timer at ~60Hz if not already running
    fn ensure_analog_timer(&self) {
        let mut active = self.analog_timer_active.lock();
        if *active {
            return; // Already running
        }

        *active = true;
        *self.last_analog_tick.lock() = Instant::now();

        // Spawn timer task
        let rates = Arc::clone(&self.analog_rates);
        let last_tick = Arc::clone(&self.last_analog_tick);
        let timer_active = Arc::clone(&self.analog_timer_active);
        let driver_self = Arc::new(self.clone_for_timer());

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(16)); // ~60Hz
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                // Check if timer should stop
                if !*timer_active.lock() {
                    debug!("OBS analog timer stopped");
                    break;
                }

                // Calculate dt (normalized to 60Hz)
                let now = Instant::now();
                let interval_ms = {
                    let mut last = last_tick.lock();
                    let elapsed = now.duration_since(*last).as_millis() as f64;
                    *last = now;
                    elapsed
                };
                let dt = interval_ms / 16.0; // Normalize to 60Hz

                // Process all active rates
                let rates_snapshot: Vec<AnalogRate> = {
                    let r = rates.read();
                    r.values().cloned().collect()
                };

                for rate in rates_snapshot {
                    let dx = rate.vx * dt;
                    let dy = rate.vy * dt;
                    let ds = rate.vs * dt;

                    if dx != 0.0 || dy != 0.0 || ds != 0.0 {
                        let dx_opt = if dx != 0.0 { Some(dx) } else { None };
                        let dy_opt = if dy != 0.0 { Some(dy) } else { None };
                        let ds_opt = if ds != 0.0 { Some(ds) } else { None };

                        if let Err(e) = driver_self.apply_delta(
                            &rate.scene,
                            &rate.source,
                            dx_opt,
                            dy_opt,
                            ds_opt
                        ).await {
                            trace!("OBS analog tick error: {}", e);
                        }
                    }
                }

                // Check if all rates are now zero (stop timer)
                if rates.read().is_empty() {
                    *timer_active.lock() = false;
                }
            }
        });

        debug!("OBS analog timer started at ~60Hz");
    }

    /// Stop the analog motion timer
    fn stop_analog_timer(&self) {
        *self.analog_timer_active.lock() = false;
    }

    /// Clone fields needed for the timer task
    fn clone_for_timer(&self) -> Self {
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
            reconnect_count: Arc::clone(&self.reconnect_count),
            shutdown_flag: Arc::clone(&self.shutdown_flag),
            analog_pan_gain: Arc::clone(&self.analog_pan_gain),
            analog_zoom_gain: Arc::clone(&self.analog_zoom_gain),
            analog_deadzone: Arc::clone(&self.analog_deadzone),
            analog_gamma: Arc::clone(&self.analog_gamma),
            analog_rates: Arc::clone(&self.analog_rates),
            analog_timer_active: Arc::clone(&self.analog_timer_active),
            last_analog_tick: Arc::clone(&self.last_analog_tick),
            encoder_tracker: Arc::clone(&self.encoder_tracker),
        }
    }
}

#[async_trait]
impl Driver for ObsDriver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&self, _ctx: ExecutionContext) -> Result<()> {
        info!("🎬 Initializing OBS WebSocket driver");
        self.connect().await?;
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        // Check if connected, reconnect if needed
        if self.client.read().await.is_none() {
            warn!("OBS client disconnected, attempting to reconnect...");
            self.connect().await?;
        }

        match action {
            "changeScene" | "setScene" => {
                let scene_name = params.get(0)
                    .and_then(|v| v.as_str())
                    .context("Scene name required")?;

                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?
                    .clone();

                // Check studio mode to determine which scene to change
                let studio_mode = *self.studio_mode.read();

                if studio_mode {
                    info!("🎬 OBS Preview scene change → '{}'", scene_name);
                    client.scenes().set_current_preview_scene(scene_name).await?;
                } else {
                    info!("🎬 OBS Program scene change → '{}'", scene_name);
                    client.scenes().set_current_program_scene(scene_name).await?;
                }

                Ok(())
            },

            "toggleStudioMode" => {
                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?
                    .clone();

                // Get current state and toggle
                let current = *self.studio_mode.read();
                let new_state = !current;

                info!("🎬 OBS Studio Mode toggle: {} → {}", current, new_state);
                client.ui().set_studio_mode_enabled(new_state).await?;

                // When enabling studio mode, set preview to current program scene
                if new_state {
                    let program = self.program_scene.read().clone();
                    if !program.is_empty() {
                        client.scenes().set_current_preview_scene(&program).await?;
                    }
                }

                Ok(())
            },

            "TriggerStudioModeTransition" => {
                info!("🎬 OBS Studio Transition requested");
                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?
                    .clone();

                client.transitions().trigger().await?;
                Ok(())
            },

            "nudgeX" => {
                let scene_name = params.get(0).and_then(|v| v.as_str())
                    .context("Scene name required")?;
                let source_name = params.get(1).and_then(|v| v.as_str())
                    .context("Source name required")?;
                let step = params.get(2).and_then(|v| v.as_f64()).unwrap_or(2.0);

                // Check if input is from gamepad or encoder
                let is_gamepad = ctx.control_id.as_ref()
                    .map(|id| id.starts_with("gamepad."))
                    .unwrap_or(false);

                if is_gamepad {
                    // Gamepad analog input: velocity-based
                    if let Some(Value::Number(n)) = ctx.value {
                        if let Some(v) = n.as_f64() {
                            if v >= -1.0 && v <= 1.0 {
                                // Shape analog value (deadzone + gamma)
                                let deadzone = *self.analog_deadzone.read();
                                let gamma = *self.analog_gamma.read();
                                let shaped = shape_analog(v, deadzone, gamma);

                                // Calculate velocity (px per 60Hz tick)
                                let gain = *self.analog_pan_gain.read();
                                let vx = shaped * step * gain;

                                // Set analog velocity (timer will apply)
                                self.set_analog_rate(scene_name, source_name, Some(vx), None, None);
                            }
                        }
                    }
                } else {
                    // Encoder input: acceleration-based
                    let delta = if let Some(value) = ctx.value {
                        match value {
                            Value::Number(n) if n.is_f64() => {
                                let v = n.as_f64().unwrap();
                                if v == 0.0 || v == 64.0 {
                                    0.0
                                } else if v >= 1.0 && v <= 63.0 {
                                    step
                                } else if v >= 65.0 && v <= 127.0 {
                                    -step
                                } else {
                                    0.0
                                }
                            },
                            _ => 0.0,
                        }
                    } else {
                        step
                    };

                    if delta != 0.0 {
                        // Apply encoder acceleration
                        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
                        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
                        let final_delta = delta * accel;

                        debug!("OBS nudgeX encoder: id='{}' delta={} accel={:.2}x final={:.2}",
                            control_id, delta, accel, final_delta);

                        self.apply_delta(scene_name, source_name, Some(final_delta), None, None).await?;
                    }
                }
                Ok(())
            },

            "nudgeY" => {
                let scene_name = params.get(0).and_then(|v| v.as_str())
                    .context("Scene name required")?;
                let source_name = params.get(1).and_then(|v| v.as_str())
                    .context("Source name required")?;
                let step = params.get(2).and_then(|v| v.as_f64()).unwrap_or(2.0);

                // Check if input is from gamepad or encoder
                let is_gamepad = ctx.control_id.as_ref()
                    .map(|id| id.starts_with("gamepad."))
                    .unwrap_or(false);

                if is_gamepad {
                    // Gamepad analog input: velocity-based
                    if let Some(Value::Number(n)) = ctx.value {
                        if let Some(v) = n.as_f64() {
                            if v >= -1.0 && v <= 1.0 {
                                // Shape analog value (deadzone + gamma)
                                let deadzone = *self.analog_deadzone.read();
                                let gamma = *self.analog_gamma.read();
                                let shaped = shape_analog(v, deadzone, gamma);

                                // Calculate velocity (px per 60Hz tick)
                                let gain = *self.analog_pan_gain.read();
                                let vy = shaped * step * gain;

                                // Set analog velocity (timer will apply)
                                self.set_analog_rate(scene_name, source_name, None, Some(vy), None);
                            }
                        }
                    }
                } else {
                    // Encoder input: acceleration-based
                    let delta = if let Some(value) = ctx.value {
                        match value {
                            Value::Number(n) if n.is_f64() => {
                                let v = n.as_f64().unwrap();
                                if v == 0.0 || v == 64.0 {
                                    0.0
                                } else if v >= 1.0 && v <= 63.0 {
                                    step
                                } else if v >= 65.0 && v <= 127.0 {
                                    -step
                                } else {
                                    0.0
                                }
                            },
                            _ => 0.0,
                        }
                    } else {
                        step
                    };

                    if delta != 0.0 {
                        // Apply encoder acceleration
                        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
                        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
                        let final_delta = delta * accel;

                        debug!("OBS nudgeY encoder: id='{}' delta={} accel={:.2}x final={:.2}",
                            control_id, delta, accel, final_delta);

                        self.apply_delta(scene_name, source_name, None, Some(final_delta), None).await?;
                    }
                }
                Ok(())
            },

            "scaleUniform" => {
                let scene_name = params.get(0).and_then(|v| v.as_str())
                    .context("Scene name required")?;
                let source_name = params.get(1).and_then(|v| v.as_str())
                    .context("Source name required")?;
                let base = params.get(2).and_then(|v| v.as_f64()).unwrap_or(0.02);

                // Check if input is from gamepad or encoder
                let is_gamepad = ctx.control_id.as_ref()
                    .map(|id| id.starts_with("gamepad."))
                    .unwrap_or(false);

                if is_gamepad {
                    // Gamepad analog input: velocity-based
                    if let Some(Value::Number(n)) = ctx.value {
                        if let Some(v) = n.as_f64() {
                            if v >= -1.0 && v <= 1.0 {
                                // Shape analog value (deadzone + gamma)
                                let deadzone = *self.analog_deadzone.read();
                                let gamma = *self.analog_gamma.read();
                                let shaped = shape_analog(v, deadzone, gamma);

                                // Calculate velocity (scale delta per 60Hz tick)
                                let gain = *self.analog_zoom_gain.read();
                                let vs = shaped * base * gain;

                                // Set analog velocity (timer will apply)
                                self.set_analog_rate(scene_name, source_name, None, None, Some(vs));
                            }
                        }
                    }
                } else {
                    // Encoder input: acceleration-based
                    let delta = if let Some(value) = ctx.value {
                        match value {
                            Value::Number(n) if n.is_f64() => {
                                let v = n.as_f64().unwrap();
                                if v == 0.0 || v == 64.0 {
                                    0.0
                                } else if v >= 1.0 && v <= 63.0 {
                                    base
                                } else if v >= 65.0 && v <= 127.0 {
                                    -base
                                } else {
                                    0.0
                                }
                            },
                            _ => 0.0,
                        }
                    } else {
                        base
                    };

                    if delta != 0.0 {
                        // Apply encoder acceleration
                        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
                        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
                        let final_delta = delta * accel;

                        debug!("OBS scaleUniform encoder: id='{}' delta={} accel={:.2}x final={:.2}",
                            control_id, delta, accel, final_delta);

                        self.apply_delta(scene_name, source_name, None, None, Some(final_delta)).await?;
                    }
                }
                Ok(())
            },

            _ => {
                warn!("Unknown OBS action: {}", action);
                Ok(())
            }
        }
    }

    async fn sync(&self) -> Result<()> {
        debug!("OBS driver sync - refreshing state");
        self.refresh_state().await?;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down OBS WebSocket driver");
        *self.shutdown_flag.lock() = true;

        if let Some(client) = self.client.write().await.take() {
            drop(client); // Close the connection
        }

        info!("✅ OBS WebSocket driver shutdown complete");
        Ok(())
    }

    fn subscribe_indicators(&self, callback: IndicatorCallback) {
        debug!("OBS driver: new indicator subscription");
        self.indicator_emitters.write().push(callback);

        // Emit initial state immediately to new subscriber
        let studio_mode = *self.studio_mode.read();
        let program_scene = self.program_scene.read().clone();
        let preview_scene = self.preview_scene.read().clone();

        let emitters = self.indicator_emitters.read();
        if let Some(emit) = emitters.last() {
            emit("obs.studioMode".to_string(), Value::Bool(studio_mode));
            emit("obs.currentProgramScene".to_string(), Value::String(program_scene.clone()));
            emit("obs.currentPreviewScene".to_string(), Value::String(preview_scene.clone()));

            // Emit composite selectedScene
            let selected = if studio_mode { preview_scene } else { program_scene };
            emit("obs.selectedScene".to_string(), Value::String(selected));
        }
    }
}

