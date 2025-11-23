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
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug, warn, trace};

use super::{Driver, ExecutionContext, IndicatorCallback};

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

        Ok(ObsItemState {
            x: transform.position_x as f64,
            y: transform.position_y as f64,
            scale_x: transform.scale_x as f64,
            scale_y: transform.scale_y as f64,
            width: Some(transform.width as f64),
            height: Some(transform.height as f64),
            bounds_width: Some(transform.bounds_width as f64),
            bounds_height: Some(transform.bounds_height as f64),
        })
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
            cached
        } else {
            // Not in cache, read from OBS
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
            // Apply scale delta
            new_state.scale_x += ds_val;
            new_state.scale_y += ds_val;

            // Clamp scale to reasonable range
            new_state.scale_x = new_state.scale_x.max(0.01).min(10.0);
            new_state.scale_y = new_state.scale_y.max(0.01).min(10.0);
        }

        // Send update to OBS
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();

        client.scene_items()
            .set_transform(obws::requests::scene_items::SetTransform {
                scene: scene_name,
                item_id,
                transform: obws::requests::scene_items::SceneItemTransform {
                    position: Some(obws::requests::scene_items::Position {
                        x: Some(new_state.x as f32),
                        y: Some(new_state.y as f32),
                        ..Default::default()
                    }),
                    scale: Some(obws::requests::scene_items::Scale {
                        x: Some(new_state.scale_x as f32),
                        y: Some(new_state.scale_y as f32),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            })
            .await
            .context("Failed to set scene item transform")?;

        // Update cache
        self.transform_cache.write().insert(cache_key, new_state);

        Ok(())
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

                // Get delta from context value if available (encoder/gamepad input)
                let delta = if let Some(value) = ctx.value {
                    match value {
                        Value::Number(n) if n.is_f64() => {
                            let v = n.as_f64().unwrap();
                            if v >= -1.0 && v <= 1.0 {
                                // Analog input (-1 to +1)
                                v * step
                            } else if v == 0.0 || v == 64.0 {
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
                    self.apply_delta(scene_name, source_name, Some(delta), None, None).await?;
                }
                Ok(())
            },

            "nudgeY" => {
                let scene_name = params.get(0).and_then(|v| v.as_str())
                    .context("Scene name required")?;
                let source_name = params.get(1).and_then(|v| v.as_str())
                    .context("Source name required")?;
                let step = params.get(2).and_then(|v| v.as_f64()).unwrap_or(2.0);

                let delta = if let Some(value) = ctx.value {
                    match value {
                        Value::Number(n) if n.is_f64() => {
                            let v = n.as_f64().unwrap();
                            if v >= -1.0 && v <= 1.0 {
                                v * step
                            } else if v == 0.0 || v == 64.0 {
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
                    self.apply_delta(scene_name, source_name, None, Some(delta), None).await?;
                }
                Ok(())
            },

            "scaleUniform" => {
                let scene_name = params.get(0).and_then(|v| v.as_str())
                    .context("Scene name required")?;
                let source_name = params.get(1).and_then(|v| v.as_str())
                    .context("Source name required")?;
                let step = params.get(2).and_then(|v| v.as_f64()).unwrap_or(0.02);

                let delta = if let Some(value) = ctx.value {
                    match value {
                        Value::Number(n) if n.is_f64() => {
                            let v = n.as_f64().unwrap();
                            if v >= -1.0 && v <= 1.0 {
                                v * step
                            } else if v == 0.0 || v == 64.0 {
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
                    self.apply_delta(scene_name, source_name, None, None, Some(delta)).await?;
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

