//! OBS connection management and event handling
//!
//! Handles WebSocket connection, reconnection, event listening, and state synchronization.

use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::driver::ObsDriver;

impl ObsDriver {
    /// Get the connected OBS client, or an error if not connected
    ///
    /// This is a helper to reduce the repeated pattern of:
    /// ```ignore
    /// let guard = self.client.read().await;
    /// let client = guard.as_ref().context("OBS not connected")?;
    /// ```
    pub(super) async fn get_connected_client(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, Option<obws::Client>>> {
        let guard = self.client.read().await;
        if guard.is_none() {
            anyhow::bail!("OBS not connected");
        }
        Ok(guard)
    }

    /// Set the active scene, respecting studio mode
    ///
    /// In studio mode, this sets the preview scene.
    /// In normal mode, this sets the program scene.
    ///
    /// This consolidates the repeated pattern:
    /// ```ignore
    /// let studio_mode = *self.studio_mode.read();
    /// if studio_mode {
    ///     client.scenes().set_current_preview_scene(scene).await?;
    /// } else {
    ///     client.scenes().set_current_program_scene(scene).await?;
    /// }
    /// ```
    pub(super) async fn set_scene_for_mode(&self, scene_name: &str) -> Result<()> {
        let guard = self.get_connected_client().await?;
        let client = guard
            .as_ref()
            .expect("invariant: get_connected_client ensures Some");

        let studio_mode = *self.studio_mode.read();
        if studio_mode {
            client
                .scenes()
                .set_current_preview_scene(scene_name)
                .await?;
        } else {
            client
                .scenes()
                .set_current_program_scene(scene_name)
                .await?;
        }
        Ok(())
    }

    /// Emit connection status to all subscribers
    pub(super) fn emit_status(&self, status: crate::tray::ConnectionStatus) {
        *self.current_status.write() = status.clone();
        for callback in self.status_callbacks.read().iter() {
            callback(status.clone());
        }
    }

    /// Connect to OBS WebSocket
    pub(super) async fn connect(&self) -> Result<()> {
        info!("🎬 Connecting to OBS at {}:{}", self.host, self.port);

        let client = obws::Client::connect(self.host.clone(), self.port, self.password.clone())
            .await
            .context("Failed to connect to OBS WebSocket")?;

        *self.client.write().await = Some(client);
        *self.reconnect_count.lock() = 0;

        // Refresh initial state
        self.refresh_state().await?;

        // Start event listener
        self.spawn_event_listener();

        // Emit connection status
        self.emit_status(crate::tray::ConnectionStatus::Connected);

        info!("✅ OBS WebSocket connected");
        Ok(())
    }

    /// Spawn background task to listen to OBS events
    pub(super) fn spawn_event_listener(&self) {
        let client = Arc::clone(&self.client);
        let studio_mode = Arc::clone(&self.studio_mode);
        let program_scene = Arc::clone(&self.program_scene);
        let preview_scene = Arc::clone(&self.preview_scene);
        let emitters = Arc::clone(&self.indicator_emitters);
        let last_selected = Arc::clone(&self.last_selected_sent);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let activity_tracker = Arc::clone(&self.activity_tracker);

        // Clone for ViewMode synchronization
        let camera_control_config = Arc::clone(&self.camera_control_config);
        let camera_control_state = Arc::clone(&self.camera_control_state);

        // Clone for reconnection trigger
        let driver_for_reconnect = self.clone_for_task();
        let status_callbacks = Arc::clone(&self.status_callbacks);
        let current_status = Arc::clone(&self.current_status);

        tokio::spawn(async move {
            // Helper: Sync ViewMode from scene name
            let sync_view_mode = |scene_name: &str| {
                use crate::drivers::obs::camera::ViewMode;

                let config_guard = camera_control_config.read();
                if let Some(config) = config_guard.as_ref() {
                    let view_mode = if scene_name == config.splits.left {
                        Some(ViewMode::SplitLeft)
                    } else if scene_name == config.splits.right {
                        Some(ViewMode::SplitRight)
                    } else if config.cameras.iter().any(|c| c.scene == scene_name) {
                        Some(ViewMode::Full)
                    } else {
                        None
                    };

                    if let Some(new_mode) = view_mode {
                        let mut state = camera_control_state.write();
                        let old_mode = state.current_view_mode;
                        state.current_view_mode = new_mode;

                        if old_mode != new_mode {
                            debug!(
                                "ViewMode synced from scene '{}': {:?} → {:?}",
                                scene_name, old_mode, new_mode
                            );
                        }
                    }
                }
            };

            loop {
                if *shutdown_flag.lock() {
                    debug!("OBS event listener shutting down");
                    break;
                }

                // Get event stream
                let events = {
                    let guard = client.read().await;
                    match guard.as_ref() {
                        Some(c) => match c.events() {
                            Ok(stream) => stream,
                            Err(e) => {
                                warn!("Failed to get OBS event stream: {}", e);
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                continue;
                            },
                        },
                        None => {
                            // Not connected, wait and retry
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        },
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

                    // Record inbound activity from OBS
                    if let Some(ref tracker) = *activity_tracker.read() {
                        tracker.record("obs", crate::tray::ActivityDirection::Inbound);
                    }

                    match event {
                        Event::CurrentProgramSceneChanged { name } => {
                            debug!("OBS program scene changed: {}", name);
                            *program_scene.write() = name.clone();

                            // Sync ViewMode if not in studio mode
                            if !*studio_mode.read() {
                                sync_view_mode(&name);
                            }

                            // Emit signals
                            let emitters_guard = emitters.read();
                            for emit in emitters_guard.iter() {
                                emit(
                                    "obs.currentProgramScene".to_string(),
                                    Value::String(name.clone()),
                                );
                            }

                            // Schedule selectedScene emission (debounced)
                            Self::emit_selected_debounced(
                                *studio_mode.read(),
                                program_scene.read().clone(),
                                preview_scene.read().clone(),
                                Arc::clone(&emitters),
                                Arc::clone(&last_selected),
                            );
                        },

                        Event::StudioModeStateChanged { enabled } => {
                            debug!("OBS studio mode changed: {}", enabled);
                            *studio_mode.write() = enabled;

                            // Sync ViewMode based on which scene is now "active"
                            let active_scene = if enabled {
                                preview_scene.read().clone()
                            } else {
                                program_scene.read().clone()
                            };
                            sync_view_mode(&active_scene);

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
                        },

                        Event::CurrentPreviewSceneChanged { name } => {
                            debug!("OBS preview scene changed: {}", name);
                            *preview_scene.write() = name.clone();

                            // Sync ViewMode in studio mode
                            if *studio_mode.read() {
                                sync_view_mode(&name);
                            }

                            // Emit signal
                            let emitters_guard = emitters.read();
                            for emit in emitters_guard.iter() {
                                emit(
                                    "obs.currentPreviewScene".to_string(),
                                    Value::String(name.clone()),
                                );
                            }

                            // Schedule selectedScene emission (debounced)
                            Self::emit_selected_debounced(
                                *studio_mode.read(),
                                program_scene.read().clone(),
                                preview_scene.read().clone(),
                                Arc::clone(&emitters),
                                Arc::clone(&last_selected),
                            );
                        },

                        _ => {
                            // Ignore other events
                        },
                    }
                }

                // Stream ended (disconnected)
                warn!("🔌 OBS event stream closed");

                // Emit disconnected status
                {
                    *current_status.write() = crate::tray::ConnectionStatus::Disconnected;
                    for callback in status_callbacks.read().iter() {
                        callback(crate::tray::ConnectionStatus::Disconnected);
                    }
                }

                // Trigger automatic reconnection
                let driver_clone = driver_for_reconnect.clone_for_task();
                tokio::spawn(async move {
                    driver_clone.schedule_reconnect().await;
                });

                // Exit listener - new one will be spawned after successful reconnect
                break;
            }
        });
    }

    /// Static helper to emit selectedScene with debouncing
    fn emit_selected_debounced(
        studio_mode: bool,
        program_scene: String,
        preview_scene: String,
        emitters: Arc<parking_lot::RwLock<Vec<super::IndicatorCallback>>>,
        last_selected: Arc<parking_lot::RwLock<Option<String>>>,
    ) {
        tokio::spawn(async move {
            // Debounce for 80ms
            tokio::time::sleep(Duration::from_millis(80)).await;

            let selected = if studio_mode {
                preview_scene
            } else {
                program_scene
            };

            // Only emit if changed
            let mut last = last_selected.write();
            if last.as_ref() != Some(&selected) {
                let emitters_guard = emitters.read();
                for emit in emitters_guard.iter() {
                    emit(
                        "obs.selectedScene".to_string(),
                        Value::String(selected.clone()),
                    );
                }
                *last = Some(selected);
            }
        });
    }

    /// Refresh OBS state (studio mode, current scenes)
    pub(super) async fn refresh_state(&self) -> Result<()> {
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

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

        // Sync ViewMode from current scene
        // In studio mode, preview is the "active" scene for camera control
        // In normal mode, program is the active scene
        let active_scene = if studio_mode {
            self.preview_scene.read().clone()
        } else {
            self.program_scene.read().clone()
        };
        self.update_view_mode_from_scene(&active_scene);

        // Emit initial indicator signals
        self.emit_all_signals().await;

        Ok(())
    }

    /// Schedule reconnection with exponential backoff
    pub(super) async fn schedule_reconnect(&self) {
        loop {
            if *self.shutdown_flag.lock() {
                return;
            }

            let retry_count = {
                let mut count = self.reconnect_count.lock();
                *count += 1;
                *count
            };

            let delay_ms = std::cmp::min(30_000, 1000 * retry_count);
            debug!("⏳ OBS reconnect #{} in {}ms", retry_count, delay_ms);

            // Emit reconnecting status
            self.emit_status(crate::tray::ConnectionStatus::Reconnecting {
                attempt: retry_count,
            });

            sleep(Duration::from_millis(delay_ms as u64)).await;

            if *self.shutdown_flag.lock() {
                return;
            }

            match self.connect().await {
                Ok(_) => {
                    info!("✅ OBS reconnection successful");
                    return; // Success, exit loop
                },
                Err(e) => {
                    debug!("OBS reconnect #{} failed: {}", retry_count, e);
                    // Continue loop for next attempt
                },
            }
        }
    }

    /// Emit a signal to all indicator subscribers
    pub(super) fn emit_signal(&self, signal: &str, value: Value) {
        let emitters = self.indicator_emitters.read();
        for emit in emitters.iter() {
            emit(signal.to_string(), value.clone());
        }
    }

    /// Emit all indicator signals (studio mode, program/preview/selected scenes)
    pub(super) async fn emit_all_signals(&self) {
        let studio_mode = *self.studio_mode.read();
        let program_scene = self.program_scene.read().clone();
        let preview_scene = self.preview_scene.read().clone();

        // Emit individual signals
        self.emit_signal("obs.studioMode", Value::Bool(studio_mode));
        self.emit_signal(
            "obs.currentProgramScene",
            Value::String(program_scene.clone()),
        );
        self.emit_signal(
            "obs.currentPreviewScene",
            Value::String(preview_scene.clone()),
        );

        // Emit composite selectedScene signal (studioMode ? preview : program)
        let selected = if studio_mode {
            preview_scene
        } else {
            program_scene
        };

        // Only emit if changed (deduplication)
        let mut last = self.last_selected_sent.write();
        if last.as_ref() != Some(&selected) {
            self.emit_signal("obs.selectedScene", Value::String(selected.clone()));
            *last = Some(selected);
        }
    }

    /// Emit selectedScene signal with 80ms debouncing
    /// Spawns a task that delays emission to coalesce rapid changes
    pub(super) fn schedule_selected_scene_emit(&self) {
        let studio_mode = *self.studio_mode.read();
        let program_scene = self.program_scene.read().clone();
        let preview_scene = self.preview_scene.read().clone();
        let emitters = Arc::clone(&self.indicator_emitters);
        let last_selected = Arc::clone(&self.last_selected_sent);

        tokio::spawn(async move {
            // Debounce for 80ms
            tokio::time::sleep(Duration::from_millis(80)).await;

            let selected = if studio_mode {
                preview_scene
            } else {
                program_scene
            };

            // Only emit if changed
            let mut last = last_selected.write();
            if last.as_ref() != Some(&selected) {
                let emitters_guard = emitters.read();
                for emit in emitters_guard.iter() {
                    emit(
                        "obs.selectedScene".to_string(),
                        Value::String(selected.clone()),
                    );
                }
                *last = Some(selected);
            }
        });
    }
}
