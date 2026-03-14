//! OBS indicator callback for LED updates and camera auto-targeting.
//!
//! Handles OBS WebSocket indicator signals (scene changes, etc.) and translates
//! them into X-Touch LED updates and Stream Deck API broadcasts.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::api;
use crate::config::{CameraControlConfig, XTouchMode};
use crate::control_mapping::{ControlMappingDB, MidiSpec};
use crate::drivers::IndicatorCallback;
use crate::router::Router;

/// Build the OBS indicator callback closure.
///
/// This creates the `IndicatorCallback` that gets subscribed to the OBS driver.
/// It handles:
/// - LED updates based on indicator signal evaluation
/// - Program scene change broadcasts to the Stream Deck API
/// - Preview scene change auto-targeting for dynamic gamepad slots
///
/// Uses the router's `Arc<RwLock<AppConfig>>` so that hot-reloaded config
/// is automatically visible to subsequent indicator events.
pub fn build_indicator_callback(
    router: Arc<Router>,
    control_db: Arc<ControlMappingDB>,
    led_tx: mpsc::Sender<Vec<u8>>,
    api_state: Arc<api::ApiState>,
) -> IndicatorCallback {
    Arc::new(move |signal: String, value: serde_json::Value| {
        let router = router.clone();
        let control_db = control_db.clone();
        let led_tx = led_tx.clone();
        let api_state = api_state.clone();

        tokio::spawn(async move {
            // Extract only needed config fields under a short read guard (avoid full clone)
            let (is_mcu_mode, camera_control, gamepad_config) = {
                let config = router.config.read().await;
                let is_mcu = config
                    .xtouch
                    .as_ref()
                    .map(|x| matches!(x.mode, XTouchMode::Mcu))
                    .unwrap_or(true);
                let cc = config.obs.as_ref().and_then(|o| o.camera_control.clone());
                let gp = config.gamepad.clone();
                (is_mcu, cc, gp)
            };
            handle_indicator_signal(
                &router,
                &control_db,
                is_mcu_mode,
                camera_control.as_ref(),
                &gamepad_config,
                &led_tx,
                &api_state,
                &signal,
                &value,
            )
            .await;
        });
    })
}

/// Process a single OBS indicator signal.
///
/// Evaluates which controls should be lit, sends LED updates, and handles
/// scene change broadcasts for the Stream Deck API.
async fn handle_indicator_signal(
    router: &Router,
    control_db: &ControlMappingDB,
    is_mcu_mode: bool,
    camera_control: Option<&CameraControlConfig>,
    gamepad_config: &Option<crate::config::GamepadConfig>,
    led_tx: &mpsc::Sender<Vec<u8>>,
    api_state: &api::ApiState,
    signal: &str,
    value: &serde_json::Value,
) {
    // Evaluate which controls should be lit
    let lit_controls = router.evaluate_indicators(signal, value).await;

    // Send LED updates to channel for each control
    send_led_updates(&lit_controls, control_db, is_mcu_mode, led_tx);

    // Handle program scene change broadcasts
    handle_program_scene_change(signal, value, camera_control, api_state);

    // Handle preview scene change auto-targeting
    handle_preview_scene_change(signal, value, camera_control, gamepad_config, api_state);
}

/// Send LED on/off messages for evaluated indicator controls.
fn send_led_updates(
    lit_controls: &HashMap<String, bool>,
    control_db: &ControlMappingDB,
    is_mcu_mode: bool,
    led_tx: &mpsc::Sender<Vec<u8>>,
) {
    for (control_id, should_be_lit) in lit_controls.iter() {
        if let Some(midi_spec) = control_db.get_midi_spec(control_id, is_mcu_mode) {
            if let MidiSpec::Note { note } = midi_spec {
                let velocity = if *should_be_lit { 127 } else { 0 };
                let midi_msg = vec![0x90, note, velocity]; // Note On, channel 1

                if let Err(e) = led_tx.try_send(midi_msg) {
                    warn!("Failed to send LED update to channel: {}", e);
                }
            }
        }
    }
}

/// Broadcast program scene changes to the Stream Deck API.
fn handle_program_scene_change(
    signal: &str,
    value: &serde_json::Value,
    camera_control: Option<&CameraControlConfig>,
    api_state: &api::ApiState,
) {
    if signal != crate::drivers::obs::signals::CURRENT_PROGRAM_SCENE {
        return;
    }

    if let Some(scene_name) = value.as_str() {
        if let Some(camera_config) =
            camera_control.and_then(|cc| cc.cameras.iter().find(|c| c.scene == scene_name))
        {
            api::broadcast_on_air_change(api_state, &camera_config.id, scene_name);
        }
    }
}

/// Handle preview scene changes for dynamic gamepad auto-targeting.
///
/// When the preview scene changes in OBS studio mode, automatically updates
/// the dynamic gamepad slot to target the corresponding camera.
fn handle_preview_scene_change(
    signal: &str,
    value: &serde_json::Value,
    camera_control: Option<&CameraControlConfig>,
    gamepad_config: &Option<crate::config::GamepadConfig>,
    api_state: &api::ApiState,
) {
    if signal != crate::drivers::obs::signals::CURRENT_PREVIEW_SCENE {
        return;
    }

    let Some(scene_name) = value.as_str() else {
        return;
    };

    // Only process if studio mode is enabled
    let is_studio_mode = api_state
        .obs_driver
        .as_ref()
        .map(|d| d.is_studio_mode())
        .unwrap_or(false);

    if !is_studio_mode {
        return; // Preview changes don't affect PTZ in non-studio mode
    }

    // Find camera matching this scene
    let Some(camera_config) =
        camera_control.and_then(|cc| cc.cameras.iter().find(|c| c.scene == scene_name))
    else {
        return;
    };

    if !camera_config.enable_ptz {
        return; // PTZ disabled for this camera
    }

    let camera_id = &camera_config.id;

    // Find the dynamic gamepad slot from gamepad config
    let Some(gamepad_slot) = find_dynamic_gamepad_slot_from_config(gamepad_config) else {
        return;
    };

    let current = api_state.camera_targets.get_target(&gamepad_slot);
    if current.as_ref() == Some(&camera_id.to_string()) {
        return; // Already targeting this camera
    }

    // Update target and broadcast
    if api_state
        .camera_targets
        .set_target(&gamepad_slot, camera_id)
        .is_ok()
    {
        api::broadcast_target_change(api_state, &gamepad_slot, camera_id);
        info!(
            "Auto-targeted {} -> {} (preview: {})",
            gamepad_slot, camera_id, scene_name
        );
    }
}

/// Find first dynamic gamepad slot from an optional `GamepadConfig`.
fn find_dynamic_gamepad_slot_from_config(
    gamepad: &Option<crate::config::GamepadConfig>,
) -> Option<String> {
    gamepad
        .as_ref()
        .and_then(|g| g.gamepads.as_ref())
        .and_then(|slots| {
            slots.iter().enumerate().find_map(|(i, slot)| {
                if slot.camera_target.as_deref() == Some("dynamic") {
                    Some(format!("gamepad{}", i + 1))
                } else {
                    None
                }
            })
        })
}
