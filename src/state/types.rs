//! MIDI state type definitions
//!
//! Defines the core types for representing MIDI state entries, addresses, and values.

use serde::{Deserialize, Serialize};

/// Type of MIDI event supported by the StateStore
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MidiStatus {
    /// Note On/Off events
    Note,
    /// Control Change events
    CC,
    /// Pitch Bend events (14-bit)
    PB,
    /// System Exclusive messages
    SysEx,
}

impl std::fmt::Display for MidiStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiStatus::Note => write!(f, "note"),
            MidiStatus::CC => write!(f, "cc"),
            MidiStatus::PB => write!(f, "pb"),
            MidiStatus::SysEx => write!(f, "sysex"),
        }
    }
}

/// Logical address of a MIDI event (including the port/application source)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MidiAddr {
    /// Port identifier (e.g., "voicemeeter", "qlc", "obs")
    pub port_id: String,
    /// MIDI status type
    pub status: MidiStatus,
    /// MIDI channel (1-16), optional for SysEx
    pub channel: Option<u8>,
    /// First data byte (note number, CC number, etc.), optional for PB and SysEx
    pub data1: Option<u8>,
}

/// MIDI value: numeric (Note/CC/PB), text, or binary (full SysEx)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MidiValue {
    /// Numeric value (0-127 for CC/Note, 0-16383 for PB)
    Number(u16),
    /// Text value (for special cases)
    Text(String),
    /// Binary data (SysEx payload)
    #[serde(with = "serde_bytes")]
    Binary(Vec<u8>),
}

impl MidiValue {
    /// Extract numeric value if available
    pub fn as_number(&self) -> Option<u16> {
        match self {
            MidiValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Extract binary data if available
    pub fn as_binary(&self) -> Option<&[u8]> {
        match self {
            MidiValue::Binary(b) => Some(b),
            _ => None,
        }
    }
}

/// MIDI state entry with metadata, stored in the StateStore
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiStateEntry {
    /// Logical address
    pub addr: MidiAddr,
    /// Value
    pub value: MidiValue,
    /// Timestamp (milliseconds since epoch)
    pub ts: u64,
    /// Origin of the event
    pub origin: Origin,
    /// Whether this entry is confirmed/known
    pub known: bool,
    /// Whether this entry might be outdated (restored from snapshot)
    #[serde(default)]
    pub stale: bool,
    /// Hash for SysEx deduplication/tracing (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// Origin of a MIDI event
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// From application (Voicemeeter/QLC/OBS feedback)
    App,
    /// From X-Touch hardware
    XTouch,
}

/// Known application keys
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppKey {
    Voicemeeter,
    Qlc,
    Obs,
    #[serde(rename = "midi-bridge")]
    MidiBridge,
}

impl AppKey {
    /// All possible app keys
    pub fn all() -> &'static [AppKey] {
        &[
            AppKey::Voicemeeter,
            AppKey::Qlc,
            AppKey::Obs,
            AppKey::MidiBridge,
        ]
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "voicemeeter" => Some(AppKey::Voicemeeter),
            "qlc" => Some(AppKey::Qlc),
            "obs" => Some(AppKey::Obs),
            "midi-bridge" => Some(AppKey::MidiBridge),
            _ => None,
        }
    }

    /// Convert to string
    pub fn as_str(&self) -> &'static str {
        match self {
            AppKey::Voicemeeter => "voicemeeter",
            AppKey::Qlc => "qlc",
            AppKey::Obs => "obs",
            AppKey::MidiBridge => "midi-bridge",
        }
    }
}

impl std::fmt::Display for AppKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Constructs a unique key for a MIDI address (including the port)
pub fn addr_key(addr: &MidiAddr) -> String {
    format!(
        "{}|{}|{}|{}",
        addr.port_id,
        addr.status,
        addr.channel.unwrap_or(0),
        addr.data1.unwrap_or(0)
    )
}

/// Constructs a key without the port (for X-Touch shadow state)
pub fn addr_key_without_port(addr: &MidiAddr) -> String {
    format!(
        "{}|{}|{}",
        addr.status,
        addr.channel.unwrap_or(0),
        addr.data1.unwrap_or(0)
    )
}

/// Compute SHA-1 hash for binary data (SysEx)
pub fn compute_hash(data: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

