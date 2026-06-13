//! Feedback-driven toggle evaluation.
//!
//! A `toggle` on a control fires driver actions when an app's *real* feedback
//! state transitions on/off (e.g. the Voicemeeter record LED), as opposed to
//! firing on the X-Touch button press like `action`/`also` do.
//!
//! The trigger is detected in [`Router::on_midi_from_app`] (the page-independent
//! feedback entry point). Edge detection against [`Router::toggle_states`] makes
//! firing idempotent: a repeated identical state never re-triggers, and the very
//! first observation only records the state (no spurious action on connect).

use std::collections::HashMap;

use tracing::{debug, warn};

use crate::config::{ControlMapping, MidiType, ToggleConfig};
use crate::control_mapping::{load_default_mappings, MidiSpec as HwMidiSpec};
use crate::state::{MidiStateEntry, MidiStatus};

/// Synthetic Note-On (velocity 127) used to dispatch toggle steps as a "press".
///
/// `dispatch_step` filters button releases and OBS ignores trigger actions on
/// release (`ExecutionContext::is_button_release`), so the steps must look like
/// a press. The note byte is irrelevant for driver actions.
const SYNTHETIC_PRESS: [u8; 3] = [0x90, 0x00, 0x7F];

impl super::Router {
    /// React to an app feedback `entry` by firing any matching toggle's
    /// `on`/`off` steps on a state transition.
    ///
    /// Called from `on_midi_from_app` *before* anti-echo suppression: the toggle
    /// is a semantic reaction to the app's reported state, distinct from the
    /// anti-echo concern of not bouncing our own values back to the surface.
    pub(crate) async fn evaluate_feedback_toggles(&self, app_key: &str, entry: &MidiStateEntry) {
        // Snapshot the toggles whose source matches this app, then drop the
        // config lock before dispatching (dispatch_step re-reads config/drivers).
        let (toggles, is_mcu) = {
            let config = self.config.read().await;
            let is_mcu = config.is_mcu_mode();
            let active_idx = *self.active_page_index.read().await;

            let mut collected: Vec<(String, ToggleConfig)> = Vec::new();

            // Global controls first (the primary use case is a global toggle),
            // then the active page's controls.
            if let Some(global) = &config.pages_global {
                if let Some(controls) = &global.controls {
                    collect_toggles(controls, app_key, &mut collected);
                }
            }
            if let Some(page) = config.pages.get(active_idx) {
                if let Some(controls) = &page.controls {
                    collect_toggles(controls, app_key, &mut collected);
                }
            }

            (collected, is_mcu)
        };

        if toggles.is_empty() {
            return;
        }

        let now_on = entry.value.as_number().map(|n| n > 0).unwrap_or(false);

        for (control_id, toggle) in toggles {
            if !toggle_matches(&control_id, &toggle, entry, is_mcu) {
                continue;
            }

            // Atomic read-compare-update under a single write lock: if two
            // feedback messages for the same control_id are ever processed
            // concurrently, only the first observes the transition and inserts —
            // the second sees the already-updated state and no-ops, so the edge
            // action can't double-fire. Dispatch happens after the lock is released.
            let prev = {
                let mut states = self.toggle_states.write().await;
                let prev = states.get(&control_id).copied();
                if prev != Some(now_on) {
                    states.insert(control_id.clone(), now_on);
                }
                prev
            };
            if prev == Some(now_on) {
                continue; // No change → idempotent no-op.
            }

            // First observation: record the state without firing, so connecting
            // (or a config reload) never triggers an unexpected action.
            if prev.is_none() {
                debug!(
                    "Toggle '{}': initial state recorded ({})",
                    control_id,
                    if now_on { "on" } else { "off" }
                );
                continue;
            }

            let steps = if now_on { &toggle.on } else { &toggle.off };
            if steps.is_empty() {
                continue;
            }
            debug!(
                "Toggle '{}': {} → dispatching {} step(s)",
                control_id,
                if now_on { "ON" } else { "OFF" },
                steps.len()
            );

            for step in steps {
                if step.midi.is_some() {
                    warn!(
                        "Toggle '{}': `midi` steps unsupported (synthetic trigger), skipping",
                        control_id
                    );
                    continue;
                }
                self.dispatch_step(&SYNTHETIC_PRESS, &control_id, step)
                    .await;
            }
        }
    }
}

/// Collect `(control_id, toggle)` pairs whose effective source matches `app_key`.
fn collect_toggles(
    controls: &HashMap<String, ControlMapping>,
    app_key: &str,
    out: &mut Vec<(String, ToggleConfig)>,
) {
    for (id, mapping) in controls {
        if let Some(toggle) = &mapping.toggle {
            let source = toggle.source.as_deref().unwrap_or(mapping.app.as_str());
            if source == app_key {
                out.push((id.clone(), toggle.clone()));
            }
        }
    }
}

/// Does `entry` match the address watched by `toggle` for `control_id`?
///
/// Uses the explicit `watch` spec when present, otherwise derives the watched
/// address from the control's own hardware mapping (e.g. `record` → Note 95 in
/// MCU mode), which a feedback echo of that button would carry.
fn toggle_matches(
    control_id: &str,
    toggle: &ToggleConfig,
    entry: &MidiStateEntry,
    is_mcu: bool,
) -> bool {
    if let Some(watch) = &toggle.watch {
        // Channel is matched only when the watch spec pins one (MidiSpec channel
        // is 1-based, same as MidiAddr::channel).
        let channel_ok = watch
            .channel
            .map(|c| entry.addr.channel == Some(c))
            .unwrap_or(true);
        return match watch.midi_type {
            MidiType::Note => {
                entry.addr.status == MidiStatus::Note
                    && watch
                        .note
                        .map(|n| entry.addr.data1 == Some(n))
                        .unwrap_or(false)
                    && channel_ok
            },
            MidiType::Cc => {
                entry.addr.status == MidiStatus::CC
                    && watch
                        .cc
                        .map(|c| entry.addr.data1 == Some(c))
                        .unwrap_or(false)
                    && channel_ok
            },
            MidiType::Pb => entry.addr.status == MidiStatus::PB && channel_ok,
            MidiType::Passthrough => false,
        };
    }

    // Default: derive from the control's hardware address.
    match load_default_mappings()
        .ok()
        .and_then(|db| db.get_midi_spec(control_id, is_mcu))
    {
        Some(HwMidiSpec::Note { note }) => {
            entry.addr.status == MidiStatus::Note && entry.addr.data1 == Some(note)
        },
        Some(HwMidiSpec::ControlChange { cc }) => {
            entry.addr.status == MidiStatus::CC && entry.addr.data1 == Some(cc)
        },
        Some(HwMidiSpec::PitchBend { .. }) => entry.addr.status == MidiStatus::PB,
        None => false,
    }
}
