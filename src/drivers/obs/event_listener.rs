//! OBS WebSocket event listener and event processing
//!
//! Handles scene changes, studio mode transitions, and ViewMode synchronization.

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

use super::camera::ViewMode;
use super::driver::ObsDriver;

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
