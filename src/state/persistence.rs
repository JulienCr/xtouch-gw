//! State persistence - StateSnapshot type for sled-based persistence
//!
//! The StateSnapshot type is used by the PersistenceActor to serialize
//! and deserialize state to/from sled's embedded database.

use super::types::{AppKey, MidiStateEntry};
use std::collections::HashMap;

/// State snapshot for JSON serialization
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    /// Timestamp of snapshot creation (milliseconds since epoch)
    pub timestamp: u64,
    /// Version of the snapshot format
    pub version: String,
    /// State entries per application
    pub states: HashMap<AppKey, Vec<MidiStateEntry>>,
}

impl StateSnapshot {
    /// Current snapshot format version
    pub const VERSION: &'static str = "1.0.0";
}
