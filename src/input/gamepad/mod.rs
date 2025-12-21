//! Gamepad input support using hybrid XInput and gilrs (WGI) backends
//!
//! Provides gamepad input integration with hot-plug support, analog processing,
//! and router integration. Supports both XInput controllers (Xbox) and non-XInput
//! controllers (FaceOff, etc.) simultaneously.

pub mod analog;
pub mod hybrid_id;
pub mod hybrid_provider;
pub mod mapper;
pub mod diagnostics;
pub mod provider;  // Legacy provider (for reference)
pub mod slot;
pub mod xinput_convert;
pub mod visualizer;
pub mod visualizer_state;

// use anyhow::{Result, Context};
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::GamepadConfig;
use crate::router::Router;

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
/// Configured mapper instance, or None if initialization fails
pub async fn init(config: &GamepadConfig, router: Arc<Router>) -> Option<GamepadMapper> {
    if !config.enabled {
        return None;
    }

    info!("Initializing gamepad input...");

    // Build slot configurations
    let slot_configs = if let Some(gamepads) = &config.gamepads {
        // Multi-gamepad mode
        gamepads.iter()
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
    let provider = match HybridGamepadProvider::start(slot_configs).await {
        Ok(p) => Arc::new(p),
        Err(e) => {
            warn!("Failed to initialize hybrid gamepad provider: {}. Continuing without gamepad.", e);
            return None;
        }
    };

    // Attach mapper
    let mapper = match GamepadMapper::attach(provider, router, config).await {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to attach gamepad mapper: {}. Continuing without gamepad.", e);
            return None;
        }
    };

    info!("✅ Gamepad input initialized");

    Some(mapper)
}
