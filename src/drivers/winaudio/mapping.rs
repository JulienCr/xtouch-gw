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
//!
//! Each observed process is also assigned a stable LCD color from a
//! 1..=7 cycle the first time it is seen, so a strip's color survives
//! the app being closed and re-opened. The `active` set tracks which
//! sessions are currently producing audio — slots whose process is not
//! active render a black, empty strip.

use std::collections::{HashMap, HashSet};

use crate::config::PinnedApp;

#[cfg(target_os = "windows")]
use super::session::SessionInfo;

/// Output of a single mapping pass: a concrete fader slot binding.
///
/// Currently only consumed by `compute_slots` tests — the production LCD
/// render path resolves per-fader on demand via `pinned_target` /
/// `discovered_target` / `derive_label` to avoid an extra Vec allocation.
/// Kept under `#[cfg(test)]` until a non-test caller materializes.
#[cfg(test)]
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

/// Number of distinct cycle colors (1..=7, skipping 0=black).
const COLOR_CYCLE_LEN: u8 = 7;

#[derive(Debug, Default, Clone)]
pub struct DiscoveryState {
    /// Lowercase process names in first-seen order. Once added, names
    /// stay in the list — keeps fader assignments stable across
    /// re-enumerations. Bounded by `DISCOVERY_CAP`; oldest entries are
    /// evicted to make room for new arrivals.
    pub discovered_order: Vec<String>,
    /// Process name (lowercase) -> LCD color (1..=7), assigned on first
    /// observation. Stable across the app being closed and re-opened.
    /// Pinned process names are tracked here too so the cycle stays
    /// consistent across pinned + discovered slots.
    pub assigned_color: HashMap<String, u8>,
    /// Process names (lowercase) currently producing audio. Replaced
    /// wholesale on each `set_active` call; LCD slots whose name is not
    /// in this set render as blank/black.
    pub active: HashSet<String>,
}

impl DiscoveryState {
    /// Push any newly seen process names (lowercase) to the end of the
    /// stable discovery order, skipping duplicates and pinned names, and
    /// assign a cycle color to any name that doesn't already have one.
    /// Evicts the oldest entries when the list exceeds `DISCOVERY_CAP`,
    /// removing their color assignment in lockstep.
    pub fn observe(&mut self, names_lc: &[String], pinned_lc: &HashSet<String>) {
        for name in names_lc {
            if pinned_lc.contains(name) {
                continue;
            }
            if !self.discovered_order.iter().any(|s| s == name) {
                self.discovered_order.push(name.clone());
                self.ensure_color(name);
            }
        }
        if self.discovered_order.len() > DISCOVERY_CAP {
            let drop_count = self.discovered_order.len() - DISCOVERY_CAP;
            let evicted: Vec<String> = self.discovered_order.drain(0..drop_count).collect();
            for name in &evicted {
                self.assigned_color.remove(name);
            }
        }
    }

    /// Assign cycle colors to pinned process names so they share the
    /// same cycle as discovered apps. Called once at startup with the
    /// list of pinned process names (lowercase). Skips names that
    /// already have a color (e.g. from a previous call).
    pub fn observe_pinned(&mut self, names_lc: &[String]) {
        for name in names_lc {
            self.ensure_color(name);
        }
    }

    /// Replace the active session set wholesale. Called on every
    /// `ActiveSessionsChanged` event from the COM thread. Returns
    /// `true` if the set actually changed; callers use this to skip
    /// downstream re-renders when the COM thread fires a redundant
    /// "changed" event during burst session-disconnect cascades.
    pub fn set_active(&mut self, names_lc: &[String]) -> bool {
        let new_set: HashSet<String> = names_lc.iter().cloned().collect();
        if new_set == self.active {
            return false;
        }
        self.active = new_set;
        true
    }

    /// Look up the cycle color for a process name, or `None` if it has
    /// not been observed yet.
    pub fn color_for(&self, name_lc: &str) -> Option<u8> {
        self.assigned_color.get(name_lc).copied()
    }

    /// True if the named process currently has an active audio session.
    pub fn is_active(&self, name_lc: &str) -> bool {
        self.active.contains(name_lc)
    }

    /// Assign the next color in the cycle if the name is unknown.
    fn ensure_color(&mut self, name_lc: &str) {
        if self.assigned_color.contains_key(name_lc) {
            return;
        }
        let next = (self.assigned_color.len() as u8 % COLOR_CYCLE_LEN) + 1;
        self.assigned_color.insert(name_lc.to_string(), next);
    }
}

/// Compute the static mapping fader_slot → app from config and current
/// discovery order. `pinned_lc` must be the lowercased set of pinned
/// process names — owned by the caller (cached on `WinAudioDriver`) so
/// hot-path callers don't rebuild it per event.
/// The result has at most 8 entries (one per fader).
///
/// Currently only consumed by tests; production code resolves per-fader
/// on demand to avoid the intermediate Vec. Kept here so the mapping
/// invariants stay covered by unit tests.
#[cfg(test)]
pub fn compute_slots(
    pinned: &[PinnedApp],
    pinned_lc: &HashSet<String>,
    discovery: &DiscoveryState,
) -> Vec<SlotBinding> {
    let mut bindings: Vec<Option<SlotBinding>> = (0..8).map(|_| None).collect();

    // 1. Pinned slots take priority.
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
            process_name: Some(name_lc),
            display_name: display,
        });
    }

    // 2. Discovered sessions fill remaining slots in stable order.
    let mut discovered_iter = discovery
        .discovered_order
        .iter()
        .filter(|n| !pinned_lc.contains(*n));
    for (slot_idx, slot) in bindings.iter_mut().enumerate().take(8) {
        if slot.is_some() {
            continue;
        }
        let Some(name_lc) = discovered_iter.next() else {
            break;
        };
        *slot = Some(SlotBinding {
            fader: (slot_idx + 1) as u8,
            process_name: Some(name_lc.clone()),
            display_name: derive_label(name_lc),
        });
    }

    bindings.into_iter().flatten().collect()
}

/// Build the lowercased pinned process-name set from a `PinnedApp` slice.
/// Used by `WinAudioDriver` to refresh its cached set on init / reload.
pub fn pinned_lc_set(pinned: &[PinnedApp]) -> HashSet<String> {
    pinned
        .iter()
        .map(|p| p.process_name.to_lowercase())
        .collect()
}

/// Resolve a `pinned:N` slot to its configured process name.
pub fn pinned_target(pinned: &[PinnedApp], fader: u8) -> Option<String> {
    pinned
        .iter()
        .find(|p| p.fader == fader)
        .map(|p| p.process_name.to_lowercase())
}

/// Reverse of [`pinned_target`] / [`discovered_target`]: given a session's
/// process name, return the YAML target string (`"pinned:N"` or
/// `"discovered:N"`) that pages bind to. `pinned_lc` is the cached
/// lowercased pinned set (see `pinned_lc_set`). Returns `None` if the
/// session is neither pinned nor in the discovery FIFO.
pub fn target_for_process(
    pinned: &[PinnedApp],
    pinned_lc: &HashSet<String>,
    discovery: &DiscoveryState,
    process_name_lc: &str,
) -> Option<String> {
    if let Some(pin) = pinned
        .iter()
        .find(|p| p.process_name.to_lowercase() == process_name_lc)
    {
        return Some(format!("pinned:{}", pin.fader));
    }
    discovery
        .discovered_order
        .iter()
        .filter(|n| !pinned_lc.contains(*n))
        .position(|n| n == process_name_lc)
        .map(|idx| format!("discovered:{}", idx))
}

/// Resolve a `discovered:N` slot to its current process name (according
/// to `discovery`), skipping pinned names. `pinned_lc` is the cached
/// lowercased pinned set (see `pinned_lc_set`).
pub fn discovered_target(
    pinned_lc: &HashSet<String>,
    discovery: &DiscoveryState,
    slot: u8,
) -> Option<String> {
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

pub fn derive_label(process_name: &str) -> String {
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
            color: None,
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
        let pinned_lc = pinned_lc_set(&pinned);
        let discovery = DiscoveryState {
            discovered_order: vec!["firefox.exe".into()],
            ..Default::default()
        };
        let bindings = compute_slots(&pinned, &pinned_lc, &discovery);
        let by_slot: std::collections::HashMap<_, _> =
            bindings.iter().map(|b| (b.fader, b.clone())).collect();
        assert_eq!(by_slot[&2].process_name.as_deref(), Some("spotify.exe"));
        assert_eq!(by_slot[&5].process_name.as_deref(), Some("discord.exe"));
        assert_eq!(by_slot[&1].process_name.as_deref(), Some("firefox.exe"));
    }

    #[test]
    fn pinned_lc_set_lowercases_names() {
        let pinned = vec![pin(1, "Discord.exe"), pin(2, "SPOTIFY.exe")];
        let set = pinned_lc_set(&pinned);
        assert!(set.contains("discord.exe"));
        assert!(set.contains("spotify.exe"));
        assert!(!set.contains("Discord.exe"));
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
    fn target_for_process_resolves_pinned_and_discovered() {
        let pinned = vec![pin(1, "Discord.exe"), pin(3, "Spotify.exe")];
        let pinned_lc = pinned_lc_set(&pinned);
        let discovery = DiscoveryState {
            discovered_order: vec!["firefox.exe".into(), "msedge.exe".into()],
            ..Default::default()
        };
        assert_eq!(
            target_for_process(&pinned, &pinned_lc, &discovery, "discord.exe"),
            Some("pinned:1".into())
        );
        assert_eq!(
            target_for_process(&pinned, &pinned_lc, &discovery, "spotify.exe"),
            Some("pinned:3".into())
        );
        assert_eq!(
            target_for_process(&pinned, &pinned_lc, &discovery, "firefox.exe"),
            Some("discovered:0".into())
        );
        assert_eq!(
            target_for_process(&pinned, &pinned_lc, &discovery, "msedge.exe"),
            Some("discovered:1".into())
        );
        assert_eq!(
            target_for_process(&pinned, &pinned_lc, &discovery, "unknown.exe"),
            None
        );
    }

    #[test]
    fn discovered_target_resolves_in_order() {
        let pinned = vec![pin(1, "Discord.exe")];
        let pinned_lc = pinned_lc_set(&pinned);
        let discovery = DiscoveryState {
            discovered_order: vec!["spotify.exe".into(), "firefox.exe".into()],
            ..Default::default()
        };
        assert_eq!(
            discovered_target(&pinned_lc, &discovery, 0),
            Some("spotify.exe".into())
        );
        assert_eq!(
            discovered_target(&pinned_lc, &discovery, 1),
            Some("firefox.exe".into())
        );
        assert_eq!(discovered_target(&pinned_lc, &discovery, 2), None);
    }

    #[test]
    fn observe_assigns_cycle_colors_in_order() {
        let mut s = DiscoveryState::default();
        let pinned = HashSet::new();
        s.observe(&["a.exe".into(), "b.exe".into(), "c.exe".into()], &pinned);
        assert_eq!(s.color_for("a.exe"), Some(1));
        assert_eq!(s.color_for("b.exe"), Some(2));
        assert_eq!(s.color_for("c.exe"), Some(3));
    }

    #[test]
    fn observe_color_cycle_wraps_after_seven() {
        let mut s = DiscoveryState::default();
        let pinned = HashSet::new();
        let names: Vec<String> = (0..9).map(|i| format!("app{i}.exe")).collect();
        s.observe(&names, &pinned);
        assert_eq!(s.color_for("app0.exe"), Some(1));
        assert_eq!(s.color_for("app6.exe"), Some(7));
        // Wraps: 8th name lands back on color 1.
        assert_eq!(s.color_for("app7.exe"), Some(1));
        assert_eq!(s.color_for("app8.exe"), Some(2));
    }

    #[test]
    fn observe_color_stable_across_repeats() {
        let mut s = DiscoveryState::default();
        let pinned = HashSet::new();
        s.observe(&["a.exe".into()], &pinned);
        let c1 = s.color_for("a.exe");
        s.observe(&["b.exe".into(), "a.exe".into()], &pinned);
        assert_eq!(
            s.color_for("a.exe"),
            c1,
            "color must be stable for re-observations"
        );
    }

    #[test]
    fn observe_pinned_assigns_colors_via_same_cycle() {
        let mut s = DiscoveryState::default();
        s.observe_pinned(&["discord.exe".into(), "spotify.exe".into()]);
        assert_eq!(s.color_for("discord.exe"), Some(1));
        assert_eq!(s.color_for("spotify.exe"), Some(2));
        // Subsequent discovered apps continue the cycle.
        let pinned: HashSet<String> = ["discord.exe".into(), "spotify.exe".into()]
            .into_iter()
            .collect();
        s.observe(&["firefox.exe".into()], &pinned);
        assert_eq!(s.color_for("firefox.exe"), Some(3));
    }

    #[test]
    fn eviction_removes_assigned_color() {
        let mut s = DiscoveryState::default();
        let pinned = HashSet::new();
        let names: Vec<String> = (0..50).map(|i| format!("app{i}.exe")).collect();
        s.observe(&names, &pinned);
        // The 18 evicted entries (app0..app17) lose their color assignment.
        for i in 0..18 {
            let name = format!("app{i}.exe");
            assert!(
                s.color_for(&name).is_none(),
                "expected {name} to have lost its color after eviction"
            );
        }
        // The retained 32 entries still have colors.
        for i in 18..50 {
            let name = format!("app{i}.exe");
            assert!(
                s.color_for(&name).is_some(),
                "expected {name} to still have a color"
            );
        }
    }

    #[test]
    fn set_active_replaces_set_wholesale() {
        let mut s = DiscoveryState::default();
        assert!(s.set_active(&["a.exe".into(), "b.exe".into()]));
        assert!(s.is_active("a.exe"));
        assert!(s.is_active("b.exe"));
        assert!(!s.is_active("c.exe"));
        assert!(s.set_active(&["c.exe".into()]));
        assert!(!s.is_active("a.exe"));
        assert!(!s.is_active("b.exe"));
        assert!(s.is_active("c.exe"));
    }

    #[test]
    fn set_active_returns_false_when_unchanged() {
        let mut s = DiscoveryState::default();
        assert!(s.set_active(&["a.exe".into(), "b.exe".into()]));
        // Same names, different order — set equality holds.
        assert!(!s.set_active(&["b.exe".into(), "a.exe".into()]));
        // Adding a new name flips back to true.
        assert!(s.set_active(&["a.exe".into(), "b.exe".into(), "c.exe".into()]));
    }

    #[test]
    fn discovered_target_indexes_discovery_filtered_of_pinned() {
        let pinned = vec![pin(1, "Discord.exe")];
        let pinned_lc = pinned_lc_set(&pinned);
        let discovery = DiscoveryState {
            discovered_order: vec![
                "spotify.exe".into(),
                "firefox.exe".into(),
                "steam.exe".into(),
            ],
            ..Default::default()
        };
        assert_eq!(
            discovered_target(&pinned_lc, &discovery, 0),
            Some("spotify.exe".into())
        );
        assert_eq!(
            discovered_target(&pinned_lc, &discovery, 1),
            Some("firefox.exe".into())
        );
        assert_eq!(
            discovered_target(&pinned_lc, &discovery, 2),
            Some("steam.exe".into())
        );
        assert_eq!(discovered_target(&pinned_lc, &discovery, 3), None);
    }
}
