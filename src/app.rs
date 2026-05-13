//! Application event loop and event handling.
//!
//! Contains the main `run_app` function that sets up drivers, spawns background
//! tasks, and runs the core `tokio::select!` event loop. Event handler functions
//! for feedback, config reload, and tray commands are co-located here.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::config::{watcher::ConfigWatcher, AppConfig};
use crate::display::extract_pitchbend_from_feedback;
use crate::drivers::obs::ObsDriver;
use crate::router::Router;
use crate::xtouch::XTouchDriver;
use crate::{api, display, driver_setup, helpers, input};

/// Run the main application event loop.
///
/// Sets up X-Touch hardware, registers drivers, spawns the API server,
/// and enters the core `tokio::select!` loop that handles all event sources.
pub async fn run_app(
    router: Arc<Router>,
    config: AppConfig,
    mut config_watcher: ConfigWatcher,
    shutdown: impl std::future::Future<Output = ()>,
    activity_tracker: Arc<crate::tray::ActivityTracker>,
    tray_command_rx: crossbeam::channel::Receiver<crate::tray::TrayCommand>,
    tray_update_tx: crossbeam::channel::Sender<crate::tray::TrayUpdate>,
    profile_store: Arc<crate::config::profiles::ProfileStore>,
    live_tx: crate::event_bus::LiveEventTx,
) -> Result<()> {
    debug!("Starting main application loop...");

    // Create tray message handler for driver status updates
    let activity_poll_interval = config
        .tray
        .as_ref()
        .map(|t| t.status_poll_interval_ms)
        .unwrap_or(100);

    let tray_handler = Arc::new(crate::tray::TrayMessageHandler::new(
        tray_update_tx.clone(),
        Some(Arc::clone(&activity_tracker)),
        activity_poll_interval,
    ));

    // Spawn tray handler task (aborted during shutdown cleanup)
    let handler_task = tokio::spawn({
        let handler = Arc::clone(&tray_handler);
        async move {
            handler.run().await;
            warn!("TrayMessageHandler task exited unexpectedly");
        }
    });
    debug!(
        "TrayMessageHandler spawned with {}ms activity polling",
        activity_poll_interval
    );

    // Create and connect X-Touch driver
    let mut xtouch = XTouchDriver::new(&config)?;
    debug!("X-Touch driver created");

    xtouch.connect().await?;
    info!("X-Touch connected successfully");
    // Best-effort: announce X-Touch availability to editor live subscribers.
    let _ = live_tx.send(crate::event_bus::LiveEvent::Connection {
        target: "xtouch".into(),
        status: crate::event_bus::ConnectionStatus::Up,
        detail: None,
        ts: crate::event_bus::now_ms(),
    });

    // Initialize LCD and LEDs for the active page
    debug!("Initializing X-Touch display...");
    if let Err(e) = xtouch.clear_all_lcds().await {
        warn!("Failed to clear LCDs: {}", e);
    }

    // Take the event receiver before wrapping in Arc (requires &mut self)
    let mut xtouch_rx = xtouch
        .take_event_receiver()
        .ok_or_else(|| anyhow::anyhow!("Failed to get X-Touch event receiver"))?;

    let xtouch = Arc::new(xtouch);
    display::update_xtouch_display(&router, &xtouch).await;
    info!("X-Touch display initialized");

    // NOTE: Initial state refresh is DEFERRED until after drivers are registered.
    // BUG-008 FIX: Snapshot values are marked `stale: true` and should not be sent
    // to X-Touch until drivers have had a chance to connect and send fresh feedback.

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

    // Take the setpoint apply receiver from router
    let mut setpoint_apply_rx = router
        .take_setpoint_receiver()
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to get setpoint receiver"))?;
    debug!("FaderSetpoint receiver initialized");

    // Register MIDI bridge drivers and OBS driver
    driver_setup::register_midi_bridge_drivers(&config, &router, &feedback_tx, &tray_handler).await;

    // Load control database for LED indicator mapping
    let control_db = driver_setup::load_control_database().await;

    // Create LED update channel for indicator system (bounded to prevent unbounded growth)
    let (led_tx, mut led_rx) = mpsc::channel::<Vec<u8>>(64);

    // Create OBS driver and API state, then register
    let obs_driver: Option<Arc<ObsDriver>> = config
        .obs
        .as_ref()
        .map(|obs_config| Arc::new(ObsDriver::from_config(obs_config)));

    // Wire the live event bus into the OBS driver so connection events
    // surface on the editor `/api/live` WS.
    if let Some(d) = obs_driver.as_ref() {
        d.set_live_tx(live_tx.clone());
    }

    // Build the editor state. The OBS picker source is the OBS driver itself
    // (which implements `ObsPickerSource`); driver action catalogs are
    // snapshotted from each registered driver's `action_catalog()`.
    use crate::drivers::Driver as _;
    let mut catalogs: std::collections::HashMap<String, Vec<crate::api_editor::ActionDescriptor>> =
        std::collections::HashMap::new();
    for name in router.list_drivers().await {
        if let Some(drv) = router.get_driver(&name).await {
            catalogs.insert(name, drv.action_catalog());
        }
    }
    if let Some(obs) = obs_driver.as_ref() {
        // OBS driver isn't registered with the router until after this point;
        // ensure its catalog is included here.
        catalogs
            .entry(obs.name().to_string())
            .or_insert_with(|| obs.action_catalog());
    }
    let editor_state = Some(build_editor_state(
        Arc::clone(&profile_store),
        live_tx.clone(),
        obs_driver.clone(),
        catalogs,
        Some({
            let fs = router.get_fader_setpoint();
            std::sync::Arc::new(move |ch: u8| fs.get_desired(ch))
                as crate::api_editor::FaderSetpointReader
        }),
        Some({
            let r = router.clone();
            std::sync::Arc::new(move || {
                let r = r.clone();
                Box::pin(async move {
                    let idx = r.get_active_page_index().await;
                    r.get_active_page().await.map(|p| (idx, p.name))
                }) as crate::api_editor::PageFuture<Option<(usize, String)>>
            }) as crate::api_editor::ActivePageReader
        }),
        Some({
            let r = router.clone();
            std::sync::Arc::new(move |idx: usize| {
                let r = r.clone();
                Box::pin(async move { r.set_active_page(&idx.to_string()).await })
                    as crate::api_editor::PageFuture<anyhow::Result<()>>
            }) as crate::api_editor::ActivePageSetter
        }),
    ));

    let api_state = Arc::new(api::ApiState {
        camera_targets: router.get_camera_targets(),
        available_cameras: Arc::new(parking_lot::RwLock::new(helpers::build_camera_infos(
            &config,
        ))),
        gamepad_slots: Arc::new(parking_lot::RwLock::new(
            helpers::build_gamepad_slot_infos_from_config(&config.gamepad),
        )),
        update_tx: tokio::sync::broadcast::channel(16).0,
        current_on_air_camera: Arc::new(parking_lot::RwLock::new(None)),
        obs_driver: obs_driver.clone(),
        editor: editor_state,
    });

    if let Some(obs_driver) = obs_driver.as_ref() {
        driver_setup::register_obs_driver(
            obs_driver,
            &router,
            &control_db,
            &led_tx,
            &api_state,
            &tray_handler,
        )
        .await;
    }

    // Register the Windows audio driver (no-op on non-Windows, gated on config).
    driver_setup::register_winaudio_driver(&config, &router, &feedback_tx, &led_tx).await;

    // Register the Windows media transport driver (play/pause/next/previous).
    driver_setup::register_winmedia_driver(&config, &router, &feedback_tx, &control_db).await;

    // `feedback_tx` is kept alive for late driver registration on profile
    // switches; receiver lives in the main loop until shutdown.
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

    // BUG-008 FIX: Wait for configurable delay before initial refresh
    driver_setup::apply_startup_refresh(&config, &router, &xtouch).await;

    // Initialize gamepad if enabled
    let mut gamepad_mapper = if let Some(gamepad_config) = &config.gamepad {
        match input::gamepad::init(gamepad_config, router.clone()).await {
            Ok(mapper) => mapper,
            Err(e) => {
                warn!(
                    "Gamepad initialization failed: {}. Continuing without gamepad.",
                    e
                );
                None
            },
        }
    } else {
        None
    };

    info!("Ready to process MIDI events!");

    // Populate the tray's profile submenu now that everything is up.
    publish_profiles_list(&tray_update_tx, &profile_store);

    // Spawn stdin reader on a blocking thread (stdin is synchronous)
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
    tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if stdin_tx.send(line).is_err() {
                        break; // Channel closed, app shutting down
                    }
                },
                Err(e) => {
                    debug!("Stdin read error (REPL stopping): {}", e);
                    break;
                },
            }
        }
    });

    // Main event loop
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            // Apply fader setpoints (from FaderSetpoint async tasks)
            Some(cmd) = setpoint_apply_rx.recv() => {
                let setpoint = router.get_fader_setpoint();
                if setpoint.is_epoch_current(cmd.channel, cmd.epoch) {
                    debug!("Applying setpoint: ch={} value={} epoch={}", cmd.channel, cmd.value14, cmd.epoch);
                    let fader_num = cmd.channel - 1;
                    if let Err(_e) = xtouch.set_fader(fader_num, cmd.value14).await {
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
                router.on_midi_from_xtouch(&event.raw_data).await;

                // Check if page changed and display needs update
                if router.check_and_clear_display_update().await {
                    debug!("Updating display after page change...");
                    display::flush_pending_midi(&router, &xtouch, "page refresh").await;
                    display::update_xtouch_display(&router, &xtouch).await;
                    let active_page_name = router.get_active_page_name().await;
                    debug!("Display updated for page: {}", active_page_name);
                }
            }

            // Handle out-of-band display refresh requests (editor API page
            // change, REPL `page` command, etc.). The X-Touch input arm
            // already flushes inline after handling its own page navigation.
            _ = router.display_refresh_notify.notified() => {
                if router.check_and_clear_display_update().await {
                    debug!("Out-of-band page refresh: flushing display");
                    display::flush_pending_midi(&router, &xtouch, "page refresh").await;
                    display::update_xtouch_display(&router, &xtouch).await;
                }
            }

            // Handle feedback from applications -> X-Touch
            Some((app_name, feedback_data)) = feedback_rx.recv() => {
                handle_app_feedback(&router, &xtouch, &activity_tracker, &app_name, &feedback_data).await;
            }

            // Handle config reload
            Some(new_config) = config_watcher.next_config() => {
                let deps = ReloadDeps {
                    feedback_tx: &feedback_tx,
                    tray_handler: &tray_handler,
                    obs_driver: obs_driver.as_ref(),
                    control_db: &control_db,
                    led_tx: &led_tx,
                };
                handle_config_reload(
                    &router,
                    &xtouch,
                    new_config,
                    &mut gamepad_mapper,
                    &api_state,
                    &deps,
                ).await;
                // Refresh the tray profile checkmark for both tray-initiated and external switches.
                publish_profiles_list(&tray_update_tx, &profile_store);
                // Best-effort: notify editor live subscribers.
                let _ = live_tx.send(crate::event_bus::LiveEvent::ConfigReloaded {
                    ts: crate::event_bus::now_ms(),
                });
            }

            // Periodic state snapshot save (every 5 seconds, debounced by persistence actor)
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                if let Err(e) = router.save_state_snapshot().await {
                    warn!("Failed to save state snapshot: {}", e);
                }
            }

            // Handle tray commands
            Some(cmd) = tray_cmd_rx.recv() => {
                if handle_tray_command(&router, &profile_store, cmd).await {
                    break;
                }
            }

            // Handle stdin commands (REPL)
            Some(line) = stdin_rx.recv() => {
                if crate::cli::process_command(&line, router.get_state_actor()).await {
                    info!("Exit requested from REPL");
                    break;
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
    handler_task.abort();
    let _ = live_tx.send(crate::event_bus::LiveEvent::Connection {
        target: "xtouch".into(),
        status: crate::event_bus::ConnectionStatus::Down,
        detail: Some("shutdown".into()),
        ts: crate::event_bus::now_ms(),
    });
    shutdown_cleanup(&router, &xtouch).await
}

/// Build the editor state with live bus, OBS picker source, and action catalogs.
///
/// Catalogs are snapshotted at startup so the `/api/drivers/:name/actions`
/// endpoint can answer without locking the live driver registry.
fn build_editor_state(
    profile_store: Arc<crate::config::profiles::ProfileStore>,
    live_tx: crate::event_bus::LiveEventTx,
    obs_driver: Option<Arc<ObsDriver>>,
    catalogs: std::collections::HashMap<String, Vec<crate::api_editor::ActionDescriptor>>,
    fader_setpoint: Option<crate::api_editor::FaderSetpointReader>,
    active_page_reader: Option<crate::api_editor::ActivePageReader>,
    active_page_setter: Option<crate::api_editor::ActivePageSetter>,
) -> Arc<crate::api_editor::EditorState> {
    let obs_picker: Option<crate::api_editor::ObsPickerSourceArc> = obs_driver
        .as_ref()
        .map(|d| -> crate::api_editor::ObsPickerSourceArc { d.clone() });

    Arc::new(crate::api_editor::EditorState {
        profiles: profile_store,
        live_tx: Some(live_tx),
        obs: obs_picker,
        drivers: Arc::new(catalogs),
        fader_setpoint,
        active_page_reader,
        active_page_setter,
    })
}

/// Handle a tray command. Returns `true` if shutdown was requested.
async fn handle_tray_command(
    router: &Arc<Router>,
    profile_store: &Arc<crate::config::profiles::ProfileStore>,
    cmd: crate::tray::TrayCommand,
) -> bool {
    debug!("Tray command received: {:?}", cmd);
    match cmd {
        crate::tray::TrayCommand::ConnectObs => {
            debug!("Attempting to reconnect OBS from tray command...");
            if let Some(obs_driver) = router.get_driver(crate::state::AppKey::Obs.as_str()).await {
                if let Err(e) = obs_driver.sync().await {
                    warn!("Failed to reconnect OBS: {}", e);
                } else {
                    debug!("OBS sync initiated");
                }
            }
            false
        },
        crate::tray::TrayCommand::RecheckAll => {
            debug!("Rechecking all drivers from tray command...");
            for driver_name in router.list_drivers().await {
                if let Some(driver) = router.get_driver(&driver_name).await {
                    if let Err(e) = driver.sync().await {
                        warn!("Failed to sync driver {}: {}", driver_name, e);
                    }
                }
            }
            false
        },
        crate::tray::TrayCommand::SwitchProfile(name) => {
            info!("Switching to profile '{}' from tray", name);
            // The mirror to watched config.yaml triggers the ConfigWatcher,
            // which feeds `handle_config_reload` on the next loop iteration —
            // that's where drivers re-sync and the tray menu's checkmark gets
            // refreshed, so we don't republish here.
            if let Err(e) = profile_store.set_active(&name) {
                warn!("Failed to activate profile '{}': {}", name, e);
            }
            false
        },
        crate::tray::TrayCommand::Shutdown => {
            info!("Shutdown requested from tray");
            true
        },
    }
}

/// Unregister any driver the new config no longer references. Stops
/// background work (OBS reconnection loop, MIDI bridge readers, WinAudio
/// COM thread) so dropped profiles don't keep polling.
async fn prune_unused_drivers(router: &Arc<Router>, new_config: &AppConfig) {
    router
        .unregister_drivers_not_in(&new_config.referenced_apps())
        .await;
}

/// Publish the current profiles list + active profile to the tray UI.
/// Best-effort: silently drops if the tray channel is full or disconnected.
fn publish_profiles_list(
    tray_update_tx: &crossbeam::channel::Sender<crate::tray::TrayUpdate>,
    profile_store: &crate::config::profiles::ProfileStore,
) {
    let profiles = match profile_store.list() {
        Ok(metas) => metas.into_iter().map(|m| m.name).collect(),
        Err(e) => {
            warn!("Failed to list profiles for tray: {}", e);
            Vec::new()
        },
    };
    let active = profile_store.active().ok();
    let _ = tray_update_tx.try_send(crate::tray::TrayUpdate::ProfilesList { profiles, active });
}

/// Perform shutdown cleanup: stop drivers, save state, reset hardware.
async fn shutdown_cleanup(router: &Arc<Router>, xtouch: &Arc<XTouchDriver>) -> Result<()> {
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

    Ok(())
}

/// Handle feedback from an application, forwarding to X-Touch when appropriate.
async fn handle_app_feedback(
    router: &Arc<Router>,
    xtouch: &Arc<XTouchDriver>,
    activity_tracker: &Arc<crate::tray::ActivityTracker>,
    app_name: &str,
    feedback_data: &[u8],
) {
    // BUG-006 FIX: Capture epoch IMMEDIATELY on receive, before any async operations
    let captured_epoch = router.get_page_epoch();

    // Record activity from application
    activity_tracker.record(app_name, crate::tray::ActivityDirection::Inbound);

    // BUG-002 FIX: Activate squelch BEFORE state update to prevent race condition
    let pb_info = extract_pitchbend_from_feedback(feedback_data);
    if pb_info.is_some() {
        xtouch.activate_squelch(120);
        debug!("Squelch activated early for PitchBend feedback");
    }

    // BUG-006 FIX: Re-verify epoch is still current before state update
    if !router.is_epoch_current(captured_epoch) {
        trace!(
            "Epoch changed ({} -> current), discarding stale feedback from '{}'",
            captured_epoch,
            app_name
        );
        return;
    }

    // ALWAYS store state from all apps (needed for page refresh to restore values)
    router
        .on_midi_from_app(app_name, feedback_data, app_name)
        .await;

    // BUG-006 FIX: Check epoch again before forwarding to X-Touch
    if !router.is_epoch_current(captured_epoch) {
        trace!(
            "Epoch changed before X-Touch forward, discarding feedback from '{}'",
            app_name
        );
        return;
    }

    // Conditionally forward to X-Touch (only if app mapped on active page)
    let Some(transformed) = router.process_feedback(app_name, feedback_data).await else {
        return;
    };

    // BUG-006 FIX: Final epoch check after async process_feedback
    if !router.is_epoch_current(captured_epoch) {
        trace!("Epoch changed during process_feedback, not forwarding to X-Touch");
        return;
    }

    debug!("Forwarding feedback to X-Touch: {:02X?}", transformed);

    if let Some((channel, value14)) = pb_info {
        debug!(
            "Using set_fader for feedback: ch={} value={}",
            channel, value14
        );
        if let Err(e) = xtouch.set_fader(channel, value14).await {
            warn!("Failed to set fader from feedback: {}", e);
        } else {
            activity_tracker.record("xtouch", crate::tray::ActivityDirection::Outbound);
        }
    } else if let Err(e) = xtouch.send_raw(&transformed).await {
        warn!("Failed to send feedback to X-Touch: {}", e);
    } else {
        activity_tracker.record("xtouch", crate::tray::ActivityDirection::Outbound);
    }
}

/// Bundle of references handed to `handle_config_reload` so it can
/// register or re-register drivers introduced by a profile swap.
struct ReloadDeps<'a> {
    feedback_tx: &'a mpsc::Sender<(String, Vec<u8>)>,
    tray_handler: &'a Arc<crate::tray::TrayMessageHandler>,
    /// `None` when the initial profile had no `obs:` block. In that case
    /// we cannot late-register OBS on a profile switch; restart required.
    obs_driver: Option<&'a Arc<ObsDriver>>,
    control_db: &'a Arc<crate::control_mapping::ControlMappingDB>,
    led_tx: &'a mpsc::Sender<Vec<u8>>,
}

/// Handle configuration file reload, updating display, gamepad, and API state.
async fn handle_config_reload(
    router: &Arc<Router>,
    xtouch: &Arc<XTouchDriver>,
    new_config: AppConfig,
    gamepad_mapper: &mut Option<input::gamepad::GamepadMapper>,
    api_state: &Arc<api::ApiState>,
    deps: &ReloadDeps<'_>,
) {
    info!("Configuration file changed, reloading...");

    // Extract gamepad config before moving new_config into update_config
    let new_gamepad_config = new_config.gamepad.clone();

    // Register profile-introduced drivers BEFORE update_config so the
    // post-swap refresh dispatches to them. Existing ones are skipped.
    driver_setup::register_midi_bridge_drivers(
        &new_config,
        router,
        deps.feedback_tx,
        deps.tray_handler,
    )
    .await;
    driver_setup::register_winaudio_driver(&new_config, router, deps.feedback_tx, deps.led_tx)
        .await;
    driver_setup::register_winmedia_driver(&new_config, router, deps.feedback_tx, deps.control_db)
        .await;
    if let Some(obs_driver) = deps.obs_driver {
        if new_config.references_app(crate::state::AppKey::Obs.as_str()) {
            driver_setup::register_obs_driver(
                obs_driver,
                router,
                deps.control_db,
                deps.led_tx,
                api_state,
                deps.tray_handler,
            )
            .await;
        }
    }

    // Stop background tasks for drivers the new profile no longer uses.
    // Done before update_config so the post-swap refresh skips them.
    prune_unused_drivers(router, &new_config).await;

    match router.update_config(new_config).await {
        Ok(()) => {
            info!("Configuration reloaded successfully");

            // Send pending MIDI messages (fader positions, button states)
            display::flush_pending_midi(router, xtouch, "config reload").await;

            // Update display for the (potentially new) active page
            display::update_xtouch_display(router, xtouch).await;

            // Shut down old gamepad mapper before creating new one
            // (must use explicit shutdown(), not Drop, to ensure the blocking thread stops)
            if let Some(old) = gamepad_mapper.take() {
                if let Err(e) = old.shutdown().await {
                    warn!("Failed to shut down old gamepad mapper: {}", e);
                }
            }

            if let Some(ref gp_config) = new_gamepad_config {
                match input::gamepad::init(gp_config, router.clone()).await {
                    Ok(Some(new_mapper)) => {
                        *gamepad_mapper = Some(new_mapper);
                        info!("Gamepad subsystem reloaded");
                    },
                    Ok(None) => {
                        debug!("Gamepad disabled in config");
                    },
                    Err(e) => {
                        warn!("Gamepad init failed after config reload: {}", e);
                    },
                }
            } else {
                debug!("Gamepad not configured, skipping gamepad init");
            }

            // Update API state gamepad slots
            *api_state.gamepad_slots.write() =
                helpers::build_gamepad_slot_infos_from_config(&new_gamepad_config);

            // Clear the display_needs_update flag (we just handled it)
            router.check_and_clear_display_update().await;
        },
        Err(e) => {
            warn!("Failed to reload config (keeping old config): {}", e);
        },
    }
}
