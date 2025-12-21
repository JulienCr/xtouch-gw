//! QLC+ Lighting Control Driver
//!
//! QLC+ is controlled via MIDI messages sent through the MIDI bridge.
//! This driver is mostly a stub - the actual control happens via MIDI passthrough
//! configured in the MIDI bridge driver.

use async_trait::async_trait;
use anyhow::Result;
use serde_json::Value;
use tracing::{info, debug};

use super::{Driver, ExecutionContext};

/// QLC+ lighting control driver
///
/// Note: QLC+ receives MIDI CC messages via the MIDI bridge.
/// This driver is primarily for logging and future direct control features.
pub struct QlcDriver;

impl QlcDriver {
    /// Create a new QLC+ driver
    pub fn new() -> Self {
        Self
    }
}

impl Default for QlcDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for QlcDriver {
    async fn init(&self, _ctx: ExecutionContext) -> Result<()> {
        info!("✅ QLC+ driver initialized (MIDI control via bridge)");
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, _ctx: ExecutionContext) -> Result<()> {
        debug!(
            action = action,
            params = ?params,
            "QLC+ driver execute (stub - actual control via MIDI bridge)"
        );
        Ok(())
    }

    async fn sync(&self) -> Result<()> {
        debug!("QLC+ driver sync (no-op)");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!("QLC+ driver shutdown");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn test_qlc_driver_lifecycle() {
        let driver = QlcDriver::new();
        // Create a minimal config for testing
        let _config = Arc::new(RwLock::new(crate::config::AppConfig {
            midi: crate::config::MidiConfig {
                input_port: "test".to_string(),
                output_port: "test".to_string(),
                apps: Some(vec![]),
            },
            xtouch: None,
            obs: None,
            paging: None,
            gamepad: None,
            pages_global: None,
            pages: vec![],
            tray: None,
        }));
        let ctx = ExecutionContext {
            value: None,
            control_id: None,
            activity_tracker: None,
        };

        // Test init
        assert!(driver.init(ctx.clone()).await.is_ok());

        // Test execute
        assert!(driver.execute("testAction", vec![], ctx.clone()).await.is_ok());

        // Test sync
        assert!(driver.sync().await.is_ok());

        // Test shutdown
        assert!(driver.shutdown().await.is_ok());
    }
}

