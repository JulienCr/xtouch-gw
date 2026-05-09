//! Page availability filter based on Voicemeeter presence.
//!
//! Tracks which pages are currently navigable through prev/next based on
//! Voicemeeter presence and provides skip-aware navigation helpers.

use crate::router::voicemeeter_detector::VmState;

#[derive(Debug, Clone)]
pub struct PageAvailability {
    /// Per-page metadata: (requires_voicemeeter, auto_when_voicemeeter_absent).
    flags: Vec<PageFlags>,
    /// Last observed VM state. `None` means no detector active — all pages
    /// are considered available.
    vm_state: Option<VmState>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PageFlags {
    pub requires_voicemeeter: bool,
    pub auto_when_voicemeeter_absent: bool,
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Forward,
    Backward,
}

impl PageAvailability {
    pub fn new(flags: Vec<PageFlags>) -> Self {
        Self {
            flags,
            vm_state: None,
        }
    }

    pub fn set_vm_state(&mut self, state: Option<VmState>) {
        self.vm_state = state;
    }

    pub fn current_vm_state(&self) -> Option<VmState> {
        self.vm_state
    }

    pub fn page_count(&self) -> usize {
        self.flags.len()
    }

    pub fn is_available(&self, index: usize) -> bool {
        let Some(flags) = self.flags.get(index) else {
            return false;
        };

        match self.vm_state {
            // No detector configured — every page is available.
            None => true,
            Some(VmState::Running) => true,
            Some(VmState::Absent) => !flags.requires_voicemeeter,
        }
    }

    pub fn next_available(&self, current: usize) -> Option<usize> {
        self.find_available(current, Direction::Forward)
    }

    pub fn prev_available(&self, current: usize) -> Option<usize> {
        self.find_available(current, Direction::Backward)
    }

    fn find_available(&self, current: usize, dir: Direction) -> Option<usize> {
        let n = self.flags.len();
        if n == 0 {
            return None;
        }
        (1..=n)
            .map(|offset| match dir {
                Direction::Forward => (current + offset) % n,
                Direction::Backward => (current + n - (offset % n)) % n,
            })
            .find(|&idx| idx != current && self.is_available(idx))
    }

    /// Compute the page index to switch to when the VM state transitions.
    ///
    /// - `Running -> Absent`: returns the index of the page tagged
    ///   `auto_when_voicemeeter_absent`, if any.
    /// - `Absent -> Running`: returns the first page with
    ///   `requires_voicemeeter` set, if the user is currently on the
    ///   "absent" page (i.e. the auto-switch target). This avoids yanking the
    ///   user away from a non-VM page (e.g. a lighting page) they've manually
    ///   navigated to.
    pub fn auto_switch_target(
        &self,
        current_index: usize,
        prev: Option<VmState>,
        new: VmState,
    ) -> Option<usize> {
        match (prev, new) {
            (Some(VmState::Absent) | None, VmState::Running) => {
                let on_absent_page = self
                    .flags
                    .get(current_index)
                    .map(|f| f.auto_when_voicemeeter_absent)
                    .unwrap_or(false);
                if !on_absent_page {
                    return None;
                }
                self.flags.iter().position(|f| f.requires_voicemeeter)
            },
            (Some(VmState::Running) | None, VmState::Absent) => self
                .flags
                .iter()
                .position(|f| f.auto_when_voicemeeter_absent),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(requires: bool, auto_absent: bool) -> PageFlags {
        PageFlags {
            requires_voicemeeter: requires,
            auto_when_voicemeeter_absent: auto_absent,
        }
    }

    fn build(pages: Vec<(bool, bool)>) -> PageAvailability {
        PageAvailability::new(pages.into_iter().map(|(r, a)| flags(r, a)).collect())
    }

    #[test]
    fn all_available_when_no_state() {
        let pa = build(vec![(true, false), (false, false), (false, true)]);
        assert!(pa.is_available(0));
        assert!(pa.is_available(1));
        assert!(pa.is_available(2));
    }

    #[test]
    fn vm_running_keeps_all_available() {
        let mut pa = build(vec![(true, false), (false, false), (false, true)]);
        pa.set_vm_state(Some(VmState::Running));
        assert!(pa.is_available(0));
        assert!(pa.is_available(1));
        assert!(pa.is_available(2));
    }

    #[test]
    fn vm_absent_locks_required_pages() {
        let mut pa = build(vec![(true, false), (false, false), (false, true)]);
        pa.set_vm_state(Some(VmState::Absent));
        assert!(!pa.is_available(0));
        assert!(pa.is_available(1));
        assert!(pa.is_available(2));
    }

    #[test]
    fn next_skips_locked_pages() {
        let mut pa = build(vec![
            (true, false),
            (true, false),
            (false, false),
            (false, true),
        ]);
        pa.set_vm_state(Some(VmState::Absent));
        assert_eq!(pa.next_available(2), Some(3));
        assert_eq!(pa.next_available(3), Some(2));
    }

    #[test]
    fn prev_skips_locked_pages() {
        let mut pa = build(vec![
            (true, false),
            (false, false),
            (true, false),
            (false, true),
        ]);
        pa.set_vm_state(Some(VmState::Absent));
        assert_eq!(pa.prev_available(3), Some(1));
        assert_eq!(pa.prev_available(1), Some(3));
    }

    #[test]
    fn next_returns_none_when_no_alternative() {
        let mut pa = build(vec![(true, false), (false, true)]);
        pa.set_vm_state(Some(VmState::Absent));
        // From the only available page, next must return None or self.
        // We never return self (the "current" index is excluded).
        assert_eq!(pa.next_available(1), None);
    }

    #[test]
    fn auto_switch_running_to_absent_targets_absent_page() {
        let pa = build(vec![(true, false), (false, false), (false, true)]);
        assert_eq!(
            pa.auto_switch_target(0, Some(VmState::Running), VmState::Absent),
            Some(2)
        );
    }

    #[test]
    fn auto_switch_absent_to_running_targets_required_page_if_on_absent_page() {
        let pa = build(vec![(true, false), (false, false), (false, true)]);
        // User is on the "auto-when-absent" page (index 2). VM comes back -> jump to required page.
        assert_eq!(
            pa.auto_switch_target(2, Some(VmState::Absent), VmState::Running),
            Some(0)
        );
    }

    #[test]
    fn auto_switch_absent_to_running_no_op_if_user_navigated_away() {
        let pa = build(vec![(true, false), (false, false), (false, true)]);
        // User is on a free page (index 1) — don't yank them back.
        assert_eq!(
            pa.auto_switch_target(1, Some(VmState::Absent), VmState::Running),
            None
        );
    }

    #[test]
    fn auto_switch_initial_absent_targets_absent_page() {
        let pa = build(vec![(true, false), (false, false), (false, true)]);
        // At startup, prev is None.
        assert_eq!(pa.auto_switch_target(0, None, VmState::Absent), Some(2));
    }
}
