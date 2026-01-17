//! State management module - MIDI state tracking per application
//!
//! This module provides the state store that tracks MIDI state for each application
//! (Voicemeeter, QLC+, OBS, etc.). It implements anti-echo, shadow states, and
//! state persistence to avoid feedback loops and enable proper bidirectional sync.

mod actor;
mod actor_handle;
mod builders;
mod commands;
mod persistence;
pub mod persistence_actor;
mod types;

pub use actor::make_shadow_key;
pub use actor_handle::StateActorHandle;
pub use builders::build_entry_from_raw;
pub use persistence::StateSnapshot;
pub use persistence_actor::{PersistenceActorHandle, DEFAULT_DEBOUNCE_MS};
pub use types::{AppKey, MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin};
