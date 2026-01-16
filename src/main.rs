//! XTouch GW v3 - Rust implementation
//!
//! Gateway to control Voicemeeter, QLC+, and OBS from Behringer X-Touch MIDI controller.

use anyhow::Result;
use clap::Parser;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod config;
mod control_mapping;
mod drivers;
mod input;
mod midi;
mod router;
mod sniffer;
mod state;
mod tray;
mod xtouch;

use crate::config::{watcher::ConfigWatcher, AppConfig};
use crate::control_mapping::{warm_default_mappings, ControlMappingDB, MidiSpec};
use crate::drivers::midibridge::MidiBridgeDriver;
use crate::drivers::obs::ObsDriver;
use crate::drivers::{Driver, IndicatorCallback};
use crate::router::Router;
use crate::xtouch::XTouchDriver;
use std::sync::Arc;

/// XTouch Gateway - Control Voicemeeter, QLC+, and OBS from Behringer X-Touch
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.yaml")]
    config: String,

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

    // Initialize logging
    init_logging(&args.log_level)?;

    info!("Starting XTouch GW v3...");
    info!("Configuration file: {}", args.config);

    // Parse and cache control mappings up-front to avoid per-event parsing
    warm_default_mappings()?;

    // Handle list ports
    if args.list_ports {
        sniffer::list_ports_formatted();
        return Ok(());
    }

    // Handle test mappings
    if args.test_mappings {
        test_control_mappings().await?;
        return Ok(());
    }

    // Handle gamepad diagnostics
    if args.gamepad_diagnostics {
        input::gamepad::run_visualizer();
        return Ok(());
    }

    // Handle sniffer mode
    if args.sniffer || args.web_sniffer {
        if args.web_sniffer {
            info!("Starting web sniffer on port {}", args.web_port);
            sniffer::run_web_sniffer(args.web_port).await?;
        } else {
            sniffer::run_cli_sniffer().await?;
        }
        return Ok(());
    }

    // Load configuration with hot-reload watcher
    let (config_watcher, initial_config) = ConfigWatcher::new(args.config.clone()).await?;
    debug!("Configuration loaded successfully with hot-reload enabled");

    // Create .state directory for persistence
    tokio::fs::create_dir_all(".state").await?;

    // Create tray channels
    let (tray_update_tx, tray_update_rx) = crossbeam::channel::unbounded::<crate::tray::TrayUpdate>();
    let (tray_command_tx, tray_command_rx) = crossbeam::channel::unbounded::<crate::tray::TrayCommand>();

    // Create activity tracker for tray UI
    let activity_tracker = Arc::new(crate::tray::ActivityTracker::new(
        initial_config.tray.as_ref().map(|t| t.activity_led_duration_ms).unwrap_or(200),
        Some(tray_update_tx.clone()),
    ));

    // Spawn tray manager on dedicated OS thread
    let tray_handle = if initial_config.tray.as_ref().map(|t| t.enabled).unwrap_or(true) {
        debug!("Starting system tray...");
        let tray_config = initial_config.tray.clone().unwrap_or_else(|| crate::config::TrayConfig {
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
    } else {
        info!("System tray disabled in config");
        None
    };

    // Initialize router
    let mut router = Router::new((*initial_config).clone());
    router.set_activity_tracker(Arc::clone(&activity_tracker));
    let router = Arc::new(router);
    debug!("Router initialized with activity tracking");

    // Load state snapshot from sled database if it exists
    // IMPORTANT: Use _and_wait() to ensure state is fully loaded before page refresh
    match router.get_persistence_actor().load_snapshot().await {
        Ok(Some(snapshot)) => {
            // Hydrate state actor with loaded snapshot (wait for each to complete)
            for (app, entries) in snapshot.states {
                router
                    .get_state_actor()
                    .hydrate_from_snapshot_and_wait(app, entries)
                    .await;
            }
            info!("State snapshot loaded from sled database");
        }
        Ok(None) => {
            debug!("No state snapshot found in sled database");
        }
        Err(e) => {
            warn!("Failed to load state snapshot: {}", e);
        }
    }

    // Set up shutdown signal
    let shutdown_signal = shutdown_signal();

    // Start the main application
    run_app(
        router,
        (*initial_config).clone(),
        config_watcher,
        shutdown_signal,
        activity_tracker,
        tray_command_rx,
        tray_update_tx,
    )
    .await?;

    // Wait for tray thread to finish if it exists
    // Note: tray_update_tx is automatically dropped when run_app returns,
    // which signals the tray thread to exit via channel disconnection
    if let Some(handle) = tray_handle {
        info!("Shutting down tray...");

        // Wait for tray thread to finish
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

async fn run_app(
    router: Arc<Router>,
    config: AppConfig,
    mut config_watcher: ConfigWatcher,
    shutdown: impl std::future::Future<Output = ()>,
    activity_tracker: Arc<crate::tray::ActivityTracker>,
    tray_command_rx: crossbeam::channel::Receiver<crate::tray::TrayCommand>,
    tray_update_tx: crossbeam::channel::Sender<crate::tray::TrayUpdate>,
) -> Result<()> {
    use tracing::{debug, trace, warn};

    debug!("Starting main application loop...");

    // Create tray message handler for driver status updates
    let activity_poll_interval = config.tray.as_ref()
        .map(|t| t.status_poll_interval_ms)
        .unwrap_or(100);

    let tray_handler = Arc::new(crate::tray::TrayMessageHandler::new(
        tray_update_tx.clone(),
        Some(Arc::clone(&activity_tracker)),
        activity_poll_interval,
    ));

    // Spawn tray handler task (runs until shutdown)
    let _handler_task = tokio::spawn({
        let handler = Arc::clone(&tray_handler);
        async move {
            handler.run().await;
        }
    });
    debug!("TrayMessageHandler spawned with {}ms activity polling", activity_poll_interval);

    // Create and connect X-Touch driver
    let mut xtouch = XTouchDriver::new(&config)?;
    debug!("X-Touch driver created");

    xtouch.connect().await?;
    info!("X-Touch connected successfully");

    // Initialize LCD and LEDs for the active page
    debug!("Initializing X-Touch display...");

    // Clear all displays first
    if let Err(e) = xtouch.clear_all_lcds().await {
        warn!("Failed to clear LCDs: {}", e);
    }

    // Get active page config
    let active_page = router.get_active_page().await;
    let active_page_name = router.get_active_page_name().await;

    if let Some(page) = active_page {
        // Apply LCD labels and colors
        let labels = page.lcd.as_ref().and_then(|lcd| lcd.labels.as_ref());

        // Convert LcdColor to u8 values
        let colors_u8: Option<Vec<u8>> = page.lcd.as_ref().and_then(|lcd| {
            lcd.colors.as_ref().map(|colors| {
                colors.iter().map(|c| c.to_u8()).collect()
            })
        });

        if let Err(e) = xtouch
            .apply_lcd_for_page(labels, colors_u8.as_ref(), &active_page_name)
            .await
        {
            warn!("Failed to apply LCD for page: {}", e);
        }
    }

    // Update F-key LEDs to show active page
    let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1) as u8;
    if let Err(e) = router
        .update_fkey_leds_for_active_page(&xtouch, paging_channel)
        .await
    {
        warn!("Failed to update F-key LEDs: {}", e);
    }

    // Update prev/next navigation LEDs (always on)
    if let Some(paging) = &config.paging {
        if let Err(e) = router
            .update_prev_next_leds(&xtouch, paging.prev_note as u8, paging.next_note as u8)
            .await
        {
            warn!("Failed to update prev/next LEDs: {}", e);
        }
    }

    info!("✅ X-Touch display initialized");

    // NOTE: Initial state refresh is DEFERRED until after drivers are registered.
    // BUG-008 FIX: Snapshot values are marked `stale: true` and should not be sent
    // to X-Touch until drivers have had a chance to connect and send fresh feedback.
    // See the refresh call after "All drivers registered and initialized".

    // Create a channel for feedback from apps to X-Touch
    let (feedback_tx, mut feedback_rx) = mpsc::channel::<(String, Vec<u8>)>(1000);

    // Bridge tray commands from crossbeam to tokio channel
    let (tray_cmd_tx, mut tray_cmd_rx) = mpsc::unbounded_channel::<crate::tray::TrayCommand>();
    tokio::spawn(async move {
        while let Ok(cmd) = tray_command_rx.recv() {
            if tray_cmd_tx.send(cmd).is_err() {
                break;
            }
        }
    });

    // Take the event receiver from XTouch
    let mut xtouch_rx = xtouch
        .take_event_receiver()
        .ok_or_else(|| anyhow::anyhow!("Failed to get X-Touch event receiver"))?;

    // Wrap XTouch in Arc for sharing
    let xtouch = Arc::new(xtouch);

    // Take the setpoint apply receiver from router
    let mut setpoint_apply_rx = router
        .take_setpoint_receiver()
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to get setpoint receiver"))?;
    debug!("FaderSetpoint receiver initialized");

    // Register MIDI bridge drivers for each configured app
    if let Some(apps) = &config.midi.apps {
        for app_config in apps {
            let driver = Arc::new(MidiBridgeDriver::new(
                app_config.output_port.clone().unwrap_or_default(), // to_port: where we send
                app_config.input_port.clone().unwrap_or_default(),  // from_port: where we receive
                None,                                               // No filter for now
                None,                                               // No transforms for now
                false,                                              // Not optional
            ));

            // Set up feedback callback to route MIDI from app back to X-Touch via channel
            let feedback_tx_clone = feedback_tx.clone();
            let app_name = app_config.name.clone();
            driver.set_feedback_callback(Arc::new(move |data: &[u8]| {
                debug!("📥 Feedback from {}: {:02X?}", app_name, data);

                // Send to channel for main loop to forward to X-Touch
                if let Err(e) = feedback_tx_clone.try_send((app_name.clone(), data.to_vec())) {
                    warn!("Failed to send feedback to channel: {}", e);
                }
            }));

            // Subscribe to connection status for tray display
            let status_callback = tray_handler.subscribe_driver(app_config.name.clone());
            driver.subscribe_connection_status(status_callback);

            match router.register_driver(app_config.name.clone(), driver).await {
                Ok(_) => info!("✅ Registered MIDI bridge driver for: {}", app_config.name),
                Err(e) => warn!("⚠️  Failed to register MIDI bridge driver for {} (will continue without it): {}", app_config.name, e),
            }
        }
    }

    // Drop the original sender so the channel closes when all drivers are shut down
    drop(feedback_tx);

    // Load control database for LED indicator mapping
    // Try external file first for hot-reload, fall back to embedded CSV
    let control_db = match ControlMappingDB::load_from_csv("docs/xtouch-matching.csv").await {
        Ok(db) => {
            info!("✅ Loaded control database ({} controls)", db.mappings.len());
            Arc::new(db)
        },
        Err(_) => {
            // Use embedded CSV (always available)
            match control_mapping::load_default_mappings() {
                Ok(db) => {
                    info!("✅ Loaded embedded control database ({} controls)", db.mappings.len());
                    Arc::new(db)
                },
                Err(e) => {
                    warn!("⚠️  Failed to load control database: {}", e);
                    Arc::new(ControlMappingDB { mappings: Default::default(), groups: Default::default() })
                },
            }
        },
    };

    // Create LED update channel for indicator system
    let (led_tx, mut led_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Create ApiState early so it can be captured by the OBS indicator callback
    let api_state = Arc::new(api::ApiState {
        camera_targets: router.get_camera_targets(),
        available_cameras: Arc::new(parking_lot::RwLock::new(build_camera_infos(&config))),
        gamepad_slots: Arc::new(parking_lot::RwLock::new(build_gamepad_slot_infos(&config))),
        update_tx: tokio::sync::broadcast::channel(16).0,
        current_on_air_camera: Arc::new(parking_lot::RwLock::new(None)),
    });

    // Register OBS driver if configured
    if let Some(obs_config) = &config.obs {
        let obs_driver = Arc::new(ObsDriver::from_config(obs_config));

        // Subscribe to OBS indicator signals before registering
        let router_clone = router.clone();
        let control_db_clone = control_db.clone();
        let config_clone = config.clone();
        let led_tx_clone = led_tx.clone();
        let api_state_clone = Arc::clone(&api_state);

        let indicator_callback: IndicatorCallback = Arc::new(move |signal: String, value: serde_json::Value| {
            let router = router_clone.clone();
            let control_db = control_db_clone.clone();
            let config = config_clone.clone();
            let led_tx = led_tx_clone.clone();
            let api_state = api_state_clone.clone();

            tokio::spawn(async move {
                // Evaluate which controls should be lit
                let lit_controls = router.evaluate_indicators(&signal, &value).await;

                // Get MCU mode from config
                let is_mcu_mode = config.xtouch.as_ref()
                    .map(|x| matches!(x.mode, crate::config::XTouchMode::Mcu))
                    .unwrap_or(true);

                // Send LED updates to channel for each control
                for (control_id, should_be_lit) in lit_controls.iter() {
                    if let Some(midi_spec) = control_db.get_midi_spec(control_id, is_mcu_mode) {
                        if let MidiSpec::Note { note } = midi_spec {
                            let velocity = if *should_be_lit { 127 } else { 0 };
                            let midi_msg = vec![0x90, note, velocity]; // Note On, channel 1

                            if let Err(e) = led_tx.send(midi_msg) {
                                warn!("Failed to send LED update to channel: {}", e);
                            }
                        }
                    }
                }

                // Check if this is a program scene change and broadcast to Stream Deck API
                // The OBS driver emits "obs.currentProgramScene" with Value::String(scene_name)
                if signal == "obs.currentProgramScene" {
                    if let Some(scene_name) = value.as_str() {
                        // Find camera matching this scene
                        if let Some(camera_config) = config.obs.as_ref()
                            .and_then(|o| o.camera_control.as_ref())
                            .and_then(|cc| cc.cameras.iter().find(|c| c.scene == scene_name))
                        {
                            api::broadcast_on_air_change(&api_state, &camera_config.id, scene_name);
                        }
                    }
                }
            });
        });

        obs_driver.subscribe_indicators(indicator_callback);
        debug!("Subscribed to OBS indicator signals");

        // Subscribe to connection status for tray display
        let status_callback = tray_handler.subscribe_driver("OBS".to_string());
        obs_driver.subscribe_connection_status(status_callback);

        match router.register_driver("obs".to_string(), obs_driver).await {
            Ok(_) => info!("✅ Registered OBS driver"),
            Err(e) => warn!("⚠️  Failed to register OBS driver (will continue without it): {}", e),
        }
    }

    // Keep led_tx alive for the duration of the program
    // Don't drop it or the channel will close!

    // Note: QLC+ is controlled via MidiBridgeDriver configured in config.midi.apps
    // No separate QlcDriver stub is needed - the MIDI bridge handles everything.

    debug!("All drivers registered and initialized");

    // Spawn Stream Deck API server task
    let api_port = api::DEFAULT_API_PORT;
    tokio::spawn({
        let api_state = Arc::clone(&api_state);
        async move {
            if let Err(e) = api::start_server(api_state, api_port).await {
                warn!("API server error: {}", e);
            }
        }
    });
    info!("Stream Deck API server started on port {}", api_port);

    // BUG-008 FIX: Wait for configurable delay before initial refresh.
    // This allows drivers time to:
    // 1. Complete their connection handshakes (WebSocket for OBS, MIDI port open)
    // 2. Receive and forward initial state feedback from applications
    // 3. Update StateStore with fresh values (stale=false) that supersede snapshot values
    //
    // The delay is configurable via xtouch.startup_refresh_delay_ms (default: 500ms).
    // Fresh feedback arriving during this delay will be stored with stale=false,
    // which takes priority over snapshot values (stale=true) per BUG-005 fix.
    let startup_delay_ms = config.xtouch.as_ref()
        .map(|x| x.startup_refresh_delay_ms)
        .unwrap_or(500);

    if startup_delay_ms > 0 {
        debug!("Waiting {}ms for drivers to sync before initial refresh (BUG-008 fix)...", startup_delay_ms);
        tokio::time::sleep(std::time::Duration::from_millis(startup_delay_ms)).await;
    }

    debug!("Applying initial state to X-Touch (post-driver registration)...");
    router.refresh_page().await;

    // Send pending MIDI messages from refresh to X-Touch
    let pending_midi = router.take_pending_midi().await;
    for msg in pending_midi {
        trace!("  -> Sending initial MIDI: {:02X?}", msg);
        if let Err(e) = xtouch.send_raw(&msg).await {
            warn!("Failed to send initial refresh MIDI: {}", e);
        }
    }
    debug!("Initial state applied to X-Touch");

    // Initialize gamepad if enabled
    let _gamepad_mapper = if let Some(gamepad_config) = &config.gamepad {
        input::gamepad::init(gamepad_config, router.clone(), api_state.update_tx.clone()).await
    } else {
        None
    };

    info!("Ready to process MIDI events!");

    // Main event loop
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            // Apply fader setpoints (from FaderSetpoint async tasks)
            Some(cmd) = setpoint_apply_rx.recv() => {
                let setpoint = router.get_fader_setpoint();

                // Check if epoch still current (double-check for race conditions)
                if setpoint.is_epoch_current(cmd.channel, cmd.epoch) {
                    debug!("🎚️  Applying setpoint: ch={} value={} epoch={}", cmd.channel, cmd.value14, cmd.epoch);
                    let fader_num = cmd.channel - 1; // Convert 1-based to 0-based
                    if let Err(_e) = xtouch.set_fader(fader_num, cmd.value14).await {
                        // Retry logic: reschedule after 120ms
                        trace!("Setpoint apply failed, requeueing: ch={} value={}", cmd.channel, cmd.value14);
                        setpoint.schedule(cmd.channel, cmd.value14, Some(120));
                    }
                } else {
                    trace!("Setpoint apply skipped (obsolete): ch={} epoch={}", cmd.channel, cmd.epoch);
                }
            }

            // Handle LED indicator updates
            Some(midi_msg) = led_rx.recv() => {
                if let Err(e) = xtouch.send_raw(&midi_msg).await {
                    warn!("Failed to send LED update: {}", e);
                }
            }

            // Handle X-Touch events
            Some(event) = xtouch_rx.recv() => {
                debug!("Received X-Touch event: raw={:02X?}", event.raw_data);

                // Route the event through the router
                router.on_midi_from_xtouch(&event.raw_data).await;

                // Check if page changed and display needs update
                if router.check_and_clear_display_update().await {
                    debug!("Updating display after page change...");

                    // Send pending MIDI messages to X-Touch (e.g., Note Off for unmapped buttons)
                    let pending_midi = router.take_pending_midi().await;
                    for msg in pending_midi {
                        trace!("  → Sending MIDI: {:02X?}", msg);
                        if let Err(e) = xtouch.send_raw(&msg).await {
                            warn!("Failed to send page refresh MIDI: {}", e);
                        }
                    }

                    // Get active page config
                    let active_page = router.get_active_page().await;
                    let active_page_name = router.get_active_page_name().await;

                    if let Some(page) = active_page {
                        // Apply LCD labels and colors
                        let labels = page.lcd.as_ref().and_then(|lcd| lcd.labels.as_ref());

                        // Convert LcdColor to u8 values
                        let colors_u8: Option<Vec<u8>> = page.lcd.as_ref().and_then(|lcd| {
                            lcd.colors.as_ref().map(|colors| {
                                colors.iter().map(|c| c.to_u8()).collect()
                            })
                        });

                        if let Err(e) = xtouch.apply_lcd_for_page(labels, colors_u8.as_ref(), &active_page_name).await {
                            warn!("Failed to apply LCD for page: {}", e);
                        }
                    }

                    // Update F-key LEDs to show active page
                    let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1) as u8;
                    if let Err(e) = router.update_fkey_leds_for_active_page(&xtouch, paging_channel).await {
                        warn!("Failed to update F-key LEDs: {}", e);
                    }

                    // Also update prev/next navigation LEDs (keep them on)
                    if let Some(paging) = &config.paging {
                        if let Err(e) = router.update_prev_next_leds(&xtouch, paging.prev_note as u8, paging.next_note as u8).await {
                            warn!("Failed to update prev/next LEDs: {}", e);
                        }
                    }

                    debug!("Display updated for page: {}", active_page_name);
                }
            }

            // Handle feedback from applications → X-Touch
            Some((app_name, feedback_data)) = feedback_rx.recv() => {
                // BUG-006 FIX: Capture epoch IMMEDIATELY on receive, before any async operations
                // This prevents race conditions where a page change occurs while processing feedback.
                let captured_epoch = router.get_page_epoch();

                // Record activity from application
                activity_tracker.record(&app_name, crate::tray::ActivityDirection::Inbound);

                // BUG-002 FIX: Activate squelch BEFORE state update to prevent race condition
                // The race was: state update -> user moves fader -> squelch check (not yet active) -> echo
                // Now: squelch activate -> state update -> user moves fader -> squelch check (active) -> suppressed
                //
                // Extract PitchBend channel/value from feedback FIRST to activate squelch early
                let pb_info = extract_pitchbend_from_feedback(&feedback_data);
                if pb_info.is_some() {
                    // Activate 120ms squelch BEFORE state update (matches TypeScript emit.ts timing)
                    // This prevents the motor movement echo from passing through during state update
                    xtouch.activate_squelch(120);
                    debug!("📤 Squelch activated early for PitchBend feedback");
                }

                // BUG-006 FIX: Re-verify epoch is still current before state update
                // If epoch changed, a page transition occurred and this feedback is stale
                if !router.is_epoch_current(captured_epoch) {
                    trace!(
                        "Epoch changed ({} -> current), discarding stale feedback from '{}'",
                        captured_epoch,
                        app_name
                    );
                    continue;
                }

                // ALWAYS store state from all apps (needed for page refresh to restore values)
                // The X-Touch forwarding (process_feedback) will filter by active page
                router.on_midi_from_app(&app_name, &feedback_data, &app_name).await;

                // BUG-006 FIX: Check epoch again before forwarding to X-Touch
                // process_feedback() is async and may have been preempted by a page change
                if !router.is_epoch_current(captured_epoch) {
                    trace!(
                        "Epoch changed before X-Touch forward, discarding feedback from '{}'",
                        app_name
                    );
                    continue;
                }

                // Conditionally forward to X-Touch (only if app mapped on active page)
                // This prevents off-page apps from moving faders
                // (matches TypeScript forwardFromApp behavior)
                if let Some(transformed) = router.process_feedback(&app_name, &feedback_data).await {
                    // BUG-006 FIX: Final epoch check after async process_feedback
                    // This is the last line of defense before actually moving hardware
                    if !router.is_epoch_current(captured_epoch) {
                        trace!(
                            "Epoch changed during process_feedback, not forwarding to X-Touch"
                        );
                        continue;
                    }

                    debug!("📤 Forwarding feedback to X-Touch: {:02X?}", transformed);

                    // Check if this is a PitchBend message - use set_fader
                    if let Some((channel, value14)) = pb_info {
                        debug!("📤 Using set_fader for feedback: ch={} value={}", channel, value14);

                        // Squelch was already activated above before state update
                        if let Err(e) = xtouch.set_fader(channel, value14).await {
                            warn!("Failed to set fader from feedback: {}", e);
                        } else {
                            // Record outbound activity to X-Touch
                            activity_tracker.record("xtouch", crate::tray::ActivityDirection::Outbound);
                        }
                    } else {
                        // For other message types, use send_raw
                        if let Err(e) = xtouch.send_raw(&transformed).await {
                            warn!("Failed to send feedback to X-Touch: {}", e);
                        } else {
                            // Record outbound activity to X-Touch
                            activity_tracker.record("xtouch", crate::tray::ActivityDirection::Outbound);
                        }
                    }
                }
            }

            // Handle config reload
            Some(new_config) = config_watcher.next_config() => {
                info!("📝 Configuration file changed, reloading...");

                match router.update_config(new_config).await {
                    Ok(()) => {
                        info!("✅ Configuration reloaded successfully without dropping events");
                    }
                    Err(e) => {
                        warn!("⚠️  Failed to reload config (keeping old config): {}", e);
                    }
                }
            }

            // Periodic state snapshot save (every 5 seconds, debounced by persistence actor)
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                if let Err(e) = router.save_state_snapshot().await {
                    warn!("Failed to save state snapshot: {}", e);
                }
            }

            // Handle tray commands
            Some(cmd) = tray_cmd_rx.recv() => {
                debug!("Tray command received: {:?}", cmd);
                match cmd {
                    crate::tray::TrayCommand::ConnectObs => {
                        debug!("Attempting to reconnect OBS from tray command...");
                        if let Some(obs_driver) = router.get_driver("obs").await {
                            if let Err(e) = obs_driver.sync().await {
                                warn!("Failed to reconnect OBS: {}", e);
                            } else {
                                debug!("OBS sync initiated");
                            }
                        }
                    }
                    crate::tray::TrayCommand::RecheckAll => {
                        debug!("Rechecking all drivers from tray command...");
                        for driver_name in router.list_drivers().await {
                            if let Some(driver) = router.get_driver(&driver_name).await {
                                if let Err(e) = driver.sync().await {
                                    warn!("Failed to sync driver {}: {}", driver_name, e);
                                }
                            }
                        }
                    }
                    crate::tray::TrayCommand::Shutdown => {
                        info!("Shutdown requested from tray");
                        break;
                    }
                }
            }

            // Handle shutdown signal
            _ = &mut shutdown => {
                info!("Shutdown signal received, stopping event loop");
                break;
            }
        }
    }

    // Cleanup
    info!("Shutting down...");
    router.shutdown_all_drivers().await?;
    debug!("All drivers shut down");

    // Save and flush final state snapshot
    if let Err(e) = router.save_state_snapshot().await {
        warn!("Failed to save final state snapshot: {}", e);
    }
    if let Err(e) = router.flush_state_snapshot().await {
        warn!("Failed to flush final state snapshot: {}", e);
    } else {
        info!("State snapshot saved");
    }

    // Reset X-Touch hardware to clean state before disconnecting
    if let Err(e) = xtouch.reset_all(true).await {
        warn!("Failed to reset X-Touch hardware on shutdown: {}", e);
    }

    // Note: XTouch will be automatically disconnected when dropped
    drop(xtouch);

    Ok(())
}

fn init_logging(level: &str) -> Result<()> {
    // Build filter with sled logs suppressed to reduce noise
    // sled emits many DEBUG logs (advancing offset, wrote lsns, etc.)
    let filter_str = format!("{},sled=warn", level);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter_str));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_thread_names(false),
        )
        .init();

    Ok(())
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!("Failed to install CTRL+C signal handler: {}", e);
        // Fall back to waiting indefinitely - the app will need to be killed manually
        std::future::pending::<()>().await;
    }
    info!("Shutdown signal received");
}

async fn test_control_mappings() -> Result<()> {
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

    println!("\n{}", "✅ Control mapping test complete!".green().bold());

    Ok(())
}

/// Build camera info list from configuration.
fn build_camera_infos(config: &AppConfig) -> Vec<api::CameraInfo> {
    let Some(obs) = config.obs.as_ref() else {
        return Vec::new();
    };
    let Some(camera_control) = obs.camera_control.as_ref() else {
        return Vec::new();
    };
    camera_control
        .cameras
        .iter()
        .map(|c| api::CameraInfo {
            id: c.id.clone(),
            scene: c.scene.clone(),
            source: c.source.clone(),
            split_source: c.split_source.clone(),
            enable_ptz: c.enable_ptz,
        })
        .collect()
}

/// Build gamepad slot info list from configuration.
fn build_gamepad_slot_infos(config: &AppConfig) -> Vec<api::GamepadSlotInfo> {
    let Some(gamepad) = config.gamepad.as_ref() else {
        return Vec::new();
    };
    let Some(slots) = gamepad.gamepads.as_ref() else {
        return Vec::new();
    };
    slots
        .iter()
        .enumerate()
        .map(|(i, slot)| api::GamepadSlotInfo {
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

/// Extract PitchBend channel and 14-bit value from raw MIDI feedback data.
///
/// This helper is used to detect PitchBend messages early in the feedback handling
/// path so that squelch can be activated BEFORE state updates (BUG-002 fix).
///
/// Returns `Some((channel, value14))` if the data is a valid PitchBend message,
/// or `None` for all other message types.
fn extract_pitchbend_from_feedback(data: &[u8]) -> Option<(u8, u16)> {
    // PitchBend message format: [0xE0-0xEF, LSB, MSB]
    // Status byte: 0xEn where n is the channel (0-15)
    if data.len() >= 3 {
        let status = data[0];
        if (status & 0xF0) == 0xE0 {
            let channel = status & 0x0F;
            let lsb = data[1] & 0x7F; // 7-bit LSB
            let msb = data[2] & 0x7F; // 7-bit MSB
            let value14 = ((msb as u16) << 7) | (lsb as u16);
            return Some((channel, value14));
        }
    }
    None
}
