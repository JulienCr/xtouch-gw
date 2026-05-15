//! Gamepad input support using hybrid XInput and gilrs (WGI) backends
//!
//! Provides gamepad input integration with hot-plug support, analog processing,
//! and router integration. Supports both XInput controllers (Xbox) and non-XInput
//! controllers (FaceOff, etc.) simultaneously.

/// Default gamepad slot identifier used when a control_id has no slot prefix
/// or the slot index is unknown. All single-gamepad / legacy paths resolve to
/// this slot.
pub const DEFAULT_GAMEPAD_SLOT: &str = "gamepad1";

/// Prefix used for every gamepad control_id (e.g. `"gamepad1.btn.a"`). Slot
/// identifiers are always `"{GAMEPAD_PREFIX}{n}"` where `n >= 1`.
pub const GAMEPAD_PREFIX: &str = "gamepad";

/// Extract the gamepad slot prefix from a control_id.
///
/// For a control_id like `"gamepad1.axis.lx"` this returns `"gamepad1"`. For
/// inputs that do not start with [`GAMEPAD_PREFIX`] (e.g. an empty string,
/// `"foo.bar"`), this still returns the first dot-separated segment without
/// validating it — callers that need to discriminate gamepad vs non-gamepad
/// controls should test `result.starts_with(GAMEPAD_PREFIX)` after.
///
/// Returns [`DEFAULT_GAMEPAD_SLOT`] when the control_id has no first segment
/// (empty input or starts with `'.'`).
///
/// This helper is allocation-free: it returns a borrow into `control_id`
/// (except for the static fallback).
pub fn extract_gamepad_slot(control_id: &str) -> &str {
    let first = control_id.split('.').next().unwrap_or(DEFAULT_GAMEPAD_SLOT);
    if first.is_empty() {
        DEFAULT_GAMEPAD_SLOT
    } else {
        first
    }
}

pub mod analog;
pub mod axis;
pub mod buttons;
pub mod diagnostics;
pub mod hybrid_id;
pub mod hybrid_provider;
pub mod mapper;
pub mod normalize;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_slot_with_full_control_id() {
        assert_eq!(extract_gamepad_slot("gamepad1.axis.lx"), "gamepad1");
        assert_eq!(extract_gamepad_slot("gamepad2.btn.a"), "gamepad2");
    }

    #[test]
    fn extract_slot_with_bare_slot() {
        assert_eq!(extract_gamepad_slot("gamepad2"), "gamepad2");
    }

    #[test]
    fn extract_slot_empty_returns_default() {
        assert_eq!(extract_gamepad_slot(""), DEFAULT_GAMEPAD_SLOT);
    }

    #[test]
    fn extract_slot_non_gamepad_returns_first_segment() {
        // Helper does not validate gamepad-ness; callers must test
        // starts_with(GAMEPAD_PREFIX) themselves if they need to.
        assert_eq!(extract_gamepad_slot("foo.bar"), "foo");
    }

    #[test]
    fn constants_are_consistent() {
        assert!(DEFAULT_GAMEPAD_SLOT.starts_with(GAMEPAD_PREFIX));
    }
}
