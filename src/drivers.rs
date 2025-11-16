//! Application drivers (OBS, QLC+, Voicemeeter)

use async_trait::async_trait;
use anyhow::Result;
use serde_json::Value;

#[async_trait]
pub trait Driver: Send + Sync {
    fn name(&self) -> &str;
    async fn init(&mut self) -> Result<()>;
    async fn execute(&self, action: &str, params: Vec<Value>) -> Result<()>;
    async fn sync(&self) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}
