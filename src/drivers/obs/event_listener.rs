//! OBS WebSocket event listener and event processing
//!
//! Handles scene changes, studio mode transitions, and ViewMode synchronization.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

use super::camera::ViewMode;
use super::driver::ObsDriver;
use super::transform::ObsItemState;

/// Remove cache entries for a given `"{scene}::{source}"` key from both the
/// transform cache and the item-id cache. Keeps the OBS driver caches in sync
/// with the live OBS session — preventing slow growth from removed items.
fn purge_caches_for_item(
    transform_cache: &parking_lot::RwLock<HashMap<String, ObsItemState>>,
    item_id_cache: &parking_lot::RwLock<HashMap<String, i64>>,
    scene: &str,
    source: &str,
) {
    let key = format!("{}::{}", scene, source);
    let removed_transform = transform_cache.write().remove(&key).is_some();
    let removed_item_id = item_id_cache.write().remove(&key).is_some();
    if removed_transform || removed_item_id {
        debug!(
            "OBS cache purge: '{}' (transform={}, item_id={})",
            key, removed_transform, removed_item_id
        );
    }
}

/// Remove all cache entries whose key matches `pred`. Used to evict cached
/// transforms/item-IDs when an OBS scene or source is removed.
fn purge_caches_where(
    transform_cache: &parking_lot::RwLock<HashMap<String, ObsItemState>>,
    item_id_cache: &parking_lot::RwLock<HashMap<String, i64>>,
    kind: &str,
    target: &str,
    pred: impl Fn(&str) -> bool,
) {
    let removed_tc = {
        let mut tc = transform_cache.write();
        let before = tc.len();
        tc.retain(|k, _| !pred(k));
        before - tc.len()
    };
    let removed_ic = {
        let mut ic = item_id_cache.write();
        let before = ic.len();
        ic.retain(|k, _| !pred(k));
        before - ic.len()
    };
    if removed_tc > 0 || removed_ic > 0 {
        debug!(
            "OBS cache purge {}='{}' (transform={}, item_id={})",
            kind, target, removed_tc, removed_ic
        );
    }
}

/// Sync ViewMode from a scene name using camera control config
fn sync_view_mode(
    scene_name: &str,
    camera_control_config: &parking_lot::RwLock<Option<crate::config::CameraControlConfig>>,
    camera_control_state: &parking_lot::RwLock<super::camera::CameraControlState>,
) {
    let config_guard = camera_control_config.read();
    let Some(config) = config_guard.as_ref() else {
        return;
    };

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

        // Don't auto-downgrade from split to full — only explicit exitSplit/toggleSplit should do that
        let is_split = matches!(old_mode, ViewMode::SplitLeft | ViewMode::SplitRight);
        if is_split && new_mode == ViewMode::Full {
            debug!(
                "ViewMode: ignoring auto-sync to Full while in {:?} (scene '{}')",
                old_mode, scene_name
            );
            return;
        }

        state.current_view_mode = new_mode;

        if old_mode != new_mode {
            debug!(
                "ViewMode synced from scene '{}': {:?} → {:?}",
                scene_name, old_mode, new_mode
            );
        }
    }
}

/// Emit a signal via the driver and schedule the debounced `selectedScene`
/// follow-up. Reuses [`ObsDriver::emit_signal`] so we have a single emission
/// path for all indicator subscribers.
///
/// Skips the entire emission + debounce pipeline when `value` matches the
/// last value emitted for `signal` (change-detection guard, #31). This
/// eliminates redundant `String::clone` allocations and Tokio task spawns
/// during continuous OBS scene-switching when the same scene is reasserted.
fn emit_and_debounce(driver: &ObsDriver, signal: &'static str, value: Value) {
    {
        let last = driver.last_emitted.read();
        if last.get(signal) == Some(&value) {
            return;
        }
    }
    driver.last_emitted.write().insert(signal, value.clone());

    driver.emit_signal(signal, value);

    ObsDriver::emit_selected_debounced(
        *driver.studio_mode.read(),
        driver.program_scene.read().clone(),
        driver.preview_scene.read().clone(),
        Arc::clone(&driver.indicator_emitters),
        Arc::clone(&driver.last_selected_sent),
    );
}

/// Run the OBS event listener loop (spawned as a tokio task).
///
/// Takes the full [`ObsDriver`] (cheap to clone — every field is `Arc`-backed
/// via `clone_for_task`) so that scene/studio-mode events can route through
/// the driver's own [`ObsDriver::emit_signal`] instead of re-implementing the
/// emission loop with a stack of `Arc<RwLock<…>>` parameters.
pub(super) async fn run_event_listener(driver: ObsDriver) {
    loop {
        if *driver.shutdown_flag.lock() {
            debug!("OBS event listener shutting down");
            break;
        }

        // Get event stream
        let events = {
            let guard = driver.client.read().await;
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
            if *driver.shutdown_flag.lock() {
                break;
            }

            if let Some(ref tracker) = *driver.activity_tracker.read() {
                tracker.record("obs", crate::tray::ActivityDirection::Inbound);
            }

            match event {
                Event::CurrentProgramSceneChanged { name } => {
                    debug!("OBS program scene changed: {}", name);
                    *driver.program_scene.write() = name.clone();

                    if !*driver.studio_mode.read() {
                        sync_view_mode(
                            &name,
                            &driver.camera_control_config,
                            &driver.camera_control_state,
                        );
                    }

                    emit_and_debounce(
                        &driver,
                        super::signals::CURRENT_PROGRAM_SCENE,
                        Value::String(name),
                    );
                },

                Event::StudioModeStateChanged { enabled } => {
                    debug!("OBS studio mode changed: {}", enabled);
                    *driver.studio_mode.write() = enabled;

                    let active_scene = if enabled {
                        driver.preview_scene.read().clone()
                    } else {
                        driver.program_scene.read().clone()
                    };
                    sync_view_mode(
                        &active_scene,
                        &driver.camera_control_config,
                        &driver.camera_control_state,
                    );

                    emit_and_debounce(&driver, super::signals::STUDIO_MODE, Value::Bool(enabled));
                },

                Event::CurrentPreviewSceneChanged { name } => {
                    debug!("OBS preview scene changed: {}", name);
                    *driver.preview_scene.write() = name.clone();

                    if *driver.studio_mode.read() {
                        sync_view_mode(
                            &name,
                            &driver.camera_control_config,
                            &driver.camera_control_state,
                        );
                    }

                    emit_and_debounce(
                        &driver,
                        super::signals::CURRENT_PREVIEW_SCENE,
                        Value::String(name),
                    );
                },

                Event::SceneItemRemoved { scene, source, .. } => {
                    purge_caches_for_item(
                        &driver.transform_cache,
                        &driver.item_id_cache,
                        &scene,
                        &source,
                    );
                },

                Event::SceneRemoved { name, .. } => {
                    let prefix = format!("{}::", name);
                    purge_caches_where(
                        &driver.transform_cache,
                        &driver.item_id_cache,
                        "scene",
                        &name,
                        |k| k.starts_with(&prefix),
                    );
                },

                Event::InputRemoved { name } => {
                    let suffix = format!("::{}", name);
                    purge_caches_where(
                        &driver.transform_cache,
                        &driver.item_id_cache,
                        "source",
                        &name,
                        |k| k.ends_with(&suffix),
                    );
                },

                _ => {},
            }
        }

        // Stream ended (disconnected)
        warn!("OBS event stream closed");

        driver.emit_status(crate::tray::ConnectionStatus::Disconnected);

        let driver_clone = driver.clone_for_task();
        tokio::spawn(async move {
            driver_clone.schedule_reconnect().await;
        });

        break;
    }
}
