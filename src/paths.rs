//! Application path management for portable and installed modes.
//!
//! This module handles the detection and resolution of application paths
//! to support both portable mode (files next to executable) and installed
//! mode (files in %APPDATA%\XTouch GW).
//!
//! ## Mode Detection
//!
//! - **Portable mode**: If a `.portable` marker file exists next to the
//!   executable, all data files are stored in the same directory. This
//!   requires the directory to be writable (not `C:\Program Files`).
//! - **Installed mode** (default): Data is stored in `%APPDATA%\XTouch GW`
//!   (or equivalent on other platforms).

use anyhow::Context;
use std::path::PathBuf;
use tracing::{debug, info};

/// Application name used for directories in installed mode
const APP_NAME: &str = "XTouch GW";

/// Application paths for config, state, and logs.
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Path to the configuration file
    pub config: PathBuf,
    /// Path to the state directory (sled database)
    pub state_dir: PathBuf,
    /// Path to the logs directory
    pub logs_dir: PathBuf,
    /// Whether running in portable mode (config next to exe)
    pub is_portable: bool,
}

impl AppPaths {
    /// Detect the appropriate paths based on environment.
    ///
    /// **Debug mode**: If `config.yaml` exists in the current working directory
    /// (typical when running with `cargo run`), use that directory. This makes
    /// development easier by using the project's config.yaml directly.
    ///
    /// **Portable mode**: If a `.portable` marker file exists next to the
    /// executable, all data files are stored in the same directory. This is
    /// explicit opt-in to avoid accidentally using portable mode in
    /// non-writable locations like `C:\Program Files`.
    ///
    /// **Installed mode** (default): Data is stored in `%APPDATA%\XTouch GW`.
    ///
    /// Note: This is called before logging is initialized, so we use eprintln
    /// for early diagnostic output.
    pub fn detect() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        #[cfg(debug_assertions)]
        eprintln!("[paths] Executable directory: {}", exe_dir.display());

        // In debug builds, check if config.yaml exists in current working directory
        // This enables seamless development with `cargo run`
        #[cfg(debug_assertions)]
        {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cwd_config = cwd.join("config.yaml");
            if cwd_config.exists() {
                eprintln!(
                    "[paths] Running in DEV mode (config.yaml found in cwd: {})",
                    cwd.display()
                );
                return Self {
                    config: cwd_config,
                    state_dir: cwd.join(".state"),
                    logs_dir: cwd.join("logs"),
                    is_portable: true, // Treat dev mode like portable
                };
            }
        }

        // Check for explicit portable mode marker file
        // This avoids accidentally triggering portable mode in Program Files
        let portable_marker = exe_dir.join(".portable");

        if portable_marker.exists() {
            #[cfg(debug_assertions)]
            eprintln!("[paths] Running in PORTABLE mode (.portable marker found)");
            Self {
                config: exe_dir.join("config.yaml"),
                state_dir: exe_dir.join(".state"),
                logs_dir: exe_dir.join("logs"),
                is_portable: true,
            }
        } else {
            // Installed mode - use %APPDATA% on Windows, ~/.local/share on Linux
            let data_dir = dirs::data_dir();
            #[cfg(debug_assertions)]
            eprintln!("[paths] dirs::data_dir() = {:?}", data_dir);

            let app_data = data_dir
                .unwrap_or_else(|| {
                    eprintln!(
                        "[paths] WARNING: dirs::data_dir() returned None, falling back to exe dir"
                    );
                    exe_dir.clone()
                })
                .join(APP_NAME);

            #[cfg(debug_assertions)]
            eprintln!(
                "[paths] Running in INSTALLED mode (data dir: {})",
                app_data.display()
            );

            Self {
                config: app_data.join("config.yaml"),
                state_dir: app_data.join("state"),
                logs_dir: app_data.join("logs"),
                is_portable: false,
            }
        }
    }

    /// Get the base directory (for displaying in logs)
    pub fn base_dir(&self) -> PathBuf {
        self.config
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Ensure all required directories exist.
    ///
    /// In installed mode, also copies `config.example.yaml` to the config
    /// location if the config file doesn't exist.
    pub fn ensure_directories(&self) -> anyhow::Result<()> {
        // Create state directory
        if !self.state_dir.exists() {
            debug!("Creating state directory: {}", self.state_dir.display());
            std::fs::create_dir_all(&self.state_dir)?;
        }

        // Create logs directory
        if !self.logs_dir.exists() {
            debug!("Creating logs directory: {}", self.logs_dir.display());
            std::fs::create_dir_all(&self.logs_dir)?;
        }

        // In installed mode, ensure parent config directory exists
        if !self.is_portable {
            if let Some(config_parent) = self.config.parent() {
                if !config_parent.exists() {
                    debug!("Creating config directory: {}", config_parent.display());
                    std::fs::create_dir_all(config_parent)?;
                }
            }

            // Copy config.example.yaml if config doesn't exist
            if !self.config.exists() {
                self.copy_example_config()?;
            }
        }

        Ok(())
    }

    /// Copy the config file to the AppData location.
    ///
    /// Looks for config.yaml or config.example.yaml next to the executable.
    /// The installer places config.yaml in Program Files, which we copy to
    /// AppData for the user to customize.
    fn copy_example_config(&self) -> anyhow::Result<()> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        // First, check for config.yaml next to exe (installed by installer)
        let installed_config = exe_dir.join("config.yaml");
        if installed_config.exists() {
            info!("Copying installed config to {}", self.config.display());
            std::fs::copy(&installed_config, &self.config).with_context(|| {
                format!(
                    "Failed to copy config from {} to {}",
                    installed_config.display(),
                    self.config.display()
                )
            })?;
            return Ok(());
        }

        // Then check for config.example.yaml
        let example_config = exe_dir.join("config.example.yaml");
        if example_config.exists() {
            info!("Copying example config to {}", self.config.display());
            std::fs::copy(&example_config, &self.config).with_context(|| {
                format!(
                    "Failed to copy example config from {} to {}",
                    example_config.display(),
                    self.config.display()
                )
            })?;
            return Ok(());
        }

        // Also check current working directory
        let cwd_example = PathBuf::from("config.example.yaml");
        if cwd_example.exists() {
            info!(
                "Copying example config from cwd to {}",
                self.config.display()
            );
            std::fs::copy(&cwd_example, &self.config).with_context(|| {
                format!(
                    "Failed to copy example config from cwd to {}",
                    self.config.display()
                )
            })?;
            return Ok(());
        }

        info!("No config found, please create {}", self.config.display());
        Ok(())
    }

    /// Get the sled database path (within state_dir)
    pub fn sled_db_path(&self) -> PathBuf {
        self.state_dir.join("sled")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_paths_structure() {
        // Just verify the struct can be created
        let paths = AppPaths {
            config: PathBuf::from("test/config.yaml"),
            state_dir: PathBuf::from("test/.state"),
            logs_dir: PathBuf::from("test/logs"),
            is_portable: true,
        };

        assert!(paths.is_portable);
        assert_eq!(paths.config, PathBuf::from("test/config.yaml"));
    }
}
