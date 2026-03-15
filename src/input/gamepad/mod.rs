//! Gamepad input support using hybrid XInput and gilrs (WGI) backends
//!
//! Provides gamepad input integration with hot-plug support, analog processing,
//! and router integration. Supports both XInput controllers (Xbox) and non-XInput
//! controllers (FaceOff, etc.) simultaneously.

pub mod analog;
pub mod axis;
pub mod buttons;
pub mod diagnostics;
pub mod hybrid_id;
pub mod hybrid_provider;
pub mod mapper;
pub mod normalize;
pub mod provider; // Legacy provider (for reference)
pub mod slot;
pub mod stick_buffer;
pub mod visualizer;
pub mod visualizer_state;
pub mod xinput_convert;

use std::sync::Arc;
use tracing::{debug, info};

use crate::config::GamepadConfig;
use crate::router::Router;
use anyhow::{Context, Result};

pub use hybrid_provider::HybridGamepadProvider;
pub use mapper::GamepadMapper;
pub use visualizer::run_visualizer;

/// Initialize and attach gamepad input to router
///
/// # Arguments
/// * `config` - Gamepad configuration
/// * `router` - Router instance
///
/// # Returns
/// `Ok(None)` if gamepad is disabled in config, `Ok(Some(mapper))` on success,
/// `Err(...)` if initialization fails with gamepad enabled.
pub async fn init(config: &GamepadConfig, router: Arc<Router>) -> Result<Option<GamepadMapper>> {
    if !config.enabled {
        info!("Gamepad disabled in config");
        return Ok(None);
    }

    debug!("Initializing gamepad input...");

    // Build slot configurations
    let slot_configs = if let Some(gamepads) = &config.gamepads {
        // Multi-gamepad mode
        gamepads
            .iter()
            .map(|g| (g.product_match.clone(), g.analog.clone()))
            .collect()
    } else if let Some(hid) = &config.hid {
        // Legacy single-gamepad mode
        if let Some(pattern) = &hid.product_match {
            // Single slot with the pattern
            vec![(pattern.clone(), config.analog.clone())]
        } else {
            // No filtering, no slot manager (legacy mode with "gamepad" prefix)
            vec![]
        }
    } else {
        // No config, no filtering
        vec![]
    };

    // Start hybrid provider with slot configs
    let provider = HybridGamepadProvider::start(slot_configs)
        .await
        .context("Failed to initialize hybrid gamepad provider")?;
    let provider = Arc::new(provider);

    // Attach mapper
    let mapper = GamepadMapper::attach(provider, router, config)
        .await
        .context("Failed to attach gamepad mapper")?;

    debug!("Gamepad input initialized");

    Ok(Some(mapper))
}
