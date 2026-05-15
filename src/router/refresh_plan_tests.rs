//! Unit tests for `refresh_plan::MidiReverseMaps` (issues #29, #30).
//!
//! These tests verify that the reverse-lookup maps built by
//! `MidiReverseMaps::build` correctly index controls by MCU note and PB
//! channel, replacing the prior linear-scan + per-entry `MidiSpec::parse`
//! pattern in the page-switch hot path.

use super::refresh_plan::MidiReverseMaps;
use crate::control_mapping::{load_default_mappings, ControlMapping, ControlMappingDB};
use std::collections::HashMap;

/// Build a synthetic mapping DB with three entries covering all MidiSpec
/// variants. Avoids depending on the real CSV layout.
fn synthetic_db() -> ControlMappingDB {
    let mut mappings = HashMap::new();
    mappings.insert(
        "mute1".to_string(),
        ControlMapping {
            control_id: "mute1".to_string(),
            group: "strip".to_string(),
            ctrl_message: "cc=10".to_string(),
            mcu_message: "note=16".to_string(),
        },
    );
    mappings.insert(
        "fader1".to_string(),
        ControlMapping {
            control_id: "fader1".to_string(),
            group: "strip".to_string(),
            ctrl_message: "cc=70".to_string(),
            mcu_message: "pb=ch1".to_string(),
        },
    );
    mappings.insert(
        "vpot_cc".to_string(),
        ControlMapping {
            control_id: "vpot_cc".to_string(),
            group: "strip".to_string(),
            ctrl_message: "cc=20".to_string(),
            mcu_message: "cc=20".to_string(),
        },
    );
    ControlMappingDB {
        mappings,
        groups: HashMap::new(),
    }
}

#[test]
fn build_populates_note_and_pb_maps() {
    let db = synthetic_db();
    let maps = MidiReverseMaps::build(&db);

    // Note 16 -> "mute1"
    assert_eq!(maps.note_to_control_id.get(&16), Some(&"mute1".to_string()));
    // pb=ch1 parses to channel 0 (0-based) -> "fader1"
    assert_eq!(
        maps.pb_channel_to_control_id.get(&0),
        Some(&"fader1".to_string())
    );

    // Unmapped notes/channels are absent
    assert!(!maps.note_to_control_id.contains_key(&99));
    assert!(!maps.pb_channel_to_control_id.contains_key(&7));
}

#[test]
fn build_skips_cc_mcu_mappings() {
    // MCU CC mappings exist in the DB but are intentionally not exposed
    // via the reverse maps (no current call site looks up by CC number).
    let db = synthetic_db();
    let maps = MidiReverseMaps::build(&db);

    // Only the two non-CC entries should populate the maps.
    assert_eq!(maps.note_to_control_id.len(), 1);
    assert_eq!(maps.pb_channel_to_control_id.len(), 1);
}

#[test]
fn build_against_default_mappings_yields_expected_entries() {
    // Smoke-test against the embedded production CSV: at minimum, the
    // 8 strip faders (pb=ch1..ch8) must populate the PB map, and the
    // mute1 note must be present.
    let db = load_default_mappings().expect("default mappings should load");
    let maps = MidiReverseMaps::build(db);

    for ch in 0..8u8 {
        assert!(
            maps.pb_channel_to_control_id.contains_key(&ch),
            "expected fader for pb channel {ch}",
        );
    }
    // mute1 in MCU mode is note=16 (see docs/xtouch-matching.csv).
    assert_eq!(maps.note_to_control_id.get(&16), Some(&"mute1".to_string()));
}
