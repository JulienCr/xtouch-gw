//! Configuration file watcher for hot-reload support

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn, error};

use super::AppConfig;

/// Config watcher that monitors file changes and sends reload notifications
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<AppConfig>,
}

impl ConfigWatcher {
    /// Create a new config watcher for the specified file
    pub async fn new(config_path: String) -> Result<(Self, Arc<AppConfig>)> {
        let (tx, rx) = mpsc::channel(10);
        
        // Load initial config
        let initial_config = AppConfig::load(&config_path).await
            .context("Failed to load initial config")?;
        let initial_config = Arc::new(initial_config);
        
        let config_path_clone = config_path.clone();
        
        // Capture the Tokio runtime handle BEFORE creating the watcher
        // (notify callbacks run on their own OS thread, not in Tokio context)
        let runtime_handle = tokio::runtime::Handle::current();
        
        // Create file watcher
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // Only reload on modify events
                    if matches!(event.kind, EventKind::Modify(_)) {
                        debug!("Config file modified: {:?}", event.paths);
                        
                        // Clone path for async block
                        let config_path = config_path_clone.clone();
                        let tx = tx.clone();
                        
                        // Use the captured runtime handle to spawn async task
                        runtime_handle.spawn(async move {
                            // Debounce: wait a bit for file writes to complete
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            
                            match AppConfig::load(&config_path).await {
                                Ok(new_config) => {
                                    info!("Configuration reloaded successfully");
                                    if let Err(e) = tx.send(new_config).await {
                                        error!("Failed to send config update: {}", e);
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to reload config (keeping old config): {}", e);
                                }
                            }
                        });
                    }
                }
                Err(e) => {
                    error!("Watch error: {}", e);
                }
            }
        })?;
        
        // Watch the config file
        watcher.watch(Path::new(&config_path), RecursiveMode::NonRecursive)
            .with_context(|| format!("Failed to watch config file: {}", config_path))?;
        
        info!("Config file watcher started for: {}", config_path);
        
        Ok((
            Self {
                _watcher: watcher,
                rx,
            },
            initial_config,
        ))
    }
    
    /// Wait for the next config update
    /// Returns None if the watcher has been closed
    pub async fn next_config(&mut self) -> Option<AppConfig> {
        self.rx.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_config_watcher_basic() -> Result<()> {
        // Create a temporary config file
        let temp_dir = TempDir::new()?;
        let config_path = temp_dir.path().join("test-config.yaml");
        
        let initial_config = r#"
midi:
  input_port: "test-in"
  output_port: "test-out"

pages:
  - name: "Test Page"
"#;
        
        fs::write(&config_path, initial_config)?;
        
        // Create watcher
        let (mut watcher, config) = ConfigWatcher::new(config_path.to_string_lossy().to_string()).await?;
        
        assert_eq!(config.midi.input_port, "test-in");
        assert_eq!(config.pages[0].name, "Test Page");
        
        // Modify the config file
        let modified_config = r#"
midi:
  input_port: "test-in-modified"
  output_port: "test-out"

pages:
  - name: "Modified Page"
"#;
        
        tokio::time::sleep(Duration::from_millis(100)).await;
        fs::write(&config_path, modified_config)?;
        
        // Wait for reload (with timeout)
        let new_config = tokio::time::timeout(
            Duration::from_secs(2),
            watcher.next_config()
        ).await?;
        
        if let Some(new_config) = new_config {
            assert_eq!(new_config.midi.input_port, "test-in-modified");
            assert_eq!(new_config.pages[0].name, "Modified Page");
        }
        
        Ok(())
    }
}

