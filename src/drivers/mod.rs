//! Application drivers (OBS, QLC+, Voicemeeter)

use async_trait::async_trait;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Execution context passed to drivers for accessing router state and config
#[derive(Clone)]
pub struct ExecutionContext {
    /// Application configuration
    pub config: Arc<RwLock<crate::config::AppConfig>>,
    /// Active page name
    pub active_page: Option<String>,
    /// Control value (for encoder/analog inputs)
    pub value: Option<serde_json::Value>,
}

/// Driver trait - all application integrations implement this
/// 
/// Note: All methods take &self (not &mut self) to support Arc<dyn Driver>.
/// Drivers should use interior mutability (RwLock, Mutex, etc.) for mutable state.
#[async_trait]
pub trait Driver: Send + Sync {
    /// Get the driver name (e.g., "console", "obs", "voicemeeter")
    fn name(&self) -> &str;
    
    /// Initialize the driver (connect to application, open ports, etc.)
    /// Uses interior mutability - implement with RwLock/Mutex for state
    async fn init(&self, ctx: ExecutionContext) -> Result<()>;
    
    /// Execute an action with parameters
    /// 
    /// # Arguments
    /// * `action` - The action name (e.g., "scene", "mute", "fader")
    /// * `params` - JSON parameters from config
    /// * `ctx` - Execution context for accessing router state
    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()>;
    
    /// Sync driver state (called after config reload)
    async fn sync(&self) -> Result<()>;
    
    /// Shutdown the driver gracefully
    async fn shutdown(&self) -> Result<()>;
}

pub mod console;
pub mod qlc;
pub mod midibridge;
pub mod obs;

// Re-export commonly used drivers
pub use console::ConsoleDriver;
pub use qlc::QlcDriver;
pub use midibridge::MidiBridgeDriver;
pub use obs::ObsDriver;

// Suppress unused warnings temporarily during Phase 5 development
#[allow(unused_imports)]
use {ConsoleDriver as _, QlcDriver as _, MidiBridgeDriver as _, ObsDriver as _};
