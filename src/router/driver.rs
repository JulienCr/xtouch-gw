//! Driver registration and lifecycle management

use crate::drivers::{Driver, ExecutionContext};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, warn};

impl super::Router {
    /// Create an execution context for driver calls
    pub(crate) async fn create_execution_context(&self) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value: None,
            control_id: None,
            activity_tracker: self.activity_tracker.clone(),
        }
    }

    /// Create an execution context with control information
    pub(crate) async fn create_execution_context_with_control(&self, control_id: String, value: Option<Value>) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value,
            control_id: Some(control_id),
            activity_tracker: self.activity_tracker.clone(),
        }
    }

    /// Register a driver by name (e.g., "voicemeeter", "qlc", "obs")
    ///
    /// The driver will be initialized immediately upon registration
    pub async fn register_driver(&self, name: String, driver: Arc<dyn Driver>) -> Result<()> {
        debug!("Registering driver '{}'...", name);

        // Create execution context
        let ctx = self.create_execution_context().await;

        // Initialize the driver
        if let Err(e) = driver.init(ctx).await {
            warn!("Failed to initialize driver '{}': {}", name, e);
            return Err(e);
        }

        // Store the driver
        let mut drivers = self.drivers.write().await;
        drivers.insert(name.clone(), driver);

        debug!("Driver '{}' registered and initialized", name);
        Ok(())
    }

    /// Get a driver by name
    pub async fn get_driver(&self, name: &str) -> Option<Arc<dyn Driver>> {
        let drivers = self.drivers.read().await;
        drivers.get(name).cloned()
    }

    /// List all registered driver names
    pub async fn list_drivers(&self) -> Vec<String> {
        let drivers = self.drivers.read().await;
        drivers.keys().cloned().collect()
    }

    /// Shutdown all registered drivers
    pub async fn shutdown_all_drivers(&self) -> Result<()> {
        debug!("Shutting down all drivers...");

        let drivers = self.drivers.read().await;
        let driver_list: Vec<_> = drivers
            .iter()
            .map(|(name, driver)| (name.clone(), driver.clone()))
            .collect();
        drop(drivers);

        let mut errors = Vec::new();
        for (name, driver) in driver_list {
            debug!("Shutting down driver '{}'...", name);
            if let Err(e) = driver.shutdown().await {
                warn!("Failed to shutdown driver '{}': {}", name, e);
                errors.push((name, e));
            } else {
                debug!("Driver '{}' shut down", name);
            }
        }

        if !errors.is_empty() {
            let error_msg = errors
                .iter()
                .map(|(n, e)| format!("{}: {}", n, e))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(anyhow!(
                "Failed to shutdown {} driver(s): {}",
                errors.len(),
                error_msg
            ));
        }

        // Clear the driver registry
        self.drivers.write().await.clear();
        debug!("All drivers shut down successfully");
        Ok(())
    }

    /// Handle a control event (resolve mapping and execute driver action)
    pub async fn handle_control(&self, control_id: &str, value: Option<Value>) -> Result<()> {
        let page = self
            .get_active_page()
            .await
            .ok_or_else(|| anyhow!("No active page"))?;

        // Look up the control mapping - check page-specific controls first, then global controls
        let config = self.config.read().await;
        let mapping = page
            .controls
            .as_ref()
            .and_then(|controls| controls.get(control_id))
            .or_else(|| {
                config
                    .pages_global
                    .as_ref()
                    .and_then(|pg| pg.controls.as_ref())
                    .and_then(|controls| controls.get(control_id))
            })
            .ok_or_else(|| anyhow!("No mapping for control '{}'", control_id))?;

        // Clone data we need before dropping config lock
        let app_name = mapping.app.clone();
        let action = mapping
            .action
            .clone()
            .ok_or_else(|| anyhow!("Control '{}' has no action defined", control_id))?;
        let params = mapping.params.clone().unwrap_or_default();

        // Drop config lock before async operations
        drop(config);

        // Get the driver
        let driver = self
            .get_driver(&app_name)
            .await
            .ok_or_else(|| anyhow!("Driver '{}' not registered", app_name))?;

        debug!(
            "Executing {}.{} for control '{}' (value: {:?})",
            app_name, action, control_id, value
        );

        // Create execution context with control information
        let ctx = self.create_execution_context_with_control(control_id.to_string(), value).await;

        // Execute the driver action
        driver.execute(&action, params, ctx).await?;

        Ok(())
    }
}

