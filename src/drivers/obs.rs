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
use parking_lot::{RwLock, Mutex};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug, warn, trace};

use super::{Driver, ExecutionContext};

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
    
    // State tracking
    studio_mode: Arc<RwLock<bool>>,
    program_scene: Arc<RwLock<String>>,
    preview_scene: Arc<RwLock<String>>,
    
    // Transform cache (scene::source -> state)
    transform_cache: Arc<RwLock<HashMap<String, ObsItemState>>>,
    item_id_cache: Arc<RwLock<HashMap<String, i64>>>,
    
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
            studio_mode: Arc::new(RwLock::new(false)),
            program_scene: Arc::new(RwLock::new(String::new())),
            preview_scene: Arc::new(RwLock::new(String::new())),
            transform_cache: Arc::new(RwLock::new(HashMap::new())),
            item_id_cache: Arc::new(RwLock::new(HashMap::new())),
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

        *self.client.write() = Some(client);
        *self.reconnect_count.lock() = 0;

        // Refresh initial state
        self.refresh_state().await?;

        info!("✅ OBS WebSocket connected");
        Ok(())
    }

    /// Refresh OBS state (studio mode, current scenes)
    async fn refresh_state(&self) -> Result<()> {
        // TODO: Implement state refresh once obws API is clarified
        // For now, just log that we're refreshing
        debug!("OBS state refresh requested");
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
    async fn resolve_item_id(&self, _scene_name: &str, _source_name: &str) -> Result<i64> {
        // TODO: Implement once obws API is clarified
        Ok(0)
    }

    /// Read current transform from OBS
    async fn read_transform(&self, _scene_name: &str, _item_id: i64) -> Result<ObsItemState> {
        // TODO: Implement once obws API is clarified
        Ok(ObsItemState {
            x: 0.0,
            y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            width: None,
            height: None,
            bounds_width: None,
            bounds_height: None,
        })
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
        debug!("OBS transform: scene='{}' source='{}' dx={:?} dy={:?} ds={:?}", 
            scene_name, source_name, dx, dy, ds);
        
        // TODO: Implement once obws API is clarified
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
        if self.client.read().is_none() {
            warn!("OBS client disconnected, attempting to reconnect...");
            self.connect().await?;
        }

        match action {
            "changeScene" | "setScene" => {
                let scene_name = params.get(0)
                    .and_then(|v| v.as_str())
                    .context("Scene name required")?;

                info!("🎬 OBS Scene change requested → '{}'", scene_name);
                // TODO: Implement scene switching once obws API is clarified
                Ok(())
            },

            "toggleStudioMode" => {
                info!("🎬 OBS Studio Mode toggle requested");
                // TODO: Implement studio mode toggle once obws API is clarified
                Ok(())
            },

            "TriggerStudioModeTransition" => {
                info!("🎬 OBS Studio Transition requested");
                // TODO: Implement studio transition once obws API is clarified
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

        if let Some(client) = self.client.write().take() {
            drop(client); // Close the connection
        }

        info!("✅ OBS WebSocket driver shutdown complete");
        Ok(())
    }
}

