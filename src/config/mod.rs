//! Configuration management for XTouch GW
//! 
//! Handles loading, parsing, and hot-reloading of YAML configuration files.

pub mod watcher;

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;

pub use watcher::ConfigWatcher;

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
    pub tray: Option<TrayConfig>,
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

/// Gamepad configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GamepadConfig {
    pub enabled: bool,
    #[serde(default = "default_gamepad_provider")]
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analog: Option<AnalogConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hid: Option<HidProviderConfig>,
}

/// Analog stick configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnalogConfig {
    #[serde(default = "default_pan_gain")]
    pub pan_gain: f32,
    #[serde(default = "default_zoom_gain")]
    pub zoom_gain: f32,
    #[serde(default = "default_deadzone")]
    pub deadzone: f32,
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    #[serde(default)]
    pub invert: HashMap<String, bool>,
}

/// HID provider configuration (for gilrs device matching)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HidProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_match: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_csv: Option<String>,
}

/// System tray UI configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_activity_duration")]
    pub activity_led_duration_ms: u64,

    #[serde(default = "default_poll_interval")]
    pub status_poll_interval_ms: u64,

    #[serde(default = "default_true")]
    pub show_activity_leds: bool,

    #[serde(default = "default_true")]
    pub show_connection_status: bool,
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
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
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

/// LED indicator configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndicatorConfig {
    pub signal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truthy: Option<bool>,
    #[serde(rename = "in")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_array: Option<Vec<serde_json::Value>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay: Option<OverlayConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator: Option<IndicatorConfig>,
}

/// MIDI control specification
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MidiSpec {
    #[serde(rename = "type")]
    pub midi_type: MidiType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
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
    /// Load configuration from file with validation
    pub async fn load(path: &str) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read config file: {}", path))?;
        
        let config: AppConfig = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse YAML config: {}", path))?;
        
        // Validate the loaded configuration
        config.validate()?;
        
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

    /// Validate configuration for correctness and consistency
    pub fn validate(&self) -> Result<()> {
        // Validate MIDI configuration
        if self.midi.input_port.is_empty() {
            anyhow::bail!("MIDI input_port cannot be empty");
        }
        if self.midi.output_port.is_empty() {
            anyhow::bail!("MIDI output_port cannot be empty");
        }

        // Collect all app names referenced in MIDI config
        let mut midi_app_names = std::collections::HashSet::new();
        if let Some(apps) = &self.midi.apps {
            for app in apps {
                if app.name.is_empty() {
                    anyhow::bail!("MIDI app name cannot be empty");
                }
                midi_app_names.insert(&app.name);
            }
        }

        // Validate pages
        if self.pages.is_empty() {
            anyhow::bail!("At least one page must be defined");
        }

        for (page_idx, page) in self.pages.iter().enumerate() {
            if page.name.is_empty() {
                anyhow::bail!("Page {} name cannot be empty", page_idx);
            }

            // Validate controls in this page
            if let Some(controls) = &page.controls {
                for (control_id, mapping) in controls {
                    self.validate_control_mapping(control_id, mapping, &midi_app_names)
                        .with_context(|| format!("Invalid control '{}' in page '{}'", control_id, page.name))?;
                }
            }

            // Validate LCD colors (should be 0-7 for X-Touch)
            if let Some(lcd) = &page.lcd {
                if let Some(colors) = &lcd.colors {
                    for (idx, color) in colors.iter().enumerate() {
                        if let LcdColor::Numeric(num) = color {
                            if *num > 7 {
                                anyhow::bail!(
                                    "LCD color {} in page '{}' strip {} is invalid (must be 0-7)",
                                    num, page.name, idx
                                );
                            }
                        }
                    }
                }
            }
        }

        // Validate global controls
        if let Some(global) = &self.pages_global {
            if let Some(controls) = &global.controls {
                for (control_id, mapping) in controls {
                    self.validate_control_mapping(control_id, mapping, &midi_app_names)
                        .with_context(|| format!("Invalid global control '{}'", control_id))?;
                }
            }
        }

        Ok(())
    }

    /// Validate a single control mapping
    fn validate_control_mapping(
        &self,
        control_id: &str,
        mapping: &ControlMapping,
        _midi_app_names: &std::collections::HashSet<&String>,
    ) -> Result<()> {
        if mapping.app.is_empty() {
            anyhow::bail!("Control '{}' app name cannot be empty", control_id);
        }

        // Validate MIDI specification if present
        if let Some(midi_spec) = &mapping.midi {
            match midi_spec.midi_type {
                MidiType::Cc => {
                    if midi_spec.cc.is_none() {
                        anyhow::bail!("CC type requires 'cc' field in control '{}'", control_id);
                    }
                    if midi_spec.channel.is_none() {
                        anyhow::bail!("CC type requires 'channel' field in control '{}'", control_id);
                    }
                }
                MidiType::Note => {
                    if midi_spec.note.is_none() {
                        anyhow::bail!("Note type requires 'note' field in control '{}'", control_id);
                    }
                    if midi_spec.channel.is_none() {
                        anyhow::bail!("Note type requires 'channel' field in control '{}'", control_id);
                    }
                }
                MidiType::Pb => {
                    if midi_spec.channel.is_none() {
                        anyhow::bail!("PitchBend type requires 'channel' field in control '{}'", control_id);
                    }
                }
                MidiType::Passthrough => {
                    // Passthrough doesn't require specific fields
                }
            }

            // Validate channel range (1-16 for MIDI, but 0-15 internally)
            if let Some(channel) = midi_spec.channel {
                if channel == 0 || channel > 16 {
                    anyhow::bail!(
                        "Control '{}' has invalid MIDI channel {} (must be 1-16)",
                        control_id, channel
                    );
                }
            }

            // Validate CC/Note range (0-127)
            if let Some(cc) = midi_spec.cc {
                if cc > 127 {
                    anyhow::bail!("Control '{}' has invalid CC number {} (must be 0-127)", control_id, cc);
                }
            }
            if let Some(note) = midi_spec.note {
                if note > 127 {
                    anyhow::bail!("Control '{}' has invalid note number {} (must be 0-127)", control_id, note);
                }
            }
        }

        // Validate that action OR midi is specified (not both empty, unless passthrough)
        if mapping.action.is_none() && mapping.midi.is_none() {
            anyhow::bail!("Control '{}' must specify either 'action' or 'midi'", control_id);
        }

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
fn default_gamepad_provider() -> String { "hid".to_string() }
fn default_pan_gain() -> f32 { 15.0 }
fn default_zoom_gain() -> f32 { 3.0 }
fn default_deadzone() -> f32 { 0.02 }
fn default_gamma() -> f32 { 1.5 }
fn default_activity_duration() -> u64 { 200 }
fn default_poll_interval() -> u64 { 100 }
