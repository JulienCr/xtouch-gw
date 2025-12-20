//! Configuration file I/O
//!
//! Handles loading and saving config.yaml files with backup creation.

use crate::config::AppConfig;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Load configuration from YAML file
pub fn load_config(path: impl AsRef<Path>) -> Result<AppConfig> {
    let path = path.as_ref();
    let yaml = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let config: AppConfig = serde_yaml::from_str(&yaml)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

    Ok(config)
}

/// Save configuration to YAML file (creates backup first)
pub fn save_config(config: &AppConfig, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();

    // Create backup of original file if it exists
    if path.exists() {
        let backup_path = format!("{}.backup", path.display());
        if let Err(e) = fs::copy(path, &backup_path) {
            tracing::warn!("Failed to create backup file: {}", e);
        } else {
            tracing::info!("Created backup: {}", backup_path);
        }
    }

    // Serialize config to YAML
    let yaml = serde_yaml::to_string(config)
        .with_context(|| "Failed to serialize config to YAML")?;

    // Write to file
    fs::write(path, yaml)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;

    tracing::info!("Saved config to: {}", path.display());
    Ok(())
}
