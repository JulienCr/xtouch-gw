//! Driver registration and initialization helpers.
//!
//! Contains functions for registering MIDI bridge drivers, OBS driver,
//! loading the control database, and performing the startup refresh sequence.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::config::AppConfig;
use crate::control_mapping::ControlMappingDB;
use crate::drivers::midibridge::MidiBridgeDriver;
use crate::drivers::obs::ObsDriver;
use crate::drivers::Driver;
use crate::router::Router;
use crate::xtouch::XTouchDriver;
use crate::{api, control_mapping, obs_indicators};

/// Register all MIDI bridge drivers from config.
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
        Err(_) => match control_mapping::load_default_mappings() {
            Ok(db) => {
                info!(
                    "Loaded embedded control database ({} controls)",
                    db.mappings.len()
                );
                Arc::new(db)
            },
            Err(e) => {
                warn!("Failed to load control database: {}", e);
                Arc::new(ControlMappingDB {
                    mappings: Default::default(),
                    groups: Default::default(),
                })
            },
        },
    }
}

/// Register the OBS driver with indicator callback and tray status.
pub async fn register_obs_driver(
    obs_driver: &Arc<ObsDriver>,
    router: &Arc<Router>,
    control_db: &Arc<ControlMappingDB>,
    led_tx: &mpsc::UnboundedSender<Vec<u8>>,
    api_state: &Arc<api::ApiState>,
    tray_handler: &Arc<crate::tray::TrayMessageHandler>,
) {
    let indicator_callback = obs_indicators::build_indicator_callback(
        router.clone(),
        control_db.clone(),
        led_tx.clone(),
        Arc::clone(api_state),
    );

    obs_driver.subscribe_indicators(indicator_callback);
    debug!("Subscribed to OBS indicator signals");

    let status_callback = tray_handler.subscribe_driver("OBS".to_string());
    obs_driver.subscribe_connection_status(status_callback);

    match router
        .register_driver("obs".to_string(), obs_driver.clone())
        .await
    {
        Ok(_) => info!("Registered OBS driver"),
        Err(e) => warn!(
            "Failed to register OBS driver (will continue without it): {}",
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

    let pending_midi = router.take_pending_midi().await;
    for msg in pending_midi {
        trace!("  -> Sending initial MIDI: {:02X?}", msg);
        if let Err(e) = xtouch.send_raw(&msg).await {
            warn!("Failed to send initial refresh MIDI: {}", e);
        }
    }
    debug!("Initial state applied to X-Touch");
}
