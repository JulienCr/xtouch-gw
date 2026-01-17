//! Control mapping parser for X-Touch control definitions
//!
//! Parses the xtouch-matching.csv file to map control IDs to MIDI messages.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;
use tracing::info;

/// Control mapping entry from CSV
#[derive(Debug, Clone, Deserialize)]
pub struct ControlMapping {
    pub control_id: String,
    pub group: String,
    pub ctrl_message: String,
    pub mcu_message: String,
}

/// Parsed MIDI message specification
#[derive(Debug, Clone, PartialEq)]
pub enum MidiSpec {
    /// Control Change: cc=number
    ControlChange { cc: u8 },
    /// Note: note=number  
    Note { note: u8 },
    /// PitchBend: pb=chN
    PitchBend { channel: u8 },
}

impl MidiSpec {
    /// Parse a MIDI spec string like "cc=70", "note=110", "pb=ch1"
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();

        if let Some(cc_str) = spec.strip_prefix("cc=") {
            let cc = cc_str
                .parse::<u8>()
                .with_context(|| format!("Invalid CC number: {}", cc_str))?;
            Ok(MidiSpec::ControlChange { cc })
        } else if let Some(note_str) = spec.strip_prefix("note=") {
            let note = note_str
                .parse::<u8>()
                .with_context(|| format!("Invalid note number: {}", note_str))?;
            Ok(MidiSpec::Note { note })
        } else if let Some(pb_str) = spec.strip_prefix("pb=") {
            // Parse channel from "ch1", "ch2", etc.
            let channel = if let Some(ch_str) = pb_str.strip_prefix("ch") {
                ch_str
                    .parse::<u8>()
                    .with_context(|| format!("Invalid channel: {}", pb_str))?
                    .saturating_sub(1) // Convert 1-based to 0-based
            } else {
                anyhow::bail!("Invalid pitch bend format: {}", spec);
            };
            Ok(MidiSpec::PitchBend { channel })
        } else {
            anyhow::bail!("Unknown MIDI spec format: {}", spec);
        }
    }

    /// Parse a MIDI spec from raw MIDI bytes
    pub fn from_raw(raw: &[u8]) -> Result<Self> {
        if raw.is_empty() {
            anyhow::bail!("Empty MIDI message");
        }

        let status = raw[0];
        let type_nibble = (status & 0xF0) >> 4;
        let channel = status & 0x0F;

        match type_nibble {
            0x8 | 0x9 => {
                // Note Off (0x8) or Note On (0x9)
                if raw.len() < 2 {
                    anyhow::bail!("Invalid Note message: too short");
                }
                Ok(MidiSpec::Note { note: raw[1] })
            },
            0xB => {
                // Control Change
                if raw.len() < 2 {
                    anyhow::bail!("Invalid CC message: too short");
                }
                Ok(MidiSpec::ControlChange { cc: raw[1] })
            },
            0xE => {
                // Pitch Bend
                Ok(MidiSpec::PitchBend { channel })
            },
            _ => {
                anyhow::bail!("Unsupported MIDI message type: 0x{:02X}", type_nibble);
            },
        }
    }
}

/// Control mapping database
#[derive(Debug, Clone)]
pub struct ControlMappingDB {
    /// Control mappings by control_id
    pub mappings: HashMap<String, ControlMapping>,

    /// Controls grouped by category
    pub groups: HashMap<String, Vec<String>>,
}

impl ControlMappingDB {
    /// Load control mappings from CSV file
    pub async fn load_from_csv(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let csv_content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read CSV file: {}", path.display()))?;

        Self::parse_csv(&csv_content)
    }

    /// Load from embedded CSV string
    pub fn load_from_string(csv_content: &str) -> Result<Self> {
        Self::parse_csv(csv_content)
    }

    /// Parse CSV content
    fn parse_csv(csv_content: &str) -> Result<Self> {
        let mut reader = csv::Reader::from_reader(csv_content.as_bytes());
        let mut mappings = HashMap::new();
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();

        for result in reader.deserialize() {
            let mapping: ControlMapping = result.context("Failed to parse CSV row")?;

            // Validate that we can parse the MIDI specs
            MidiSpec::parse(&mapping.ctrl_message)
                .with_context(|| format!("Invalid ctrl_message for {}", mapping.control_id))?;
            MidiSpec::parse(&mapping.mcu_message)
                .with_context(|| format!("Invalid mcu_message for {}", mapping.control_id))?;

            // Add to groups
            groups
                .entry(mapping.group.clone())
                .or_default()
                .push(mapping.control_id.clone());

            // Store mapping
            mappings.insert(mapping.control_id.clone(), mapping);
        }

        info!(
            "Loaded {} control mappings in {} groups",
            mappings.len(),
            groups.len()
        );

        Ok(Self { mappings, groups })
    }

    /// Get a control mapping by ID
    pub fn get(&self, control_id: &str) -> Option<&ControlMapping> {
        self.mappings.get(control_id)
    }

    /// Get MIDI spec for a control in the specified mode
    pub fn get_midi_spec(&self, control_id: &str, mcu_mode: bool) -> Option<MidiSpec> {
        self.mappings.get(control_id).and_then(|mapping| {
            let spec_str = if mcu_mode {
                &mapping.mcu_message
            } else {
                &mapping.ctrl_message
            };
            MidiSpec::parse(spec_str).ok()
        })
    }

    /// Get all control IDs in a group
    pub fn get_group(&self, group: &str) -> Option<&Vec<String>> {
        self.groups.get(group)
    }

    /// Get all groups
    pub fn groups(&self) -> impl Iterator<Item = &str> {
        self.groups.keys().map(|s| s.as_str())
    }

    /// Find control ID by MIDI message (reverse lookup)
    pub fn find_control_by_midi(&self, midi_spec: &MidiSpec, mcu_mode: bool) -> Option<&str> {
        self.mappings.iter().find_map(|(id, mapping)| {
            let spec_str = if mcu_mode {
                &mapping.mcu_message
            } else {
                &mapping.ctrl_message
            };

            if let Ok(spec) = MidiSpec::parse(spec_str) {
                if spec == *midi_spec {
                    return Some(id.as_str());
                }
            }
            None
        })
    }

    /// Get all fader control IDs (fader1-fader8, fader_master)
    pub fn get_fader_controls(&self) -> Vec<&str> {
        let mut faders = Vec::new();
        for i in 1..=8 {
            let fader_id = format!("fader{}", i);
            if self.mappings.contains_key(&fader_id) {
                faders.push(self.mappings[&fader_id].control_id.as_str());
            }
        }
        if self.mappings.contains_key("fader_master") {
            faders.push("fader_master");
        }
        faders
    }

    /// Get all button control IDs for a strip (mute, solo, rec, select)
    pub fn get_strip_buttons(&self, strip_num: u8) -> Vec<&str> {
        let mut buttons = Vec::new();
        for button_type in &["rec", "solo", "mute", "select"] {
            let button_id = format!("{}{}", button_type, strip_num);
            if self.mappings.contains_key(&button_id) {
                buttons.push(self.mappings[&button_id].control_id.as_str());
            }
        }
        buttons
    }

    /// Get all encoder control IDs (vpotN_rotate, vpotN_push)
    pub fn get_encoder_controls(&self, encoder_num: u8) -> Vec<&str> {
        let mut controls = Vec::new();
        let rotate_id = format!("vpot{}_rotate", encoder_num);
        let push_id = format!("vpot{}_push", encoder_num);

        if self.mappings.contains_key(&rotate_id) {
            controls.push(self.mappings[&rotate_id].control_id.as_str());
        }
        if self.mappings.contains_key(&push_id) {
            controls.push(self.mappings[&push_id].control_id.as_str());
        }
        controls
    }
}

/// Default embedded CSV content (for when file is not available)
pub const DEFAULT_CSV: &str = include_str!("../docs/xtouch-matching.csv");

/// Global cache for the embedded default mappings
static DEFAULT_DB: OnceLock<ControlMappingDB> = OnceLock::new();

/// In-memory cache for on-disk CSV (path + mtime)
struct FileCache {
    path: PathBuf,
    mtime: SystemTime,
    db: ControlMappingDB,
}

static FILE_DB: OnceLock<Mutex<Option<FileCache>>> = OnceLock::new();

/// Load the default control mappings (cached after first parse)
pub fn load_default_mappings() -> Result<ControlMappingDB> {
    if let Some(db) = DEFAULT_DB.get() {
        return Ok(db.clone());
    }

    let db = ControlMappingDB::load_from_string(DEFAULT_CSV)?;
    // Ignore error if another thread set it first
    let _ = DEFAULT_DB.set(db.clone());
    Ok(db)
}

/// Warm the default cache so parsing happens at startup
pub fn warm_default_mappings() -> Result<()> {
    let _ = load_default_mappings()?;
    Ok(())
}

/// Load mappings from a CSV path, re-parsing only when the file changed.
pub async fn load_mappings_from_path(path: impl AsRef<Path>) -> Result<ControlMappingDB> {
    let path = path.as_ref().to_path_buf();

    // Check file metadata (mtime)
    let meta = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("Failed to stat CSV file: {}", path.display()))?;
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    let cache = FILE_DB.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().expect("cache mutex poisoned");
        if let Some(FileCache {
            path: cached_path,
            mtime: cached_mtime,
            db,
        }) = guard.as_ref()
        {
            if *cached_path == path && *cached_mtime == mtime {
                return Ok(db.clone());
            }
        }
    }

    // Not cached or outdated -> re-read
    let csv_content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("Failed to read CSV file: {}", path.display()))?;
    let db = ControlMappingDB::parse_csv(&csv_content)?;

    // Update cache
    {
        let mut guard = cache.lock().expect("cache mutex poisoned");
        *guard = Some(FileCache {
            path,
            mtime,
            db: db.clone(),
        });
    }

    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_midi_spec_parsing() {
        // Test CC parsing
        assert_eq!(
            MidiSpec::parse("cc=70").unwrap(),
            MidiSpec::ControlChange { cc: 70 }
        );

        // Test Note parsing
        assert_eq!(
            MidiSpec::parse("note=110").unwrap(),
            MidiSpec::Note { note: 110 }
        );

        // Test PitchBend parsing
        assert_eq!(
            MidiSpec::parse("pb=ch1").unwrap(),
            MidiSpec::PitchBend { channel: 0 } // ch1 becomes channel 0 (0-based)
        );
        assert_eq!(
            MidiSpec::parse("pb=ch8").unwrap(),
            MidiSpec::PitchBend { channel: 7 }
        );
    }

    #[test]
    fn test_load_default_mappings() {
        let db = load_default_mappings().unwrap();

        // Check that we have some mappings
        assert!(db.mappings.len() > 100);

        // Check specific control
        let fader1 = db.get("fader1").unwrap();
        assert_eq!(fader1.control_id, "fader1");
        assert_eq!(fader1.group, "strip");
        assert_eq!(fader1.ctrl_message, "cc=70");
        assert_eq!(fader1.mcu_message, "pb=ch1");

        // Test MIDI spec retrieval
        assert_eq!(
            db.get_midi_spec("fader1", false).unwrap(),
            MidiSpec::ControlChange { cc: 70 }
        );
        assert_eq!(
            db.get_midi_spec("fader1", true).unwrap(),
            MidiSpec::PitchBend { channel: 0 }
        );
    }

    #[test]
    fn test_reverse_lookup() {
        let db = load_default_mappings().unwrap();

        // Find control by MIDI message
        let control = db.find_control_by_midi(
            &MidiSpec::ControlChange { cc: 70 },
            false, // CTRL mode
        );
        assert_eq!(control, Some("fader1"));

        let control = db.find_control_by_midi(
            &MidiSpec::PitchBend { channel: 0 },
            true, // MCU mode
        );
        assert_eq!(control, Some("fader1"));
    }

    #[test]
    fn test_group_queries() {
        let db = load_default_mappings().unwrap();

        // Check groups exist
        assert!(db.get_group("strip").is_some());
        assert!(db.get_group("transport").is_some());
        assert!(db.get_group("function").is_some());

        // Check fader controls
        let faders = db.get_fader_controls();
        assert_eq!(faders.len(), 9); // 8 strips + master
        assert!(faders.contains(&"fader_master"));

        // Check strip buttons
        let buttons = db.get_strip_buttons(1);
        assert_eq!(buttons.len(), 4);
        assert!(buttons.contains(&"mute1"));
        assert!(buttons.contains(&"solo1"));
    }
}
