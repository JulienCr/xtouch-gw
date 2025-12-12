//! State management module - MIDI state tracking per application
//!
//! This module provides the state store that tracks MIDI state for each application
//! (Voicemeeter, QLC+, OBS, etc.). It implements anti-echo, shadow states, and
//! state persistence to avoid feedback loops and enable proper bidirectional sync.

mod builders;
mod persistence;
mod store;
mod types;

pub use builders::build_entry_from_raw;
pub use store::StateStore;
pub use types::{AppKey, MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin};
