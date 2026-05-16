# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.8.0] - 2026-05-16

Backlog reduction sprint: 15 issues closed across 6 PRs (#46–#51).

### Added

- New `LiveEvent::ProfileLoaded { profile_name, ts }` variant emitted from `app::run_app` after driver registration completes (#36, PR #50).
- Config-load validation for winaudio session-target params (`pinned:N`, `discovered:N`); typos like `pined:1` now error at startup with page+control context instead of warning at button press (#38, PR #50).
- Auto-detection of winaudio-eligible pages by scanning `controls` for any `app == "winaudio"`; replaces the hardcoded `"Windows Audio"` page-name match so renaming the page no longer silently breaks state refresh (#39, PR #50).
- Reverse-lookup `note→control_id` and `pb→control_id` maps in `refresh_plan`, eliminating O(N×M) string-parsing scans per page-switch (~6400 `MidiSpec::parse` calls dropped to ~50 for typical configs) (#30, PR #47).
- Cached `pinned_lc` HashSet on `WinAudioDriver` via `arc-swap`; refreshed atomically on config reload (#40, PR #48).
- Cached audio-session enumeration in winaudio COM thread, invalidated by `IAudioSessionNotification::OnSessionCreated` plus a 2s TTL safety net; cached `pid → process_name_lc` to avoid repeated `OpenProcess` calls (#35, PR #48).
- Change-detection guard in `emit_and_debounce` skips no-op scene events before allocation (#31, PR #46).
- `DEFAULT_GAMEPAD_SLOT` and `GAMEPAD_PREFIX` constants in `input/gamepad/mod.rs`; unified `extract_gamepad_slot` helper deduplicating two divergent implementations (#26, #28, PR #46).
- 5 new unit tests for `extract_gamepad_slot`; 3 new tests for the refresh_plan reverse-lookup map.

### Changed

- Winaudio init no longer sleeps 800ms post-init; awaits `LiveEvent::ProfileLoaded` with a 2s `tokio::time::timeout` safety net, then refreshes state if the active page is winaudio-eligible (#36, PR #50).
- `run_event_listener` (OBS) now takes `Arc<ObsDriver>` instead of 11 individual `Arc<RwLock<T>>` parameters; `emit_and_debounce` calls `driver.emit_signal` directly (#27, PR #46).
- `run_com_loop` per-arm bodies extracted into a new `winaudio/com_handlers.rs` submodule; the match block dropped from ~85 to 49 lines (#37, PR #50).
- `compute_slots` and `discovered_target` signatures take `&HashSet<String>` instead of rebuilding the lowercased pinned set per call (#40, PR #48).
- Shared `resolve_note_control` preamble extracted from `try_cc_to_note_transform` and `try_direct_note_lookup` (#29, PR #47).

### Fixed

- Duplicate `MasterVolumeChanged` events emitted after `SetMasterVolume*` and `SetMute` operations: the OS callback (`IAudioEndpointVolumeCallback::OnNotify`) already fires per channel-change, the synthetic emit was redundant and caused 2-3 events to traverse the full feedback pipeline per master tweak (#41, PR #48).

### Removed

- 99 → 0 clippy warnings across the workspace via two passes (#24, PRs #49 + #51):
  - Dead code: `FileCache`, `FILE_DB`, `load_mappings_from_path`, `get_encoder_controls`, 9 unused `midi.rs` helpers, `analog::to_midi_cc/pb/button_to_midi` (duplicates of `crate::midi`), `persistence_tx` field in `StateActor`, dead `analog.rs` MIDI helpers.
  - Style: `.clamp()`, `or_default()`, collapsed `if let`, `Display` vs inherent `to_string`, redundant `..Default::default()`.
  - Correctness: dropped redundant `Arc<Mutex<Menu>>` in `tray/manager.rs`; documented intentional `Send/Sync` Arc patterns; allowed `too_many_arguments` on top-level `run_app`.
- `WINAUDIO_PAGE_NAME` constant (no longer needed after auto-detection).

### Internal

- Verified `obs_indicators::build_indicator_callback` already reads `router.config` behind `RwLock` per signal (lock+drop pattern at `obs_indicators.rs:42-47`); no stale-capture, no code change required (#23, PR #46).
- Test count: 222 → 242 passing (15 net new tests added across the sprint).

[1.8.0]: https://github.com/JulienCr/xtouch-gw/releases/tag/v1.8.0
