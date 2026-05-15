//! OBS connection management and event handling
//!
//! Handles WebSocket connection, reconnection, event listening, and state synchronization.

use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info};

use super::driver::ObsDriver;

/// RAII releaser for the `reconnecting` single-flight guard. Ensures the
/// flag is cleared on every exit path including panic unwinds.
struct ReconnectGuard<'a>(&'a AtomicBool);
impl Drop for ReconnectGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

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
            .context("BUG: get_connected_client returned None")?;

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

        // Best-effort tap into the editor live bus (if wired). Map tray
        // statuses to the simpler Up/Down used by the editor.
        if let Some(tx) = self.live_tx.read().clone() {
            let live_status = match &status {
                crate::tray::ConnectionStatus::Connected => crate::event_bus::ConnectionStatus::Up,
                _ => crate::event_bus::ConnectionStatus::Down,
            };
            let detail = match &status {
                crate::tray::ConnectionStatus::Reconnecting { attempt } => {
                    Some(format!("reconnect attempt #{}", attempt))
                },
                _ => None,
            };
            let _ = tx.send(crate::event_bus::LiveEvent::Connection {
                target: "obs".into(),
                status: live_status,
                detail,
                ts: crate::event_bus::now_ms(),
            });
        }

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

        // Fresh session: caches from the previous (possibly different) OBS
        // session may reference scene items that no longer exist. Drop them
        // so we never serve stale item IDs / transforms across reconnects.
        self.transform_cache.write().clear();
        self.item_id_cache.write().clear();

        // Refresh initial state
        self.refresh_state().await?;

        // Start event listener
        self.spawn_event_listener();

        // Emit connection status
        self.emit_status(crate::tray::ConnectionStatus::Connected);

        info!("✅ OBS WebSocket connected");
        Ok(())
    }

    /// Spawn background task to listen to OBS events.
    ///
    /// Hands the listener a cheap `clone_for_task` of the driver so it can
    /// reuse [`ObsDriver::emit_signal`] and access shared state (caches,
    /// scene cells, shutdown flag) without re-passing every Arc field.
    pub(super) fn spawn_event_listener(&self) {
        let driver = self.clone_for_task();
        tokio::spawn(super::event_listener::run_event_listener(driver));
    }

    /// Static helper to emit selectedScene with debouncing
    pub(super) fn emit_selected_debounced(
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
                        super::signals::SELECTED_SCENE.to_string(),
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

    /// Schedule reconnection with exponential backoff.
    ///
    /// Uses the `reconnecting: AtomicBool` flag as a single-flight guard
    /// (compare_exchange) so multiple sources of "we got disconnected"
    /// (action retry, listener shutdown) cannot stack overlapping
    /// reconnect loops — which would race on `connect()` and could leak
    /// connections. The guard is released via RAII `Drop` so panic unwinding
    /// (or any future early-return path) cannot leave it stuck `true`.
    pub(super) async fn schedule_reconnect(&self) {
        use std::sync::atomic::Ordering;

        if self
            .reconnecting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!("OBS reconnect already in progress, skipping");
            return;
        }
        let _guard = ReconnectGuard(&self.reconnecting);

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
                    return;
                },
                Err(e) => {
                    debug!("OBS reconnect #{} failed: {}", retry_count, e);
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
        self.emit_signal(super::signals::STUDIO_MODE, Value::Bool(studio_mode));
        self.emit_signal(
            super::signals::CURRENT_PROGRAM_SCENE,
            Value::String(program_scene.clone()),
        );
        self.emit_signal(
            super::signals::CURRENT_PREVIEW_SCENE,
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
            self.emit_signal(
                super::signals::SELECTED_SCENE,
                Value::String(selected.clone()),
            );
            *last = Some(selected);
        }
    }
}
