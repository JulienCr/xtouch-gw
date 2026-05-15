//! Configuration management for XTouch GW
//!
//! Handles loading, parsing, and hot-reloading of YAML configuration files.

pub mod profiles;
pub mod watcher;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;

/// Root configuration structure
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
    /// Windows audio (master + per-app session) configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winaudio: Option<WinAudioConfig>,
    pub pages: Vec<PageConfig>,
}

/// Windows audio driver configuration.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WinAudioConfig {
    /// Apps pinned to specific fader slots. Faders 1..=8.
    #[serde(default)]
    pub pinned_apps: Vec<PinnedApp>,
}

/// A pinned audio session: a process name fixed on a specific fader slot.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PinnedApp {
    /// Fader slot 1..=8.
    #[schemars(range(min = 1, max = 8))]
    pub fader: u8,
    /// Process executable name (e.g. "Discord.exe"). Match is case-insensitive.
    pub process_name: String,
    /// Optional friendly label rendered on the LCD; falls back to process_name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Optional explicit LCD color. When unset, the driver assigns a
    /// color from the same 1..=7 cycle as discovered apps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<LcdColor>,
}

/// MIDI port configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MidiConfig {
    pub input_port: String,
    pub output_port: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<MidiAppConfig>>,
}

/// App-specific MIDI port mapping
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MidiAppConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_port: Option<String>,
}

/// OBS WebSocket configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ObsConfig {
    #[serde(default = "default_obs_host")]
    pub host: String,
    #[serde(default = "default_obs_port")]
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_control: Option<CameraControlConfig>,
}

/// Camera control configuration for OBS split views
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CameraControlConfig {
    pub cameras: Vec<CameraConfig>,
    pub splits: SplitConfig,
    /// Default camera to switch to when exiting split mode. If not set, uses first camera.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_camera: Option<String>,
}

/// Individual camera configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CameraConfig {
    pub id: String,
    pub scene: String,
    pub source: String,
    pub split_source: String,
    /// Enable PTZ (pan/tilt/zoom) control for this camera. Default: true
    #[serde(default = "default_true")]
    pub enable_ptz: bool,
}

/// Split scene configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SplitConfig {
    pub left: String,
    pub right: String,
}

/// X-Touch specific configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct XTouchConfig {
    #[serde(default = "default_xtouch_mode")]
    pub mode: XTouchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay: Option<OverlayConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_per_app: Option<HashMap<String, OverlayConfig>>,
    /// Delay in milliseconds before applying initial page refresh after drivers are registered.
    /// This allows drivers time to connect and send fresh feedback before stale snapshot
    /// values are applied to the X-Touch. Default: 500ms.
    /// BUG-008 FIX: Prevents stale snapshot values from overriding fresh app feedback.
    #[serde(default = "default_startup_refresh_delay")]
    pub startup_refresh_delay_ms: u64,
}

/// X-Touch operation mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum XTouchMode {
    Mcu,
    Ctrl,
}

/// LCD overlay configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct OverlayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<OverlayMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_bits: Option<CcBits>,
}

/// Overlay display mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverlayMode {
    Percent,
    #[serde(rename = "7bit")]
    SevenBit,
    #[serde(rename = "8bit")]
    EightBit,
}

/// CC bit display mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
pub enum CcBits {
    #[serde(rename = "7bit")]
    SevenBit,
    #[serde(rename = "8bit")]
    EightBit,
}

/// Page navigation configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PagingConfig {
    #[serde(default = "default_paging_channel")]
    pub channel: u8,
    #[serde(default = "default_prev_note")]
    pub prev_note: u8,
    #[serde(default = "default_next_note")]
    pub next_note: u8,
}

/// Gamepad configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GamepadConfig {
    pub enabled: bool,
    #[serde(default = "default_gamepad_provider")]
    pub provider: String,

    // Legacy single-gamepad config (for backward compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analog: Option<AnalogConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hid: Option<HidProviderConfig>,

    // NEW: Multi-gamepad support
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gamepads: Option<Vec<GamepadSlotConfig>>,
}

/// Analog stick configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HidProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_match: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_csv: Option<String>,
}

/// Configuration for a single gamepad slot (multi-gamepad mode)
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GamepadSlotConfig {
    /// Product name pattern to match (substring, case-insensitive)
    pub product_match: String,

    /// Per-gamepad analog configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analog: Option<AnalogConfig>,

    /// Camera targeting mode for gamepad controls:
    /// - None: static mode (params used as-is)
    /// - "dynamic": runtime-selectable via Stream Deck API
    /// - "camera_id": fixed to specific camera from camera_control config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_target: Option<String>,
}

/// System tray UI configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GlobalPageDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controls: Option<HashMap<String, ControlMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lcd: Option<LcdConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passthroughs: Option<Vec<PassthroughConfig>>,
}

/// Page configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MidiType {
    Cc,
    Note,
    Pb,
    Passthrough,
}

/// LCD configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LcdConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<LcdLabel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<LcdColor>>,
}

/// LCD label (string or structured)
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum LcdColor {
    Numeric(u32),
    Named(String),
}

impl LcdColor {
    /// Convert LCD color to X-Touch color value (0-7)
    /// Colors: 0=black, 1=red, 2=green, 3=yellow, 4=blue, 5=magenta, 6=cyan, 7=white
    pub fn to_u8(&self) -> u8 {
        match self {
            LcdColor::Numeric(n) => (*n as u8).min(7),
            LcdColor::Named(name) => match name.to_lowercase().as_str() {
                "black" | "off" => 0,
                "red" => 1,
                "green" => 2,
                "yellow" => 3,
                "blue" => 4,
                "magenta" | "pink" | "purple" => 5,
                "cyan" | "aqua" => 6,
                "white" => 7,
                _ => {
                    tracing::warn!("Unknown LCD color '{}', defaulting to black", name);
                    0
                },
            },
        }
    }
}

/// MIDI passthrough configuration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TransformConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pb_to_note: Option<PbToNoteTransform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pb_to_cc: Option<PbToCcTransform>,
}

/// PitchBend to Note transform
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PbToNoteTransform {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
}

/// PitchBend to CC transform
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PbToCcTransform {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_channel: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_cc: Option<serde_json::Value>, // Can be number or hex string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_by_channel: Option<HashMap<u8, serde_json::Value>>,
}

impl AppConfig {
    /// True when the X-Touch is configured (or defaulted) to MCU mode.
    pub fn is_mcu_mode(&self) -> bool {
        self.xtouch
            .as_ref()
            .map(|x| matches!(x.mode, XTouchMode::Mcu))
            .unwrap_or(true)
    }

    /// Collect every app name referenced by control mappings or
    /// passthrough configs on any page (including `pages_global`). Used
    /// to decide which drivers a profile requires.
    ///
    /// Walks both `controls.*.app` and page-level `passthrough` /
    /// `passthroughs` so that a profile relying solely on top-level
    /// passthrough routing does not have its bridge driver pruned on
    /// reload.
    pub fn referenced_apps(&self) -> std::collections::HashSet<String> {
        fn record_controls(
            controls: &HashMap<String, ControlMapping>,
            apps: &mut std::collections::HashSet<String>,
        ) {
            for mapping in controls.values() {
                apps.insert(mapping.app.clone());
            }
        }

        let mut apps = std::collections::HashSet::new();
        for page in &self.pages {
            if let Some(controls) = &page.controls {
                record_controls(controls, &mut apps);
            }
            if let Some(pt) = &page.passthrough {
                apps.insert(pt.driver.clone());
            }
            if let Some(pts) = &page.passthroughs {
                for pt in pts {
                    apps.insert(pt.driver.clone());
                }
            }
        }
        if let Some(global) = self.pages_global.as_ref() {
            if let Some(controls) = global.controls.as_ref() {
                record_controls(controls, &mut apps);
            }
            if let Some(pts) = global.passthroughs.as_ref() {
                for pt in pts {
                    apps.insert(pt.driver.clone());
                }
            }
        }
        apps
    }

    /// True if any page or `pages_global` mapping (control or
    /// passthrough) references the given app name.
    pub fn references_app(&self, name: &str) -> bool {
        let any_control = |c: &HashMap<String, ControlMapping>| c.values().any(|m| m.app == name);
        let any_pt = |pts: &[PassthroughConfig]| pts.iter().any(|p| p.driver == name);

        let in_pages = self.pages.iter().any(|p| {
            p.controls.as_ref().is_some_and(any_control)
                || p.passthrough.as_ref().is_some_and(|pt| pt.driver == name)
                || p.passthroughs.as_ref().is_some_and(|pts| any_pt(pts))
        });
        if in_pages {
            return true;
        }
        let Some(global) = self.pages_global.as_ref() else {
            return false;
        };
        global.controls.as_ref().is_some_and(any_control)
            || global.passthroughs.as_ref().is_some_and(|pts| any_pt(pts))
    }

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

    /// Save configuration to file. Currently uncalled; kept as the
    /// canonical YAML serializer for the upcoming editor write path
    /// (the legacy editor goes through `config::profiles` instead).
    #[allow(dead_code)]
    pub async fn save(&self, path: &str) -> Result<()> {
        let yaml = serde_yaml::to_string(self).context("Failed to serialize config to YAML")?;

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

                // Warn about MIDI app port configuration issues
                let has_output = app.output_port.is_some();
                let has_input = app.input_port.is_some();

                if !has_output && !has_input {
                    tracing::warn!(
                        "MIDI app '{}' has no ports configured - bidirectional communication will not work",
                        app.name
                    );
                } else if has_output && !has_input {
                    tracing::warn!(
                        "MIDI app '{}' has output but no input port - feedback will not be received",
                        app.name
                    );
                } else if has_input && !has_output {
                    tracing::warn!(
                        "MIDI app '{}' has input but no output port - commands will not be sent",
                        app.name
                    );
                }
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
                        .with_context(|| {
                            format!("Invalid control '{}' in page '{}'", control_id, page.name)
                        })?;
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
                                    num,
                                    page.name,
                                    idx
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

        // Validate winaudio pinned_apps slot range and uniqueness.
        if let Some(winaudio) = &self.winaudio {
            let mut seen = std::collections::HashSet::new();
            for pin in &winaudio.pinned_apps {
                if !(1..=8).contains(&pin.fader) {
                    anyhow::bail!(
                        "winaudio.pinned_apps: fader slot {} for '{}' must be in 1..=8",
                        pin.fader,
                        pin.process_name
                    );
                }
                if !seen.insert(pin.fader) {
                    anyhow::bail!(
                        "winaudio.pinned_apps: fader slot {} is pinned more than once",
                        pin.fader
                    );
                }
                if pin.process_name.trim().is_empty() {
                    anyhow::bail!(
                        "winaudio.pinned_apps: process_name cannot be empty (fader {})",
                        pin.fader
                    );
                }
            }
        }

        // Validate winaudio session-target params (e.g. `pinned:1`,
        // `discovered:3`, `auto`) at config-load. Typos previously
        // surfaced only when the user pressed the button (#38).
        self.validate_winaudio_session_targets()?;

        Ok(())
    }

    /// Iterate every page (and `pages_global`) for control mappings
    /// bound to `app: "winaudio"` and a session-target action; parse
    /// their first param via [`parse_session_target`]. Errors carry
    /// `page` + `control_id` context so the user can fix the YAML.
    fn validate_winaudio_session_targets(&self) -> Result<()> {
        for page in &self.pages {
            if let Some(controls) = page.controls.as_ref() {
                validate_winaudio_controls_in(controls, &page.name)?;
            }
        }
        if let Some(global) = self.pages_global.as_ref() {
            if let Some(controls) = global.controls.as_ref() {
                validate_winaudio_controls_in(controls, "<global>")?;
            }
        }
        Ok(())
    }

    /// Validate a single control mapping
    fn validate_control_mapping(
        &self,
        control_id: &str,
        mapping: &ControlMapping,
        midi_app_names: &std::collections::HashSet<&String>,
    ) -> Result<()> {
        if mapping.app.is_empty() {
            anyhow::bail!("Control '{}' app name cannot be empty", control_id);
        }

        // Validate app name references a configured app
        // (obs, winaudio, winmedia are non-MIDI apps that don't need a port entry).
        const NON_MIDI_APPS: &[&str] = &["obs", "winaudio", "winmedia"];
        if !NON_MIDI_APPS.contains(&mapping.app.as_str()) && !midi_app_names.contains(&mapping.app)
        {
            anyhow::bail!(
                "Control '{}' references unknown app '{}'. Available apps: {:?}",
                control_id,
                mapping.app,
                midi_app_names
            );
        }

        // Validate MIDI specification if present
        if let Some(midi_spec) = &mapping.midi {
            match midi_spec.midi_type {
                MidiType::Cc => {
                    if midi_spec.cc.is_none() {
                        anyhow::bail!("CC type requires 'cc' field in control '{}'", control_id);
                    }
                    if midi_spec.channel.is_none() {
                        anyhow::bail!(
                            "CC type requires 'channel' field in control '{}'",
                            control_id
                        );
                    }
                },
                MidiType::Note => {
                    if midi_spec.note.is_none() {
                        anyhow::bail!(
                            "Note type requires 'note' field in control '{}'",
                            control_id
                        );
                    }
                    if midi_spec.channel.is_none() {
                        anyhow::bail!(
                            "Note type requires 'channel' field in control '{}'",
                            control_id
                        );
                    }
                },
                MidiType::Pb => {
                    if midi_spec.channel.is_none() {
                        anyhow::bail!(
                            "PitchBend type requires 'channel' field in control '{}'",
                            control_id
                        );
                    }
                },
                MidiType::Passthrough => {
                    // Passthrough doesn't require specific fields
                },
            }

            // Validate channel range (1-16 for MIDI, but 0-15 internally)
            if let Some(channel) = midi_spec.channel {
                if channel == 0 || channel > 16 {
                    anyhow::bail!(
                        "Control '{}' has invalid MIDI channel {} (must be 1-16)",
                        control_id,
                        channel
                    );
                }
            }

            // Validate CC/Note range (0-127)
            if let Some(cc) = midi_spec.cc {
                if cc > 127 {
                    anyhow::bail!(
                        "Control '{}' has invalid CC number {} (must be 0-127)",
                        control_id,
                        cc
                    );
                }
            }
            if let Some(note) = midi_spec.note {
                if note > 127 {
                    anyhow::bail!(
                        "Control '{}' has invalid note number {} (must be 0-127)",
                        control_id,
                        note
                    );
                }
            }
        }

        // Validate that action OR midi is specified (not both empty, unless passthrough)
        if mapping.action.is_none() && mapping.midi.is_none() {
            anyhow::bail!(
                "Control '{}' must specify either 'action' or 'midi'",
                control_id
            );
        }

        Ok(())
    }
}

/// Driver name that owns Windows audio session control. Duplicated here
/// (and kept in sync with `drivers::winaudio::DRIVER_NAME`) so the lib
/// crate can validate session-target YAML without depending on the bin
/// crate's `drivers` module.
const WINAUDIO_DRIVER_NAME: &str = "winaudio";

/// Actions on `app: "winaudio"` that consume a session target as their
/// first param. Used by `validate_winaudio_session_targets` so config-load
/// rejects typos like `"pined:1"` early (#38).
const WINAUDIO_SESSION_ACTIONS: &[&str] = &["session_volume", "session_mute"];

/// Validate the session-target param of every winaudio session-action
/// mapping in `controls`. Errors carry the page/control context.
fn validate_winaudio_controls_in(
    controls: &HashMap<String, ControlMapping>,
    page_name: &str,
) -> Result<()> {
    for (control_id, mapping) in controls {
        if mapping.app != WINAUDIO_DRIVER_NAME {
            continue;
        }
        let Some(action) = mapping.action.as_deref() else {
            continue;
        };
        if !WINAUDIO_SESSION_ACTIONS.contains(&action) {
            continue;
        }
        let params = mapping.params.as_deref().unwrap_or(&[]);
        parse_winaudio_session_target_str(params).map_err(|e| {
            anyhow::anyhow!(
                "page '{}' control '{}' (winaudio.{}): {}",
                page_name,
                control_id,
                action,
                e
            )
        })?;
    }
    Ok(())
}

/// Lightweight mirror of `drivers::winaudio::actions::parse_session_target`
/// for use at config-validation time (lib-crate, no access to `drivers`).
/// Keep semantics in sync with the runtime parser — both must accept the
/// same surface and reject the same typos.
fn parse_winaudio_session_target_str(params: &[serde_json::Value]) -> Result<()> {
    let raw = params
        .first()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "session action requires a target parameter (auto, pinned:N or discovered:N)"
            )
        })?
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!("session target must be a string (auto, pinned:N or discovered:N)")
        })?;

    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        return Ok(());
    }

    let (kind, idx) = trimmed
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("session target '{}' missing ':' separator", raw))?;

    let n: u8 = idx
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("session target '{}': index '{}' is not a u8", raw, idx))?;

    match kind.trim() {
        "pinned" => {
            if !(1..=8).contains(&n) {
                return Err(anyhow::anyhow!("pinned slot {} must be in 1..=8", n));
            }
        },
        "discovered" => {
            if n >= 8 {
                return Err(anyhow::anyhow!("discovered slot {} must be < 8", n));
            }
        },
        other => {
            return Err(anyhow::anyhow!(
                "unknown session target kind '{}': expected 'auto', 'pinned' or 'discovered'",
                other
            ));
        },
    }
    Ok(())
}

// Default value functions
fn default_obs_host() -> String {
    "localhost".to_string()
}
fn default_obs_port() -> u16 {
    4455
}
fn default_xtouch_mode() -> XTouchMode {
    XTouchMode::Mcu
}
fn default_startup_refresh_delay() -> u64 {
    500
} // 500ms default delay for BUG-008 fix
fn default_true() -> bool {
    true
}
fn default_paging_channel() -> u8 {
    1
}
fn default_prev_note() -> u8 {
    46
}
fn default_next_note() -> u8 {
    47
}
fn default_gamepad_provider() -> String {
    "hid".to_string()
}
fn default_pan_gain() -> f32 {
    15.0
}
fn default_zoom_gain() -> f32 {
    3.0
}
fn default_deadzone() -> f32 {
    0.02
}
fn default_gamma() -> f32 {
    1.5
}
fn default_activity_duration() -> u64 {
    200
}
fn default_poll_interval() -> u64 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: deriving `JsonSchema` across the entire config tree must
    /// produce a serializable schema. This catches missing derives on nested
    /// types that would otherwise only fail when `export-schema` is run.
    #[test]
    fn app_config_schema_serializes() {
        let schema = schemars::schema_for!(AppConfig);
        let json =
            serde_json::to_string_pretty(&schema).expect("AppConfig schema must serialize to JSON");
        assert!(json.contains("AppConfig"));
        assert!(json.contains("MidiConfig"));
    }

    /// `config.example.yaml` must round-trip through the parser + validator.
    /// Catches schema regressions before the bundled example silently breaks.
    #[test]
    fn example_config_parses_and_validates() {
        let yaml = std::fs::read_to_string("config.example.yaml")
            .expect("config.example.yaml must exist at repo root");
        let parsed: AppConfig = serde_yaml::from_str(&yaml).expect("YAML parse failed");
        parsed.validate().expect("config validation failed");

        let win = parsed.winaudio.as_ref().expect("winaudio block expected");
        assert_eq!(win.pinned_apps.len(), 3);
    }

    fn empty_config() -> AppConfig {
        AppConfig {
            midi: MidiConfig {
                input_port: "in".into(),
                output_port: "out".into(),
                apps: None,
            },
            obs: None,
            xtouch: None,
            paging: None,
            gamepad: None,
            pages_global: None,
            winaudio: None,
            pages: vec![],
            tray: None,
        }
    }

    fn control(app: &str) -> ControlMapping {
        ControlMapping {
            app: app.to_string(),
            action: Some("noop".into()),
            params: None,
            midi: None,
            indicator: None,
            overlay: None,
        }
    }

    fn passthrough(driver: &str) -> PassthroughConfig {
        PassthroughConfig {
            driver: driver.to_string(),
            to_port: "to".into(),
            from_port: "from".into(),
            filter: None,
            optional: None,
            transform: None,
        }
    }

    #[test]
    fn referenced_apps_collects_page_and_global_controls() {
        let mut cfg = empty_config();
        let mut page_controls = HashMap::new();
        page_controls.insert("fader1".into(), control("voicemeeter"));
        page_controls.insert("mute1".into(), control("qlc"));

        cfg.pages.push(PageConfig {
            name: "P1".into(),
            controls: Some(page_controls),
            ..PageConfig::default()
        });

        let mut global_controls = HashMap::new();
        global_controls.insert("prev".into(), control("obs"));
        cfg.pages_global = Some(GlobalPageDefaults {
            controls: Some(global_controls),
            lcd: None,
            passthroughs: None,
        });

        let apps = cfg.referenced_apps();
        assert!(apps.contains("voicemeeter"));
        assert!(apps.contains("qlc"));
        assert!(apps.contains("obs"));
        assert_eq!(apps.len(), 3);
    }

    #[test]
    fn referenced_apps_includes_page_passthrough_drivers() {
        let mut cfg = empty_config();
        cfg.pages.push(PageConfig {
            name: "P1".into(),
            passthrough: Some(passthrough("voicemeeter")),
            passthroughs: Some(vec![passthrough("qlc")]),
            ..PageConfig::default()
        });

        let apps = cfg.referenced_apps();
        assert!(apps.contains("voicemeeter"));
        assert!(apps.contains("qlc"));
    }

    #[test]
    fn referenced_apps_includes_global_passthrough_drivers() {
        let mut cfg = empty_config();
        cfg.pages_global = Some(GlobalPageDefaults {
            controls: None,
            lcd: None,
            passthroughs: Some(vec![passthrough("voicemeeter")]),
        });

        let apps = cfg.referenced_apps();
        assert!(apps.contains("voicemeeter"));
        assert_eq!(apps.len(), 1);
    }

    #[test]
    fn references_app_matches_passthrough_and_controls() {
        let mut cfg = empty_config();
        cfg.pages.push(PageConfig {
            name: "P1".into(),
            passthrough: Some(passthrough("voicemeeter")),
            ..PageConfig::default()
        });

        assert!(cfg.references_app("voicemeeter"));
        assert!(!cfg.references_app("qlc"));

        let mut controls = HashMap::new();
        controls.insert("fader1".into(), control("qlc"));
        cfg.pages[0].controls = Some(controls);

        assert!(cfg.references_app("qlc"));
    }

    // -- #38 — winaudio session-target validation at config-load -------------

    fn winaudio_control(action: &str, param: serde_json::Value) -> ControlMapping {
        ControlMapping {
            app: "winaudio".into(),
            action: Some(action.into()),
            params: Some(vec![param]),
            midi: None,
            indicator: None,
            overlay: None,
        }
    }

    fn cfg_with_winaudio_control(action: &str, param: serde_json::Value) -> AppConfig {
        let mut cfg = empty_config();
        let mut controls = HashMap::new();
        controls.insert("fader1".into(), winaudio_control(action, param));
        cfg.pages.push(PageConfig {
            name: "WinAudio".into(),
            controls: Some(controls),
            ..PageConfig::default()
        });
        cfg
    }

    #[test]
    fn validate_winaudio_session_target_accepts_valid_params() {
        for param in [
            serde_json::json!("pinned:1"),
            serde_json::json!("pinned:8"),
            serde_json::json!("discovered:0"),
            serde_json::json!("discovered:7"),
            serde_json::json!("auto"),
            serde_json::json!("AUTO"),
        ] {
            let cfg = cfg_with_winaudio_control("session_volume", param.clone());
            cfg.validate()
                .unwrap_or_else(|e| panic!("param {param:?} should be valid, got: {e}"));
        }
    }

    #[test]
    fn validate_winaudio_session_target_rejects_bad_prefix() {
        let cfg = cfg_with_winaudio_control("session_volume", serde_json::json!("pined:1"));
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("page 'WinAudio'"), "got: {err}");
        assert!(err.contains("fader1"), "got: {err}");
        assert!(err.contains("session_volume"), "got: {err}");
    }

    #[test]
    fn validate_winaudio_session_target_rejects_out_of_range() {
        let cfg = cfg_with_winaudio_control("session_mute", serde_json::json!("pinned:9"));
        assert!(cfg.validate().is_err());
        let cfg = cfg_with_winaudio_control("session_mute", serde_json::json!("pinned:0"));
        assert!(cfg.validate().is_err());
        let cfg = cfg_with_winaudio_control("session_mute", serde_json::json!("discovered:8"));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_winaudio_session_target_rejects_wrong_type() {
        // Numeric param where a string is required.
        let cfg = cfg_with_winaudio_control("session_volume", serde_json::json!(42));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_winaudio_session_target_ignores_non_session_actions() {
        // `master_volume` / `master_mute` take no session target — their
        // params should not be parsed. Even garbage must pass through.
        let cfg = cfg_with_winaudio_control("master_volume", serde_json::json!("not a target"));
        cfg.validate().expect("master_volume params not validated");
    }

    #[test]
    fn validate_winaudio_session_target_ignores_other_drivers() {
        // A `voicemeeter` control with a garbage first-param string must
        // still pass — only winaudio sessions are checked.
        let mut cfg = empty_config();
        let mut controls = HashMap::new();
        controls.insert(
            "fader1".into(),
            ControlMapping {
                app: "voicemeeter".into(),
                action: Some("session_volume".into()),
                params: Some(vec![serde_json::json!("pined:1")]),
                midi: None,
                indicator: None,
                overlay: None,
            },
        );
        cfg.midi.apps = Some(vec![MidiAppConfig {
            name: "voicemeeter".into(),
            output_port: Some("vm-out".into()),
            input_port: Some("vm-in".into()),
        }]);
        cfg.pages.push(PageConfig {
            name: "VM".into(),
            controls: Some(controls),
            ..PageConfig::default()
        });
        cfg.validate().expect("non-winaudio params not validated");
    }
}
