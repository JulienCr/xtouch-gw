//! Configuration management for XTouch GW
//! 
//! Handles loading, parsing, and hot-reloading of YAML configuration files.

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;

/// Root configuration structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub midi: MidiConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obs: Option<ObsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtouch: Option<XTouchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paging: Option<PagingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gamepad: Option<GamepadConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages_global: Option<GlobalPageDefaults>,
    pub pages: Vec<PageConfig>,
}

/// MIDI port configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MidiConfig {
    pub input_port: String,
    pub output_port: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<MidiAppConfig>>,
}

/// App-specific MIDI port mapping
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MidiAppConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_port: Option<String>,
}

/// OBS WebSocket configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObsConfig {
    #[serde(default = "default_obs_host")]
    pub host: String,
    #[serde(default = "default_obs_port")]
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// X-Touch specific configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct XTouchConfig {
    #[serde(default = "default_xtouch_mode")]
    pub mode: XTouchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay: Option<OverlayConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_per_app: Option<HashMap<String, OverlayConfig>>,
}

/// X-Touch operation mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum XTouchMode {
    Mcu,
    Ctrl,
}

/// LCD overlay configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OverlayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<OverlayMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_bits: Option<CcBits>,
}

/// Overlay display mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlayMode {
    Percent,
    #[serde(rename = "7bit")]
    SevenBit,
    #[serde(rename = "8bit")]
    EightBit,
}

/// CC bit display mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum CcBits {
    #[serde(rename = "7bit")]
    SevenBit,
    #[serde(rename = "8bit")]
    EightBit,
}

/// Page navigation configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PagingConfig {
    #[serde(default = "default_paging_channel")]
    pub channel: u8,
    #[serde(default = "default_prev_note")]
    pub prev_note: u8,
    #[serde(default = "default_next_note")]
    pub next_note: u8,
}

/// Gamepad configuration (placeholder for now)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GamepadConfig {
    pub enabled: bool,
    // Additional fields will be added as needed
}

/// Global page defaults
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GlobalPageDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controls: Option<HashMap<String, ControlMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lcd: Option<LcdConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passthroughs: Option<Vec<PassthroughConfig>>,
}

/// Page configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PageConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controls: Option<HashMap<String, ControlMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lcd: Option<LcdConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passthrough: Option<PassthroughConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passthroughs: Option<Vec<PassthroughConfig>>,
}

/// Control mapping
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ControlMapping {
    pub app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiSpec>,
}

/// MIDI control specification
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MidiSpec {
    #[serde(rename = "type")]
    pub midi_type: MidiType,
    pub channel: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
}

/// MIDI message type
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MidiType {
    Cc,
    Note,
    Pb,
    Passthrough,
}

/// LCD configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LcdConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<LcdLabel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<LcdColor>>,
}

/// LCD label (string or structured)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LcdLabel {
    Simple(String),
    Structured {
        #[serde(skip_serializing_if = "Option::is_none")]
        upper: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        lower: Option<String>,
    },
}

/// LCD color (numeric or string)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LcdColor {
    Numeric(u32),
    Named(String),
}

/// MIDI passthrough configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PassthroughConfig {
    pub driver: String,
    pub to_port: String,
    pub from_port: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<MidiFilterConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformConfig>,
}

/// MIDI filter configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MidiFilterConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_notes: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_notes: Option<Vec<u8>>,
}

/// MIDI transform configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransformConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pb_to_note: Option<PbToNoteTransform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pb_to_cc: Option<PbToCcTransform>,
}

/// PitchBend to Note transform
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PbToNoteTransform {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
}

/// PitchBend to CC transform
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PbToCcTransform {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_channel: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_cc: Option<serde_json::Value>, // Can be number or hex string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_by_channel: Option<HashMap<u8, serde_json::Value>>,
}

impl AppConfig {
    /// Load configuration from file
    pub async fn load(path: &str) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read config file: {}", path))?;
        
        let config: AppConfig = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse YAML config: {}", path))?;
        
        Ok(config)
    }

    /// Save configuration to file
    pub async fn save(&self, path: &str) -> Result<()> {
        let yaml = serde_yaml::to_string(self)
            .context("Failed to serialize config to YAML")?;
        
        fs::write(path, yaml)
            .await
            .with_context(|| format!("Failed to write config file: {}", path))?;
        
        Ok(())
    }
}

// Default value functions
fn default_obs_host() -> String { "localhost".to_string() }
fn default_obs_port() -> u16 { 4455 }
fn default_xtouch_mode() -> XTouchMode { XTouchMode::Mcu }
fn default_true() -> bool { true }
fn default_paging_channel() -> u8 { 1 }
fn default_prev_note() -> u8 { 46 }
fn default_next_note() -> u8 { 47 }
