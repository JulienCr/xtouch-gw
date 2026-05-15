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

/// Emit a signal to all indicator emitters and schedule debounced selectedScene
fn emit_and_debounce(
    signal: &str,
    value: Value,
    studio_mode: &parking_lot::RwLock<bool>,
    program_scene: &parking_lot::RwLock<String>,
    preview_scene: &parking_lot::RwLock<String>,
    emitters: &Arc<parking_lot::RwLock<Vec<super::IndicatorCallback>>>,
    last_selected: &Arc<parking_lot::RwLock<Option<String>>>,
) {
    let emitters_guard = emitters.read();
    for emit in emitters_guard.iter() {
        emit(signal.to_string(), value.clone());
    }
    drop(emitters_guard);

    ObsDriver::emit_selected_debounced(
        *studio_mode.read(),
        program_scene.read().clone(),
        preview_scene.read().clone(),
        Arc::clone(emitters),
        Arc::clone(last_selected),
    );
}

/// Run the OBS event listener loop (spawned as a tokio task)
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_event_listener(
    client: Arc<tokio::sync::RwLock<Option<obws::Client>>>,
    studio_mode: Arc<parking_lot::RwLock<bool>>,
    program_scene: Arc<parking_lot::RwLock<String>>,
    preview_scene: Arc<parking_lot::RwLock<String>>,
    emitters: Arc<parking_lot::RwLock<Vec<super::IndicatorCallback>>>,
    last_selected: Arc<parking_lot::RwLock<Option<String>>>,
    shutdown_flag: Arc<parking_lot::Mutex<bool>>,
    activity_tracker: Arc<parking_lot::RwLock<Option<Arc<crate::tray::ActivityTracker>>>>,
    camera_control_config: Arc<parking_lot::RwLock<Option<crate::config::CameraControlConfig>>>,
    camera_control_state: Arc<parking_lot::RwLock<super::camera::CameraControlState>>,
    transform_cache: Arc<parking_lot::RwLock<HashMap<String, ObsItemState>>>,
    item_id_cache: Arc<parking_lot::RwLock<HashMap<String, i64>>>,
    driver_for_reconnect: ObsDriver,
) {
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

            if let Some(ref tracker) = *activity_tracker.read() {
                tracker.record("obs", crate::tray::ActivityDirection::Inbound);
            }

            match event {
                Event::CurrentProgramSceneChanged { name } => {
                    debug!("OBS program scene changed: {}", name);
                    *program_scene.write() = name.clone();

                    if !*studio_mode.read() {
                        sync_view_mode(&name, &camera_control_config, &camera_control_state);
                    }

                    emit_and_debounce(
                        super::signals::CURRENT_PROGRAM_SCENE,
                        Value::String(name),
                        &studio_mode,
                        &program_scene,
                        &preview_scene,
                        &emitters,
                        &last_selected,
                    );
                },

                Event::StudioModeStateChanged { enabled } => {
                    debug!("OBS studio mode changed: {}", enabled);
                    *studio_mode.write() = enabled;

                    let active_scene = if enabled {
                        preview_scene.read().clone()
                    } else {
                        program_scene.read().clone()
                    };
                    sync_view_mode(&active_scene, &camera_control_config, &camera_control_state);

                    emit_and_debounce(
                        super::signals::STUDIO_MODE,
                        Value::Bool(enabled),
                        &studio_mode,
                        &program_scene,
                        &preview_scene,
                        &emitters,
                        &last_selected,
                    );
                },

                Event::CurrentPreviewSceneChanged { name } => {
                    debug!("OBS preview scene changed: {}", name);
                    *preview_scene.write() = name.clone();

                    if *studio_mode.read() {
                        sync_view_mode(&name, &camera_control_config, &camera_control_state);
                    }

                    emit_and_debounce(
                        super::signals::CURRENT_PREVIEW_SCENE,
                        Value::String(name),
                        &studio_mode,
                        &program_scene,
                        &preview_scene,
                        &emitters,
                        &last_selected,
                    );
                },

                Event::SceneItemRemoved { scene, source, .. } => {
                    purge_caches_for_item(&transform_cache, &item_id_cache, &scene, &source);
                },

                Event::SceneRemoved { name, .. } => {
                    let prefix = format!("{}::", name);
                    purge_caches_where(&transform_cache, &item_id_cache, "scene", &name, |k| {
                        k.starts_with(&prefix)
                    });
                },

                Event::InputRemoved { name } => {
                    let suffix = format!("::{}", name);
                    purge_caches_where(&transform_cache, &item_id_cache, "source", &name, |k| {
                        k.ends_with(&suffix)
                    });
                },

                _ => {},
            }
        }

        // Stream ended (disconnected)
        warn!("OBS event stream closed");

        driver_for_reconnect.emit_status(crate::tray::ConnectionStatus::Disconnected);

        let driver_clone = driver_for_reconnect.clone_for_task();
        tokio::spawn(async move {
            driver_clone.schedule_reconnect().await;
        });

        break;
    }
}
