//! Router module - Core orchestration of MIDI events and page management

use crate::config::AppConfig;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct Router {
    config: Arc<RwLock<AppConfig>>,
}

impl Router {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
        }
    }
}
