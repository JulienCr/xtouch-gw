//! Gamepad input support using GilRs
//!
//! Provides gamepad input integration with hot-plug support, analog processing,
//! and router integration.

pub mod analog;
pub mod provider;
pub mod mapper;

use anyhow::{Result, Context};
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::GamepadConfig;
use crate::router::Router;

pub use provider::{GilrsProvider, GamepadEvent};
pub use mapper::GamepadMapper;

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

    // Extract product match pattern
    let product_match = config.hid.as_ref()
        .and_then(|hid| hid.product_match.clone());

    // Start provider
    let provider = match GilrsProvider::start(product_match).await {
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
