//! Gamepad input support using GilRs
//!
//! Provides gamepad input integration with hot-plug support, analog processing,
//! and router integration.

pub mod analog;
pub mod provider;
pub mod mapper;
pub mod diagnostics;
pub mod slot;

// use anyhow::{Result, Context};
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::GamepadConfig;
use crate::router::Router;

pub use provider::GilrsProvider;
pub use mapper::GamepadMapper;
pub use diagnostics::print_gamepad_diagnostics;

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

    // Start provider with slot configs
    let provider = match GilrsProvider::start(slot_configs).await {
        Ok(p) => Arc::new(p),
        Err(e) => {
            warn!("Failed to initialize gamepad provider: {}. Continuing without gamepad.", e);
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
