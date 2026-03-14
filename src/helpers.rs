//! Utility functions for startup, shutdown, and configuration helpers.
//!
//! Contains standalone functions extracted from `main.rs` to keep it focused
//! on the application entry point and event loop.

use anyhow::Result;
use tracing::info;

use crate::config::{AppConfig, GamepadConfig};
use crate::paths::AppPaths;

/// Initialize logging with console and optional file output.
///
/// In release builds, logs are also written to daily rolling files.
/// Returns a guard that must be held for the duration of the program
/// to ensure async file writes complete.
pub fn init_logging(
    level: &str,
    paths: &AppPaths,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // Build filter with sled logs suppressed to reduce noise
    // sled emits many DEBUG logs (advancing offset, wrote lsns, etc.)
    let filter_str = format!("{},sled=warn", level);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter_str));

    // Console layer (always enabled)
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false);

    // File layer (release builds only)
    #[cfg(not(debug_assertions))]
    {
        // Try to create logs directory - if it fails, fall back to console-only
        let can_write_logs = match std::fs::create_dir_all(&paths.logs_dir) {
            Ok(_) => true,
            Err(e) => {
                eprintln!(
                    "Warning: Cannot create logs directory '{}': {}",
                    paths.logs_dir.display(),
                    e
                );
                eprintln!("File logging disabled, using console only.");
                false
            },
        };

        if can_write_logs {
            // Daily rolling log file
            let file_appender = tracing_appender::rolling::daily(&paths.logs_dir, "xtouch-gw.log");
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

            let file_layer = tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_thread_names(false)
                .with_ansi(false) // No colors in file
                .with_writer(non_blocking);

            tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .with(file_layer)
                .init();

            return Ok(Some(guard));
        } else {
            // Fall back to console-only logging
            tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .init();

            return Ok(None);
        }
    }

    // Debug builds: console only
    #[cfg(debug_assertions)]
    {
        let _ = paths; // Suppress unused warning in debug builds

        tracing_subscriber::registry()
            .with(filter)
            .with(console_layer)
            .init();

        Ok(None)
    }
}

/// Wait for a shutdown signal (Ctrl+C).
pub async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!("Failed to install CTRL+C signal handler: {}", e);
        // Fall back to waiting indefinitely - the app will need to be killed manually
        std::future::pending::<()>().await;
    }
    info!("Shutdown signal received");
}

/// Run the interactive control mapping test (--test-mappings CLI flag).
pub async fn test_control_mappings() -> Result<()> {
    use crate::control_mapping::{load_default_mappings, MidiSpec};
    use colored::*;

    println!("\n{}", "=== Testing Control Mappings ===".bold().cyan());

    let db = load_default_mappings()?;

    println!("\n{}", "Loaded Mappings:".bold());
    println!(
        "  Total controls: {}",
        db.mappings.len().to_string().green()
    );
    println!("  Groups: {}", db.groups().count().to_string().green());

    println!("\n{}", "Groups:".bold());
    for group in db.groups() {
        let count = db.get_group(group).map(|g| g.len()).unwrap_or(0);
        println!("  {} ({} controls)", group.yellow(), count);
    }

    println!("\n{}", "Sample Mappings:".bold());

    // Test fader1
    if let Some(mapping) = db.get("fader1") {
        println!("\n  {}:", "fader1".bright_white());
        println!("    Group: {}", mapping.group.cyan());
        println!("    CTRL mode: {}", mapping.ctrl_message.green());
        println!("    MCU mode:  {}", mapping.mcu_message.green());

        // Parse and display
        if let Ok(spec) = MidiSpec::parse(&mapping.ctrl_message) {
            println!("    Parsed CTRL: {:?}", spec);
        }
        if let Ok(spec) = MidiSpec::parse(&mapping.mcu_message) {
            println!("    Parsed MCU:  {:?}", spec);
        }
    }

    // Test transport controls
    println!("\n  {}:", "Transport Controls".bright_white());
    for control in &["play", "stop", "record", "rewind", "fast_forward"] {
        if let Some(mapping) = db.get(control) {
            println!(
                "    {}: CTRL={}, MCU={}",
                control.yellow(),
                mapping.ctrl_message.green(),
                mapping.mcu_message.green()
            );
        }
    }

    // Test reverse lookup
    println!("\n{}", "Reverse Lookup Test:".bold());
    let test_spec = MidiSpec::ControlChange { cc: 70 };
    if let Some(control) = db.find_control_by_midi(&test_spec, false) {
        println!("  CC 70 in CTRL mode maps to: {}", control.green());
    }

    let test_spec = MidiSpec::PitchBend { channel: 0 };
    if let Some(control) = db.find_control_by_midi(&test_spec, true) {
        println!("  PitchBend ch1 in MCU mode maps to: {}", control.green());
    }

    println!(
        "\n{}",
        "=== Control mapping test complete! ===".green().bold()
    );

    Ok(())
}

/// Build camera info list from configuration.
pub fn build_camera_infos(config: &AppConfig) -> Vec<crate::api::CameraInfo> {
    let Some(obs) = config.obs.as_ref() else {
        return Vec::new();
    };
    let Some(camera_control) = obs.camera_control.as_ref() else {
        return Vec::new();
    };
    camera_control
        .cameras
        .iter()
        .map(|c| crate::api::CameraInfo {
            id: c.id.clone(),
            scene: c.scene.clone(),
            source: c.source.clone(),
            split_source: c.split_source.clone(),
            enable_ptz: c.enable_ptz,
        })
        .collect()
}

/// Build gamepad slot info list from configuration.
pub fn build_gamepad_slot_infos(config: &AppConfig) -> Vec<crate::api::GamepadSlotInfo> {
    build_gamepad_slot_infos_from_config(&config.gamepad)
}

/// Build gamepad slot info list from an optional `GamepadConfig`.
///
/// Used both at startup (via `build_gamepad_slot_infos`) and on hot-reload
/// where only the gamepad portion of the config is available.
pub fn build_gamepad_slot_infos_from_config(
    gamepad: &Option<GamepadConfig>,
) -> Vec<crate::api::GamepadSlotInfo> {
    let Some(gamepad) = gamepad.as_ref() else {
        return Vec::new();
    };
    let Some(slots) = gamepad.gamepads.as_ref() else {
        return Vec::new();
    };
    slots
        .iter()
        .enumerate()
        .map(|(i, slot)| crate::api::GamepadSlotInfo {
            slot: format!("gamepad{}", i + 1),
            product_match: slot.product_match.clone(),
            camera_target_mode: slot
                .camera_target
                .clone()
                .unwrap_or_else(|| "static".to_string()),
            current_camera: None,
        })
        .collect()
}

/// Find the first gamepad slot configured with "dynamic" camera_target mode.
///
/// Returns the slot name (e.g., "gamepad1") if found, or None if no dynamic slot exists.
pub fn find_dynamic_gamepad_slot(config: &AppConfig) -> Option<String> {
    config
        .gamepad
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
