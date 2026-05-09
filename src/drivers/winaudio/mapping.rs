//! Map fader slots (1..=8) to concrete audio sessions.
//!
//! Two-pass resolution:
//!   1. **Pinned** apps from `WinAudioConfig::pinned_apps` claim their
//!      requested fader slot regardless of session activity.
//!   2. **Discovered** sessions fill the remaining slots in FIFO discovery
//!      order. The order is captured in `discovered_order` and is stable
//!      across re-enumerations: a session is appended the first time we
//!      see it and its position never moves, even if it temporarily
//!      disappears and re-opens later.

use std::collections::HashSet;

use crate::config::PinnedApp;

#[cfg(target_os = "windows")]
use super::session::SessionInfo;

/// Output of a single mapping pass: a concrete fader slot binding.
#[derive(Debug, Clone)]
pub struct SlotBinding {
    /// 1..=8.
    pub fader: u8,
    /// Lowercase exe name (e.g. "discord.exe"), or `None` for a pinned slot
    /// whose app isn't currently producing audio.
    pub process_name: Option<String>,
    /// Friendly LCD label.
    pub display_name: String,
}

/// Hard cap on the FIFO discovery list. Only 8 fader slots exist; the
/// extra headroom keeps a few "next in line" entries around so a session
/// that briefly closes and re-opens reclaims its slot. Beyond this, the
/// oldest entries are evicted to bound memory across long-running sessions
/// where many short-lived audio sources come and go.
const DISCOVERY_CAP: usize = 32;

#[derive(Debug, Default, Clone)]
pub struct DiscoveryState {
    /// Lowercase process names in first-seen order. Once added, names
    /// stay in the list — keeps fader assignments stable across
    /// re-enumerations. Bounded by `DISCOVERY_CAP`; oldest entries are
    /// evicted to make room for new arrivals.
    pub discovered_order: Vec<String>,
}

impl DiscoveryState {
    /// Push any newly seen process names (lowercase) to the end of the
    /// stable discovery order, skipping duplicates and pinned names.
    /// Evicts the oldest entries when the list exceeds `DISCOVERY_CAP`.
    pub fn observe(&mut self, names_lc: &[String], pinned_lc: &HashSet<String>) {
        for name in names_lc {
            if pinned_lc.contains(name) {
                continue;
            }
            if !self.discovered_order.iter().any(|s| s == name) {
                self.discovered_order.push(name.clone());
            }
        }
        if self.discovered_order.len() > DISCOVERY_CAP {
            let drop_count = self.discovered_order.len() - DISCOVERY_CAP;
            self.discovered_order.drain(0..drop_count);
        }
    }
}

/// Compute the static mapping fader_slot → app from config and current
/// discovery order. The result has at most 8 entries (one per fader).
pub fn compute_slots(pinned: &[PinnedApp], discovery: &DiscoveryState) -> Vec<SlotBinding> {
    let mut bindings: Vec<Option<SlotBinding>> = (0..8).map(|_| None).collect();

    // 1. Pinned slots take priority.
    let mut pinned_lc: HashSet<String> = HashSet::new();
    for pin in pinned {
        if !(1..=8).contains(&pin.fader) {
            continue;
        }
        let name_lc = pin.process_name.to_lowercase();
        let display = pin
            .display_name
            .clone()
            .unwrap_or_else(|| derive_label(&pin.process_name));
        bindings[(pin.fader - 1) as usize] = Some(SlotBinding {
            fader: pin.fader,
            process_name: Some(name_lc.clone()),
            display_name: display,
        });
        pinned_lc.insert(name_lc);
    }

    // 2. Discovered sessions fill remaining slots in stable order.
    let mut discovered_iter = discovery
        .discovered_order
        .iter()
        .filter(|n| !pinned_lc.contains(*n));
    for slot_idx in 0..8 {
        if bindings[slot_idx].is_some() {
            continue;
        }
        let Some(name_lc) = discovered_iter.next() else {
            break;
        };
        bindings[slot_idx] = Some(SlotBinding {
            fader: (slot_idx + 1) as u8,
            process_name: Some(name_lc.clone()),
            display_name: derive_label(name_lc),
        });
    }

    bindings.into_iter().flatten().collect()
}

/// Resolve a `pinned:N` slot to its configured process name.
pub fn pinned_target(pinned: &[PinnedApp], fader: u8) -> Option<String> {
    pinned
        .iter()
        .find(|p| p.fader == fader)
        .map(|p| p.process_name.to_lowercase())
}

/// Resolve a `discovered:N` slot to its current process name (according
/// to `discovery`), skipping pinned names.
pub fn discovered_target(
    pinned: &[PinnedApp],
    discovery: &DiscoveryState,
    slot: u8,
) -> Option<String> {
    let pinned_lc: HashSet<String> = pinned
        .iter()
        .map(|p| p.process_name.to_lowercase())
        .collect();
    discovery
        .discovered_order
        .iter()
        .filter(|n| !pinned_lc.contains(*n))
        .nth(slot as usize)
        .cloned()
}

/// Find a session info matching a process name (lowercase).
#[cfg(target_os = "windows")]
pub fn find_session<'a>(
    sessions: &'a [SessionInfo],
    process_name_lc: &str,
) -> Option<&'a SessionInfo> {
    sessions.iter().find(|s| s.process_name == process_name_lc)
}

fn derive_label(process_name: &str) -> String {
    // Drop ".exe" suffix and capitalize the first letter for display.
    let stem = process_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(process_name);
    let mut chars = stem.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PinnedApp;

    fn pin(fader: u8, name: &str) -> PinnedApp {
        PinnedApp {
            fader,
            process_name: name.to_string(),
            display_name: None,
        }
    }

    #[test]
    fn discovery_appends_in_order() {
        let mut s = DiscoveryState::default();
        let pinned = HashSet::new();
        s.observe(&["a.exe".into(), "b.exe".into()], &pinned);
        s.observe(&["b.exe".into(), "c.exe".into()], &pinned);
        assert_eq!(s.discovered_order, vec!["a.exe", "b.exe", "c.exe"]);
    }

    #[test]
    fn discovery_skips_pinned() {
        let mut s = DiscoveryState::default();
        let pinned: HashSet<String> = ["b.exe".into()].into_iter().collect();
        s.observe(&["a.exe".into(), "b.exe".into(), "c.exe".into()], &pinned);
        assert_eq!(s.discovered_order, vec!["a.exe", "c.exe"]);
    }

    #[test]
    fn pinned_keeps_their_slots() {
        let pinned = vec![pin(2, "Spotify.exe"), pin(5, "Discord.exe")];
        let discovery = DiscoveryState {
            discovered_order: vec!["firefox.exe".into()],
        };
        let bindings = compute_slots(&pinned, &discovery);
        let by_slot: std::collections::HashMap<_, _> =
            bindings.iter().map(|b| (b.fader, b.clone())).collect();
        assert_eq!(by_slot[&2].process_name.as_deref(), Some("spotify.exe"));
        assert_eq!(by_slot[&5].process_name.as_deref(), Some("discord.exe"));
        assert_eq!(by_slot[&1].process_name.as_deref(), Some("firefox.exe"));
    }

    #[test]
    fn discovery_caps_at_max_size() {
        let mut s = DiscoveryState::default();
        let pinned = HashSet::new();
        // Push 50 unique names; expect only the last DISCOVERY_CAP (=32) to remain.
        let names: Vec<String> = (0..50).map(|i| format!("app{i}.exe")).collect();
        s.observe(&names, &pinned);
        assert_eq!(s.discovered_order.len(), DISCOVERY_CAP);
        // Oldest entries (app0..app17) should be evicted; app18..app49 retained.
        assert_eq!(s.discovered_order.first().unwrap(), "app18.exe");
        assert_eq!(s.discovered_order.last().unwrap(), "app49.exe");
    }

    #[test]
    fn discovered_target_resolves_in_order() {
        let pinned = vec![pin(1, "Discord.exe")];
        let discovery = DiscoveryState {
            discovered_order: vec!["spotify.exe".into(), "firefox.exe".into()],
        };
        assert_eq!(
            discovered_target(&pinned, &discovery, 0),
            Some("spotify.exe".into())
        );
        assert_eq!(
            discovered_target(&pinned, &discovery, 1),
            Some("firefox.exe".into())
        );
        assert_eq!(discovered_target(&pinned, &discovery, 2), None);
    }
}
