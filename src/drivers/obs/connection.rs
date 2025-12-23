//! OBS connection management and event handling
//!
//! Handles WebSocket connection, reconnection, event listening, and state synchronization.

use anyhow::{Result, Context};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug, warn};

use super::driver::ObsDriver;

impl ObsDriver {
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

        tokio::spawn(async move {
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

                    // Record inbound activity from OBS
                    if let Some(ref tracker) = *activity_tracker.read() {
                        tracker.record("obs", crate::tray::ActivityDirection::Inbound);
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
        emitters: Arc<parking_lot::RwLock<Vec<super::IndicatorCallback>>>,
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
    pub(super) async fn refresh_state(&self) -> Result<()> {
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
    pub(super) async fn schedule_reconnect(&self) {
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

        // Emit reconnecting status
        self.emit_status(crate::tray::ConnectionStatus::Reconnecting {
            attempt: retry_count,
        });

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
    pub(super) fn schedule_selected_scene_emit(&self) {
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
}

