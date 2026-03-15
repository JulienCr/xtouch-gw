//! Page refresh plan building: priority-based MIDI entry collection
//!
//! Builds ordered lists of MIDI entries to replay when switching pages.
//! Uses priority-based replacement to resolve conflicts between apps.

use crate::config::PageConfig;
use crate::control_mapping::{load_default_mappings, ControlMappingDB, MidiSpec};
use crate::state::{AppKey, MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin};
use std::collections::HashMap;
use tracing::trace;

/// Internal entry with priority for plan conflict resolution
#[derive(Clone)]
struct PlanEntry {
    entry: MidiStateEntry,
    priority: u8,
}

/// Insert entry with priority-based replacement (higher priority wins, then newer timestamp)
fn insert_prioritized<K: std::hash::Hash + Eq>(
    map: &mut HashMap<K, PlanEntry>,
    key: K,
    entry: MidiStateEntry,
    priority: u8,
) {
    let dominated = map.get(&key).is_none_or(|cur| {
        priority > cur.priority || (priority == cur.priority && entry.ts > cur.entry.ts)
    });
    if dominated {
        map.insert(key, PlanEntry { entry, priority });
    }
}

/// Generate (channel, data1) key for Note/CC plans (zero-allocation)
fn channel_data1_key(entry: &MidiStateEntry) -> (u8, u8) {
    (
        entry.addr.channel.unwrap_or(0),
        entry.addr.data1.unwrap_or(0),
    )
}

/// X-Touch fader channels: 8 strip faders + 1 master fader
const FADER_CHANNELS: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9];

impl super::Router {
    /// Plan page refresh: build ordered list of MIDI entries to send
    ///
    /// Returns entries in order: Notes -> CC -> PB
    /// Priority for each type:
    /// - PB: Known PB = 3 > Mapped CC = 2 > Zero = 1
    /// - Notes/CC: Known value = 2 > Reset (0/OFF) = 1
    pub(super) async fn plan_page_refresh(&self, page: &PageConfig) -> Vec<MidiStateEntry> {
        let mut note_plan: HashMap<(u8, u8), PlanEntry> = HashMap::new();
        let mut cc_plan: HashMap<(u8, u8), PlanEntry> = HashMap::new();
        let mut pb_plan: HashMap<u8, PlanEntry> = HashMap::new();

        // Load mapping DB once for the entire refresh plan
        let mapping_db = match load_default_mappings() {
            Ok(db) => db,
            Err(e) => {
                tracing::error!("Failed to load control mapping DB for page refresh: {}", e);
                return Vec::new();
            },
        };

        // Get apps mapped on this page (only restore state for mapped apps)
        let config = self.config.read().await;
        let apps_on_page = self.get_apps_for_page(page, &config);

        // Build plans for each app (only apps mapped on this page)
        for app in AppKey::all() {
            if !apps_on_page.contains(app.as_str()) {
                continue;
            }

            self.plan_pb_entries(page, &config, app, &mapping_db, &mut pb_plan)
                .await;
            self.plan_note_entries(page, &config, app, &mapping_db, &mut note_plan)
                .await;
            Self::plan_cc_reset_entries(app, &mut cc_plan);
        }

        // Fill missing fader channels with setpoint or zero fallback
        self.plan_fader_fallbacks(&mut pb_plan);

        // Materialize plans into ordered list: Notes → CC → PB
        Self::materialize_plans(note_plan, cc_plan, pb_plan)
    }

    /// Build PB plan entries for an app (priority: Known PB = 3 > Mapped CC→PB = 2)
    async fn plan_pb_entries(
        &self,
        page: &PageConfig,
        config: &crate::config::AppConfig,
        app: &AppKey,
        mapping_db: &ControlMappingDB,
        pb_plan: &mut HashMap<u8, PlanEntry>,
    ) {
        for &ch in FADER_CHANNELS {
            if let Some(latest_pb) = self
                .state_actor
                .get_known_latest(*app, MidiStatus::PB, Some(ch), Some(0))
                .await
            {
                // BUG-010 FIX: Only use PB state if this app owns this fader on the current page.
                if Self::is_app_mapped_to_fader(page, config, app.as_str(), ch, mapping_db) {
                    insert_prioritized(pb_plan, ch, latest_pb, 3);
                    continue;
                }
            }

            if let Some(transformed_pb) = self
                .try_cc_to_pb_transform(page, config, app, ch, mapping_db)
                .await
            {
                trace!(
                    "  Adding CC->PB to plan: ch={} value={:?}",
                    ch,
                    transformed_pb.value
                );
                insert_prioritized(pb_plan, ch, transformed_pb, 2);
            }
        }
    }

    /// Build Note plan entries for an app (CC→Note transform, direct lookup, or reset)
    async fn plan_note_entries(
        &self,
        page: &PageConfig,
        config: &crate::config::AppConfig,
        app: &AppKey,
        mapping_db: &ControlMappingDB,
        note_plan: &mut HashMap<(u8, u8), PlanEntry>,
    ) {
        for note in 0..=31 {
            if let Some(transformed_note) = self
                .try_cc_to_note_transform(page, config, app, note, mapping_db)
                .await
            {
                let key = channel_data1_key(&transformed_note);
                insert_prioritized(note_plan, key, transformed_note, 2);
                continue;
            }

            if let Some(note_entry) = self
                .try_direct_note_lookup(page, config, app, note, mapping_db)
                .await
            {
                let key = channel_data1_key(&note_entry);
                insert_prioritized(note_plan, key, note_entry, 2);
                continue;
            }

            let off = Self::make_reset_entry(app.as_str(), MidiStatus::Note, 1, note);
            let key = channel_data1_key(&off);
            insert_prioritized(note_plan, key, off, 1);
        }
    }

    /// Build CC reset entries (always send 0 to clear previous page)
    fn plan_cc_reset_entries(app: &AppKey, cc_plan: &mut HashMap<(u8, u8), PlanEntry>) {
        for &ch in FADER_CHANNELS {
            for cc in 0..=31 {
                let zero = Self::make_reset_entry(app.as_str(), MidiStatus::CC, ch, cc);
                let key = channel_data1_key(&zero);
                insert_prioritized(cc_plan, key, zero, 1);
            }
        }
    }

    /// Fill missing fader channels with setpoint or zero fallback
    fn plan_fader_fallbacks(&self, pb_plan: &mut HashMap<u8, PlanEntry>) {
        for &ch in FADER_CHANNELS {
            if pb_plan.contains_key(&ch) {
                continue;
            }

            if let Some(desired14) = self.fader_setpoint.get_desired(ch) {
                let setpoint_pb = MidiStateEntry {
                    addr: MidiAddr {
                        port_id: "xtouch".to_string(),
                        status: MidiStatus::PB,
                        channel: Some(ch),
                        data1: Some(0),
                    },
                    value: MidiValue::Number(desired14),
                    ts: Self::now_ms(),
                    origin: Origin::XTouch,
                    known: false,
                    stale: false,
                    hash: None,
                };
                insert_prioritized(pb_plan, ch, setpoint_pb, 2);
                continue;
            }

            let zero_pb = Self::make_reset_entry("xtouch", MidiStatus::PB, ch, 0);
            insert_prioritized(pb_plan, ch, zero_pb, 1);
        }
    }

    /// Materialize plans into ordered list: Notes → CC → PB
    fn materialize_plans(
        note_plan: HashMap<(u8, u8), PlanEntry>,
        cc_plan: HashMap<(u8, u8), PlanEntry>,
        pb_plan: HashMap<u8, PlanEntry>,
    ) -> Vec<MidiStateEntry> {
        let mut entries = Vec::new();
        for plan_entry in note_plan.values() {
            entries.push(plan_entry.entry.clone());
        }
        for plan_entry in cc_plan.values() {
            entries.push(plan_entry.entry.clone());
        }
        for plan_entry in pb_plan.values() {
            entries.push(plan_entry.entry.clone());
        }
        entries
    }

    /// Query CC value from StateActor for a control mapping
    async fn get_cc_value_for_control(
        &self,
        app: &AppKey,
        control_config: &crate::config::ControlMapping,
    ) -> Option<(MidiStateEntry, u8)> {
        let midi_spec = control_config.midi.as_ref()?;

        if !matches!(midi_spec.midi_type, crate::config::MidiType::Cc) {
            return None;
        }

        let cc_channel = midi_spec.channel?;
        let cc_num = midi_spec.cc?;

        let cc_entry = self
            .state_actor
            .get_known_latest(*app, MidiStatus::CC, Some(cc_channel), Some(cc_num))
            .await?;

        let cc_value = cc_entry.value.as_number()? as u8;
        Some((cc_entry, cc_value))
    }

    /// Try to transform CC value to PB for page refresh (reverse transformation)
    async fn try_cc_to_pb_transform(
        &self,
        page: &PageConfig,
        global_config: &crate::config::AppConfig,
        app: &AppKey,
        pb_channel: u8,
        mapping_db: &ControlMappingDB,
    ) -> Option<MidiStateEntry> {
        let control_id = Self::find_control_by_midi_spec(
            mapping_db,
            |spec| matches!(spec, MidiSpec::PitchBend { channel } if *channel == pb_channel.saturating_sub(1)),
        )?;

        trace!(
            "CC->PB transform: control={} pb_channel={}",
            control_id,
            pb_channel
        );

        let control_config = Self::get_control_config(page, global_config, &control_id)?;

        if control_config.app != app.as_str() {
            return None;
        }

        let (cc_entry, cc_value) = self.get_cc_value_for_control(app, &control_config).await?;
        let pb_value = crate::midi::convert::to_14bit(cc_value);

        trace!(
            "  Transform CC {} -> PB {} (0x{:04X})",
            cc_value,
            pb_value,
            pb_value
        );

        Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: app.as_str().to_string(),
                status: MidiStatus::PB,
                channel: Some(pb_channel),
                data1: Some(0),
            },
            value: MidiValue::Number(pb_value),
            ts: cc_entry.ts,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        })
    }

    /// Try to transform CC value to Note for page refresh (reverse transformation)
    async fn try_cc_to_note_transform(
        &self,
        page: &PageConfig,
        global_config: &crate::config::AppConfig,
        app: &AppKey,
        note: u8,
        mapping_db: &ControlMappingDB,
    ) -> Option<MidiStateEntry> {
        let control_id = Self::find_control_by_midi_spec(
            mapping_db,
            |spec| matches!(spec, MidiSpec::Note { note: n } if *n == note),
        )?;

        let control_config = Self::get_control_config(page, global_config, &control_id)?;

        if control_config.app != app.as_str() {
            return None;
        }

        let (cc_entry, cc_value) = self.get_cc_value_for_control(app, &control_config).await?;
        let velocity = if cc_value > 0 { 127 } else { 0 };

        trace!(
            "CC->Note transform: {} CC {} -> Note {} velocity {}",
            control_id,
            cc_value,
            note,
            velocity
        );

        Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: app.as_str().to_string(),
                status: MidiStatus::Note,
                channel: Some(1),
                data1: Some(note),
            },
            value: MidiValue::Number(velocity as u16),
            ts: cc_entry.ts,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        })
    }

    /// Direct Note state lookup for controls mapped on the current page
    async fn try_direct_note_lookup(
        &self,
        page: &PageConfig,
        global_config: &crate::config::AppConfig,
        app: &AppKey,
        note: u8,
        mapping_db: &ControlMappingDB,
    ) -> Option<MidiStateEntry> {
        let control_id = Self::find_control_by_midi_spec(
            mapping_db,
            |spec| matches!(spec, MidiSpec::Note { note: n } if *n == note),
        )?;

        let control_config = Self::get_control_config(page, global_config, &control_id)?;

        if control_config.app != app.as_str() {
            return None;
        }

        self.state_actor
            .get_known_latest(*app, MidiStatus::Note, Some(1), Some(note))
            .await
    }
}
