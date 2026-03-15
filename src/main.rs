//! XTouch GW v3 - Rust implementation
//!
//! Gateway to control Voicemeeter, QLC+, and OBS from Behringer X-Touch MIDI controller.

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{debug, info, warn};

mod api;
mod app;
mod cli;
mod config;
mod control_mapping;
mod display;
mod driver_setup;
mod drivers;
mod helpers;
mod input;
mod midi;
mod obs_indicators;
mod paths;
mod router;
mod sniffer;
mod state;
mod tray;
mod xtouch;

use crate::config::watcher::ConfigWatcher;
use crate::control_mapping::warm_default_mappings;
use crate::paths::AppPaths;
use crate::router::Router;

/// XTouch Gateway - Control Voicemeeter, QLC+, and OBS from Behringer X-Touch
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file (overrides auto-detection)
    #[arg(short, long)]
    config: Option<String>,

    /// Log level (error, warn, info, debug, trace)
    #[arg(short, long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Run in sniffer mode
    #[arg(long)]
    sniffer: bool,

    /// Enable web sniffer interface
    #[arg(long)]
    web_sniffer: bool,

    /// Web sniffer port
    #[arg(long, default_value = "8123")]
    web_port: u16,

    /// List available MIDI ports
    #[arg(long)]
    list_ports: bool,

    /// Test control mappings
    #[arg(long)]
    test_mappings: bool,

    /// Print gamepad diagnostics
    #[arg(long)]
    gamepad_diagnostics: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();

    // Parse command line arguments
    let args = Args::parse();

    // Detect application paths (portable vs installed mode)
    let app_paths = AppPaths::detect();

    // Resolve config path: CLI override > auto-detected
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| app_paths.config.to_string_lossy().to_string());

    // Ensure directories exist (creates %APPDATA%\XTouch GW if needed)
    if let Err(e) = app_paths.ensure_directories() {
        eprintln!("Warning: Failed to create directories: {}", e);
        eprintln!("  State dir: {}", app_paths.state_dir.display());
        eprintln!("  Logs dir: {}", app_paths.logs_dir.display());
        eprintln!("  Config: {}", app_paths.config.display());
    }

    // Initialize logging (with file output in release mode)
    let _log_guard = helpers::init_logging(&args.log_level, &app_paths)?;

    info!("Starting XTouch GW v3...");
    info!(
        "Mode: {}",
        if app_paths.is_portable {
            "portable"
        } else {
            "installed"
        }
    );
    info!("Configuration file: {}", config_path);
    info!("State directory: {}", app_paths.state_dir.display());

    // Parse and cache control mappings up-front to avoid per-event parsing
    warm_default_mappings()?;

    // Handle CLI-only modes (no config needed)
    if let Some(exit) = handle_cli_modes(&args).await? {
        return Ok(exit);
    }

    // Load configuration with hot-reload watcher
    let (config_watcher, initial_config) = ConfigWatcher::new(config_path.clone()).await?;
    debug!("Configuration loaded successfully with hot-reload enabled");

    // Ensure sled subdirectory exists
    let sled_path = app_paths.sled_db_path();
    if let Some(parent) = sled_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Create tray channels and activity tracker
    let (tray_update_tx, tray_update_rx) =
        crossbeam::channel::unbounded::<crate::tray::TrayUpdate>();
    let (tray_command_tx, tray_command_rx) =
        crossbeam::channel::unbounded::<crate::tray::TrayCommand>();

    let activity_tracker = Arc::new(crate::tray::ActivityTracker::new(
        initial_config
            .tray
            .as_ref()
            .map(|t| t.activity_led_duration_ms)
            .unwrap_or(200),
        Some(tray_update_tx.clone()),
    ));

    // Spawn tray manager on dedicated OS thread
    let tray_handle = spawn_tray_manager(&initial_config, tray_update_rx, tray_command_tx);

    // Initialize router with detected state path
    let sled_path_str = sled_path.to_string_lossy().to_string();
    let mut router = Router::with_db_path((*initial_config).clone(), &sled_path_str)?;
    router.set_activity_tracker(Arc::clone(&activity_tracker));
    let router = Arc::new(router);
    debug!(
        "Router initialized with activity tracking (db: {})",
        sled_path_str
    );

    // Load state snapshot from sled database if it exists
    hydrate_state_from_sled(&router).await;

    // Set up shutdown signal and start the main application
    let shutdown_signal = helpers::shutdown_signal();

    app::run_app(
        router,
        (*initial_config).clone(),
        config_watcher,
        shutdown_signal,
        activity_tracker,
        tray_command_rx,
        tray_update_tx,
    )
    .await?;

    // Wait for tray thread to finish
    if let Some(handle) = tray_handle {
        info!("Shutting down tray...");
        let join_result = handle.join();
        if join_result.is_err() {
            warn!("Tray thread did not exit cleanly");
        } else {
            debug!("Tray thread exited");
        }
    }

    info!("XTouch GW shutdown complete");
    Ok(())
}

/// Handle CLI-only modes that exit immediately (sniffer, list-ports, etc.).
///
/// Returns `Some(())` if a mode was handled and the program should exit,
/// or `None` if normal startup should continue.
async fn handle_cli_modes(args: &Args) -> Result<Option<()>> {
    if args.list_ports {
        sniffer::list_ports_formatted();
        return Ok(Some(()));
    }

    if args.test_mappings {
        helpers::test_control_mappings().await?;
        return Ok(Some(()));
    }

    if args.gamepad_diagnostics {
        input::gamepad::run_visualizer();
        return Ok(Some(()));
    }

    if args.sniffer || args.web_sniffer {
        if args.web_sniffer {
            info!("Starting web sniffer on port {}", args.web_port);
            sniffer::run_web_sniffer(args.web_port).await?;
        } else {
            sniffer::run_cli_sniffer().await?;
        }
        return Ok(Some(()));
    }

    Ok(None)
}

/// Spawn the system tray manager on a dedicated OS thread.
fn spawn_tray_manager(
    config: &config::AppConfig,
    tray_update_rx: crossbeam::channel::Receiver<crate::tray::TrayUpdate>,
    tray_command_tx: crossbeam::channel::Sender<crate::tray::TrayCommand>,
) -> Option<std::thread::JoinHandle<()>> {
    if !config.tray.as_ref().map(|t| t.enabled).unwrap_or(true) {
        info!("System tray disabled in config");
        return None;
    }

    debug!("Starting system tray...");
    let tray_config = config.tray.clone().unwrap_or(crate::config::TrayConfig {
        enabled: true,
        activity_led_duration_ms: 200,
        status_poll_interval_ms: 100,
        show_activity_leds: true,
        show_connection_status: true,
    });

    let tray_manager = crate::tray::TrayManager::new(tray_update_rx, tray_command_tx, tray_config);

    Some(std::thread::spawn(move || {
        if let Err(e) = tray_manager.run() {
            warn!("Tray manager error: {}", e);
        }
    }))
}

/// Load and hydrate state from the sled database.
async fn hydrate_state_from_sled(router: &Arc<Router>) {
    match router.get_persistence_actor().load_snapshot().await {
        Ok(Some(snapshot)) => {
            for (app, entries) in snapshot.states {
                router
                    .get_state_actor()
                    .hydrate_from_snapshot_and_wait(app, entries)
                    .await;
            }
            info!("State snapshot loaded from sled database");
        },
        Ok(None) => {
            debug!("No state snapshot found in sled database");
        },
        Err(e) => {
            warn!("Failed to load state snapshot: {}", e);
        },
    }
}
