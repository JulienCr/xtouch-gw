//! Driver registration and initialization helpers.
//!
//! Contains functions for registering MIDI bridge drivers, OBS driver,
//! loading the control database, and performing the startup refresh sequence.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::AppConfig;
use crate::control_mapping::ControlMappingDB;
use crate::drivers::midibridge::MidiBridgeDriver;
use crate::drivers::obs::ObsDriver;
use crate::drivers::winaudio::WinAudioDriver;
use crate::drivers::winmedia::WinMediaDriver;
use crate::drivers::Driver;
use crate::router::Router;
use crate::xtouch::XTouchDriver;
use crate::{api, control_mapping, obs_indicators};

/// Register all MIDI bridge drivers from config.
///
/// Idempotent: skips drivers already registered with the router (e.g. on
/// profile reload). Lets us re-call after each config swap to pick up apps
/// introduced by the new profile without re-initializing existing ones.
pub async fn register_midi_bridge_drivers(
    config: &AppConfig,
    router: &Arc<Router>,
    feedback_tx: &mpsc::Sender<(String, Vec<u8>)>,
    tray_handler: &Arc<crate::tray::TrayMessageHandler>,
) {
    let Some(apps) = &config.midi.apps else {
        return;
    };

    for app_config in apps {
        if router.get_driver(&app_config.name).await.is_some() {
            debug!(
                "MIDI bridge driver '{}' already registered — skipping",
                app_config.name
            );
            continue;
        }

        let driver = Arc::new(MidiBridgeDriver::new(
            app_config.output_port.clone().unwrap_or_default(),
            app_config.input_port.clone().unwrap_or_default(),
            None,
            None,
            false,
        ));

        let feedback_tx_clone = feedback_tx.clone();
        let app_name = app_config.name.clone();
        driver.set_feedback_callback(Arc::new(move |data: &[u8]| {
            debug!("Feedback from {}: {:02X?}", app_name, data);
            if let Err(e) = feedback_tx_clone.try_send((app_name.clone(), data.to_vec())) {
                warn!("Failed to send feedback to channel: {}", e);
            }
        }));

        let status_callback = tray_handler.subscribe_driver(app_config.name.clone());
        driver.subscribe_connection_status(status_callback);

        match router
            .register_driver(app_config.name.clone(), driver)
            .await
        {
            Ok(_) => info!("Registered MIDI bridge driver for: {}", app_config.name),
            Err(e) => warn!(
                "Failed to register MIDI bridge driver for {} (will continue without it): {}",
                app_config.name, e
            ),
        }
    }
}

/// Load the control mapping database (external file or embedded fallback).
pub async fn load_control_database() -> Arc<ControlMappingDB> {
    match ControlMappingDB::load_from_csv("docs/xtouch-matching.csv").await {
        Ok(db) => {
            info!("Loaded control database ({} controls)", db.mappings.len());
            Arc::new(db)
        },
        Err(csv_err) => {
            debug!("CSV control database not available: {}", csv_err);
            match control_mapping::load_default_mappings() {
                Ok(db) => {
                    info!(
                        "Loaded embedded control database ({} controls)",
                        db.mappings.len()
                    );
                    Arc::new(db.clone())
                },
                Err(e) => {
                    error!(
                        "Failed to load control database (system will have no control mappings!): {}",
                        e
                    );
                    Arc::new(ControlMappingDB {
                        mappings: Default::default(),
                        groups: Default::default(),
                    })
                },
            }
        },
    }
}

/// Register the OBS driver with indicator callback and tray status.
///
/// Idempotent: if the OBS driver is already in the router (e.g. on profile
/// reload), this is a no-op. The driver's `init()` re-arms its shutdown
/// flag so re-registration after a previous unregister works correctly.
pub async fn register_obs_driver(
    obs_driver: &Arc<ObsDriver>,
    router: &Arc<Router>,
    control_db: &Arc<ControlMappingDB>,
    led_tx: &mpsc::Sender<Vec<u8>>,
    api_state: &Arc<api::ApiState>,
    tray_handler: &Arc<crate::tray::TrayMessageHandler>,
) {
    if router
        .get_driver(crate::state::AppKey::Obs.as_str())
        .await
        .is_some()
    {
        debug!("OBS driver already registered — skipping");
        return;
    }

    let indicator_callback = obs_indicators::build_indicator_callback(
        router.clone(),
        control_db.clone(),
        led_tx.clone(),
        Arc::clone(api_state),
    );

    obs_driver.subscribe_indicators(indicator_callback);
    debug!("Subscribed to OBS indicator signals");

    let status_callback =
        tray_handler.subscribe_driver(crate::state::AppKey::Obs.as_str().to_string());
    obs_driver.subscribe_connection_status(status_callback);

    match router
        .register_driver(
            crate::state::AppKey::Obs.as_str().to_string(),
            obs_driver.clone(),
        )
        .await
    {
        Ok(_) => info!("Registered OBS driver"),
        Err(e) => warn!(
            "Failed to register OBS driver (will continue without it): {}",
            e
        ),
    }
}

/// Register the Windows audio driver if `winaudio` is configured or any
/// page references the `winaudio` app.
///
/// The driver itself is cross-platform (no-op on non-Windows); registration
/// is unconditional once the config opts in, so YAML routing succeeds on
/// any platform.
///
/// `feedback_tx` is cloned inside the driver and used by its COM thread
/// consumer to inject volume-change feedback as if it came from a regular
/// MIDI app — this routes through the existing anti-echo / fader-setpoint
/// pipeline and keeps the X-Touch motorized fader in sync.
pub async fn register_winaudio_driver(
    config: &AppConfig,
    router: &Arc<Router>,
    feedback_tx: &mpsc::Sender<(String, Vec<u8>)>,
) {
    let referenced = config.pages.iter().any(|p| {
        p.controls
            .as_ref()
            .map(|m| m.values().any(|c| c.app == "winaudio"))
            .unwrap_or(false)
    }) || config
        .pages_global
        .as_ref()
        .and_then(|g| g.controls.as_ref())
        .map(|m| m.values().any(|c| c.app == "winaudio"))
        .unwrap_or(false);

    if !referenced && config.winaudio.is_none() {
        debug!("WinAudio driver not configured and unreferenced — skipping registration");
        return;
    }

    if router
        .get_driver(crate::drivers::winaudio::DRIVER_NAME)
        .await
        .is_some()
    {
        debug!("WinAudio driver already registered — skipping");
        return;
    }

    let winaudio_cfg = config
        .winaudio
        .clone()
        .unwrap_or_else(|| crate::config::WinAudioConfig {
            pinned_apps: Vec::new(),
        });

    let driver = Arc::new(WinAudioDriver::new(winaudio_cfg));
    driver.set_router(router.clone()).await;
    driver.set_feedback_sender(feedback_tx.clone()).await;

    match router
        .register_driver(crate::drivers::winaudio::DRIVER_NAME.to_string(), driver)
        .await
    {
        Ok(_) => info!("Registered WinAudio driver"),
        Err(e) => warn!(
            "Failed to register WinAudio driver (will continue without it): {}",
            e
        ),
    }
}

/// Register the WinMedia driver if any control mapping references it.
///
/// Idempotent: skips if already in the router or no page references
/// `app: "winmedia"`. The driver itself is cross-platform (no-op on
/// non-Windows) so YAML routing and editor UX work uniformly across
/// hosts.
///
/// Wires `feedback_tx` and the router up-front so the SMTC poller can
/// emit play-LED feedback through the unified router pipeline.
pub async fn register_winmedia_driver(
    config: &AppConfig,
    router: &Arc<Router>,
    feedback_tx: &mpsc::Sender<(String, Vec<u8>)>,
) {
    let referenced = config.pages.iter().any(|p| {
        p.controls
            .as_ref()
            .map(|m| {
                m.values()
                    .any(|c| c.app == crate::drivers::winmedia::DRIVER_NAME)
            })
            .unwrap_or(false)
    }) || config
        .pages_global
        .as_ref()
        .and_then(|g| g.controls.as_ref())
        .map(|m| {
            m.values()
                .any(|c| c.app == crate::drivers::winmedia::DRIVER_NAME)
        })
        .unwrap_or(false);

    if !referenced {
        debug!("WinMedia driver unreferenced — skipping registration");
        return;
    }

    if router
        .get_driver(crate::drivers::winmedia::DRIVER_NAME)
        .await
        .is_some()
    {
        debug!("WinMedia driver already registered — skipping");
        return;
    }

    let driver = Arc::new(WinMediaDriver::new());
    driver.set_router(router.clone()).await;
    driver.set_feedback_sender(feedback_tx.clone()).await;

    match router
        .register_driver(crate::drivers::winmedia::DRIVER_NAME.to_string(), driver)
        .await
    {
        Ok(_) => info!("Registered WinMedia driver"),
        Err(e) => warn!(
            "Failed to register WinMedia driver (will continue without it): {}",
            e
        ),
    }
}

/// Wait for startup delay, then apply initial state refresh to X-Touch.
pub async fn apply_startup_refresh(
    config: &AppConfig,
    router: &Arc<Router>,
    xtouch: &Arc<XTouchDriver>,
) {
    let startup_delay_ms = config
        .xtouch
        .as_ref()
        .map(|x| x.startup_refresh_delay_ms)
        .unwrap_or(500);

    if startup_delay_ms > 0 {
        debug!(
            "Waiting {}ms for drivers to sync before initial refresh (BUG-008 fix)...",
            startup_delay_ms
        );
        tokio::time::sleep(std::time::Duration::from_millis(startup_delay_ms)).await;
    }

    debug!("Applying initial state to X-Touch (post-driver registration)...");
    router.refresh_page().await;

    crate::display::flush_pending_midi(router, xtouch, "initial refresh").await;
    debug!("Initial state applied to X-Touch");
}
