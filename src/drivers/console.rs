//! Console driver - logs all actions for testing and debugging

use crate::drivers::{Driver, ExecutionContext};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug, warn};

/// ConsoleDriver logs all driver actions to console/logs
/// 
/// This is useful for:
/// - Testing control mappings without real applications
/// - Debugging driver execution flow
/// - Validating parameter passing
/// - Development without hardware dependencies
pub struct ConsoleDriver {
    name: String,
    /// Track if driver is initialized
    initialized: Arc<RwLock<bool>>,
    /// Execution counter for debugging
    execution_count: Arc<RwLock<u64>>,
}

impl ConsoleDriver {
    /// Create a new ConsoleDriver with a given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            initialized: Arc::new(RwLock::new(false)),
            execution_count: Arc::new(RwLock::new(0)),
        }
    }
}

#[async_trait]
impl Driver for ConsoleDriver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&self, ctx: ExecutionContext) -> Result<()> {
        let config = ctx.config.read().await;
        info!(
            "🔌 ConsoleDriver '{}' initializing (config has {} pages)",
            self.name,
            config.pages.len()
        );
        drop(config);

        *self.initialized.write().await = true;
        *self.execution_count.write().await = 0;

        info!("✅ ConsoleDriver '{}' initialized", self.name);
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        // Check if initialized
        if !*self.initialized.read().await {
            warn!("⚠️  ConsoleDriver '{}' not initialized, skipping execution", self.name);
            return Ok(());
        }

        // Increment execution counter
        let mut count = self.execution_count.write().await;
        *count += 1;
        let exec_num = *count;
        drop(count);

        // Format parameters nicely
        let params_str = if params.is_empty() {
            "(no params)".to_string()
        } else {
            params
                .iter()
                .map(|p| format!("{}", p))
                .collect::<Vec<_>>()
                .join(", ")
        };

        // Get active page if available
        let page_info = ctx
            .active_page
            .map(|p| format!(" [page: {}]", p))
            .unwrap_or_default();

        info!(
            "🎮 [{}] Driver '{}' → {} ({}){} [exec #{}]",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            self.name,
            action,
            params_str,
            page_info,
            exec_num
        );

        // Add debug-level details
        debug!(
            driver = self.name,
            action = action,
            params = ?params,
            exec_count = exec_num,
            "ConsoleDriver execution"
        );

        Ok(())
    }

    async fn sync(&self) -> Result<()> {
        if *self.initialized.read().await {
            info!("🔄 ConsoleDriver '{}' syncing state", self.name);
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let was_initialized = *self.initialized.read().await;
        
        if was_initialized {
            let final_count = *self.execution_count.read().await;
            info!(
                "🛑 ConsoleDriver '{}' shutting down (executed {} actions)",
                self.name, final_count
            );
        }

        *self.initialized.write().await = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_test_context() -> ExecutionContext {
        let config = AppConfig {
            midi: crate::config::MidiConfig {
                input_port: "test_in".to_string(),
                output_port: "test_out".to_string(),
                apps: None,
            },
            obs: None,
            xtouch: None,
            paging: None,
            gamepad: None,
            pages_global: None,
            pages: vec![],
            tray: None,
        };

        ExecutionContext {
            config: Arc::new(RwLock::new(config)),
            active_page: Some("TestPage".to_string()),
            value: None,
            control_id: None,
            activity_tracker: None,
        }
    }

    #[tokio::test]
    async fn test_console_driver_lifecycle() {
        let driver = ConsoleDriver::new("test");
        let ctx = make_test_context();

        assert_eq!(driver.name(), "test");

        // Should not be initialized initially
        assert!(!*driver.initialized.read().await);

        // Initialize
        driver.init(ctx.clone()).await.unwrap();
        assert!(*driver.initialized.read().await);

        // Execute some actions
        driver
            .execute("action1", vec![Value::from("param1")], ctx.clone())
            .await
            .unwrap();
        
        driver
            .execute("action2", vec![Value::from(42)], ctx.clone())
            .await
            .unwrap();

        // Check execution count
        assert_eq!(*driver.execution_count.read().await, 2);

        // Sync
        driver.sync().await.unwrap();

        // Shutdown
        driver.shutdown().await.unwrap();
        assert!(!*driver.initialized.read().await);
    }

    #[tokio::test]
    async fn test_console_driver_execute_without_init() {
        let driver = ConsoleDriver::new("uninit_test");
        let ctx = make_test_context();

        // Should succeed but warn (not error)
        let result = driver
            .execute("test_action", vec![], ctx)
            .await;
        
        assert!(result.is_ok());
        assert_eq!(*driver.execution_count.read().await, 0);
    }

    #[tokio::test]
    async fn test_console_driver_multiple_executions() {
        let driver = ConsoleDriver::new("multi_test");
        let ctx = make_test_context();

        driver.init(ctx.clone()).await.unwrap();

        // Execute many actions
        for i in 0..10 {
            driver
                .execute(
                    "test_action",
                    vec![Value::from(i)],
                    ctx.clone(),
                )
                .await
                .unwrap();
        }

        assert_eq!(*driver.execution_count.read().await, 10);
    }
}

