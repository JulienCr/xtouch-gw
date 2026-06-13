//! Tests for Router module

use super::*;
use crate::config::{ControlMapping, MidiConfig, PageConfig};
use crate::drivers::ConsoleDriver;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Counter for unique test database paths
static TEST_DB_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Create a unique database path for each test to avoid lock conflicts
fn make_test_db_path() -> String {
    let id = TEST_DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!(".state/test_sled_{}", id)
}

fn make_test_config(pages: Vec<PageConfig>) -> AppConfig {
    AppConfig {
        midi: MidiConfig {
            input_port: "test_in".to_string(),
            output_port: "test_out".to_string(),
            apps: None,
        },
        obs: None,
        xtouch: None,
        paging: None,
        gamepad: None,
        pages_global: None,
        winaudio: None,
        pages,
        tray: None,
    }
}

fn make_test_page(name: &str) -> PageConfig {
    PageConfig {
        name: name.to_string(),
        ..PageConfig::default()
    }
}

/// Create a Router with a unique test database path
fn make_test_router(config: AppConfig) -> Router {
    Router::with_db_path(config, &make_test_db_path()).expect("Failed to create test router")
}

#[tokio::test]
async fn test_page_navigation() {
    let config = make_test_config(vec![
        make_test_page("Page 1"),
        make_test_page("Page 2"),
        make_test_page("Page 3"),
    ]);

    let router = make_test_router(config);

    assert_eq!(router.get_active_page_name().await, "Page 1");

    router.next_page().await;
    assert_eq!(router.get_active_page_name().await, "Page 2");

    router.next_page().await;
    assert_eq!(router.get_active_page_name().await, "Page 3");

    router.next_page().await; // Wrap around
    assert_eq!(router.get_active_page_name().await, "Page 1");

    router.prev_page().await; // Wrap around backwards
    assert_eq!(router.get_active_page_name().await, "Page 3");
}

#[tokio::test]
async fn test_set_page_by_name() {
    let config = make_test_config(vec![make_test_page("Voicemeeter"), make_test_page("OBS")]);

    let router = make_test_router(config);

    router.set_active_page("OBS").await.unwrap();
    assert_eq!(router.get_active_page_name().await, "OBS");

    router.set_active_page("voicemeeter").await.unwrap(); // Case insensitive
    assert_eq!(router.get_active_page_name().await, "Voicemeeter");
}

#[tokio::test]
async fn test_set_page_by_index() {
    let config = make_test_config(vec![make_test_page("Page 0"), make_test_page("Page 1")]);

    let router = make_test_router(config);

    router.set_active_page("1").await.unwrap();
    assert_eq!(router.get_active_page_name().await, "Page 1");

    router.set_active_page("0").await.unwrap();
    assert_eq!(router.get_active_page_name().await, "Page 0");
}

#[tokio::test]
async fn test_midi_note_navigation() {
    let config = make_test_config(vec![
        make_test_page("Page 1"),
        make_test_page("Page 2"),
        make_test_page("Page 3"),
    ]);

    let router = make_test_router(config);

    // Test next page (note 47 on channel 1)
    let note_on_next = [0x90, 47, 127]; // Note On, Ch1, note 47, velocity 127
    router.on_midi_from_xtouch(&note_on_next).await;
    assert_eq!(router.get_active_page_name().await, "Page 2");

    // Test prev page (note 46 on channel 1)
    let note_on_prev = [0x90, 46, 127]; // Note On, Ch1, note 46, velocity 127
    router.on_midi_from_xtouch(&note_on_prev).await;
    assert_eq!(router.get_active_page_name().await, "Page 1");

    // Test F-key direct access (F3 = note 56 = page index 2)
    let note_on_f3 = [0x90, 56, 127]; // Note On, Ch1, note 56 (F3)
    router.on_midi_from_xtouch(&note_on_f3).await;
    assert_eq!(router.get_active_page_name().await, "Page 3");
}

#[tokio::test]
async fn test_midi_note_navigation_ignores_velocity_zero() {
    let config = make_test_config(vec![make_test_page("Page 1"), make_test_page("Page 2")]);

    let router = make_test_router(config);

    // Note Off (velocity 0) should be ignored
    let note_off = [0x90, 47, 0]; // Note On with velocity 0 = Note Off
    router.on_midi_from_xtouch(&note_off).await;
    assert_eq!(router.get_active_page_name().await, "Page 1"); // Should stay on Page 1
}

#[tokio::test]
async fn test_midi_note_off_does_not_double_fire_driver_action() {
    // Regression for the X-Touch double-fire bug: pressing a button mapped to
    // a driver action used to execute the action twice — once on Note On
    // (press) and again on Note Off (release). Drive the full
    // on_midi_from_xtouch -> handle_driver_action_mode path with a real
    // mapping and assert only the press fires.
    //
    // mute1 in MCU mode is note=16 (see docs/xtouch-matching.csv), which
    // is outside the paging note ranges so it falls through to driver mode.
    let mut page = make_test_page("Page 1");
    let mut controls = HashMap::new();
    controls.insert(
        "mute1".to_string(),
        ControlMapping {
            app: "test_console".to_string(),
            action: Some("trigger".to_string()),
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );
    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = make_test_router(config);

    let driver = Arc::new(ConsoleDriver::new("test_console"));
    router
        .register_driver("test_console".to_string(), driver.clone())
        .await
        .unwrap();

    // Press: Note On, ch1, note 16, velocity 127 -> action executes once.
    router.on_midi_from_xtouch(&[0x90, 16, 127]).await;
    assert_eq!(driver.execution_count().await, 1);

    // Release: real Note Off (0x80) must NOT re-execute the action.
    router.on_midi_from_xtouch(&[0x80, 16, 0]).await;
    assert_eq!(driver.execution_count().await, 1);

    // And the legacy "Note On with velocity 0" release form is also ignored.
    router.on_midi_from_xtouch(&[0x90, 16, 0]).await;
    assert_eq!(driver.execution_count().await, 1);
}

// ===== PHASE 4: Driver Framework Integration Tests =====

#[tokio::test]
async fn test_driver_registration_and_initialization() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    // Create and register a console driver
    let driver = Arc::new(ConsoleDriver::new("test_console"));
    let result = router
        .register_driver("test_console".to_string(), driver)
        .await;

    assert!(result.is_ok());

    // Verify driver is registered
    let driver_names = router.list_drivers().await;
    assert!(driver_names.contains(&"test_console".to_string()));

    // Verify we can retrieve the driver
    let retrieved = router.get_driver("test_console").await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name(), "test_console");
}

#[tokio::test]
async fn test_driver_shutdown_all() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    // Register multiple drivers
    router
        .register_driver(
            "driver1".to_string(),
            Arc::new(ConsoleDriver::new("driver1")),
        )
        .await
        .unwrap();

    router
        .register_driver(
            "driver2".to_string(),
            Arc::new(ConsoleDriver::new("driver2")),
        )
        .await
        .unwrap();

    // Verify they're registered
    assert_eq!(router.list_drivers().await.len(), 2);

    // Shutdown all
    let result = router.shutdown_all_drivers().await;
    assert!(result.is_ok());

    // Verify all drivers are removed
    assert_eq!(router.list_drivers().await.len(), 0);
}

#[tokio::test]
async fn test_driver_hot_reload_config() {
    let initial_config = make_test_config(vec![make_test_page("Page 1"), make_test_page("Page 2")]);

    let router = Router::new(initial_config).expect("Failed to create router");

    // Register a driver
    router
        .register_driver(
            "test_driver".to_string(),
            Arc::new(ConsoleDriver::new("test_driver")),
        )
        .await
        .unwrap();

    // Update config with different pages. Note: the new config does NOT
    // reference `test_driver` from any page or passthrough. Per audit #55,
    // `update_config` purges drivers no longer referenced, so we expect the
    // driver to be unregistered.
    let new_config = make_test_config(vec![
        make_test_page("New Page 1"),
        make_test_page("New Page 2"),
        make_test_page("New Page 3"),
    ]);

    let result = router.update_config(new_config).await;
    assert!(result.is_ok());

    // Verify new config is active
    let pages = router.list_pages().await;
    assert_eq!(pages.len(), 3);
    assert!(pages.contains(&"New Page 1".to_string()));
    assert!(pages.contains(&"New Page 3".to_string()));

    // Driver should be unregistered now that the new config doesn't reference it.
    // Before #55 this only happened via `app.rs::prune_unused_drivers`, which
    // bypassed any direct caller of `update_config` (REPL, tests, future APIs).
    assert!(
        router.get_driver("test_driver").await.is_none(),
        "drivers not referenced by the new config must be purged on update_config"
    );
}

#[tokio::test]
async fn test_driver_execution_with_context() {
    // Create a page with control mappings
    let mut page = make_test_page("Test Page");
    let mut controls = HashMap::new();
    controls.insert(
        "fader1".to_string(),
        ControlMapping {
            app: "test_console".to_string(),
            action: Some("set_volume".to_string()),
            params: Some(vec![json!(100)]),
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );
    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = make_test_router(config);

    // Register driver
    router
        .register_driver(
            "test_console".to_string(),
            Arc::new(ConsoleDriver::new("test_console")),
        )
        .await
        .unwrap();

    // Execute control action
    let result = router
        .handle_control("fader1", Some(json!(127)), None)
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_driver_execution_missing_driver() {
    // Create a page with control mapping pointing to non-existent driver
    let mut page = make_test_page("Test Page");
    let mut controls = HashMap::new();
    controls.insert(
        "fader1".to_string(),
        ControlMapping {
            app: "missing_driver".to_string(),
            action: Some("test_action".to_string()),
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );
    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = make_test_router(config);

    // Attempt to execute control action (should fail)
    let result = router
        .handle_control("fader1", Some(json!(127)), None)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Driver 'missing_driver' not registered"));
}

#[tokio::test]
async fn test_driver_execution_missing_control() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    // Register driver
    router
        .register_driver(
            "test_console".to_string(),
            Arc::new(ConsoleDriver::new("test_console")),
        )
        .await
        .unwrap();

    // Attempt to execute non-existent control
    let result = router
        .handle_control("non_existent_control", Some(json!(127)), None)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("No mapping for control"));
}

#[tokio::test]
async fn test_multiple_drivers_execution() {
    // Create a page with multiple control mappings to different drivers
    let mut page = make_test_page("Multi Driver Page");
    let mut controls = HashMap::new();

    controls.insert(
        "obs_control".to_string(),
        ControlMapping {
            app: "obs_driver".to_string(),
            action: Some("switch_scene".to_string()),
            params: Some(vec![json!("Scene 1")]),
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );

    controls.insert(
        "vm_control".to_string(),
        ControlMapping {
            app: "vm_driver".to_string(),
            action: Some("set_fader".to_string()),
            params: Some(vec![json!(1), json!(0.5)]),
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );

    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = make_test_router(config);

    // Register multiple drivers
    router
        .register_driver(
            "obs_driver".to_string(),
            Arc::new(ConsoleDriver::new("obs_driver")),
        )
        .await
        .unwrap();

    router
        .register_driver(
            "vm_driver".to_string(),
            Arc::new(ConsoleDriver::new("vm_driver")),
        )
        .await
        .unwrap();

    // Execute controls for different drivers
    let result1 = router.handle_control("obs_control", None, None).await;
    assert!(result1.is_ok());

    let result2 = router
        .handle_control("vm_control", Some(json!(64)), None)
        .await;
    assert!(result2.is_ok());

    // Verify both drivers are still registered
    assert_eq!(router.list_drivers().await.len(), 2);
}

// ===== BUG-006: Page Epoch Tests =====

#[tokio::test]
async fn test_page_epoch_increments_on_page_change() {
    let config = make_test_config(vec![
        make_test_page("Page 1"),
        make_test_page("Page 2"),
        make_test_page("Page 3"),
    ]);

    let router = make_test_router(config);

    // Initial epoch should be 0
    let initial_epoch = router.get_page_epoch();
    assert_eq!(initial_epoch, 0);

    // After refresh_page (called by next_page), epoch should increment
    router.next_page().await;
    let epoch_after_next = router.get_page_epoch();
    assert_eq!(epoch_after_next, 1);

    // Another page change increments again
    router.next_page().await;
    assert_eq!(router.get_page_epoch(), 2);

    // prev_page also increments
    router.prev_page().await;
    assert_eq!(router.get_page_epoch(), 3);
}

#[tokio::test]
async fn test_page_epoch_is_epoch_current() {
    let config = make_test_config(vec![make_test_page("Page 1"), make_test_page("Page 2")]);

    let router = make_test_router(config);

    // Capture current epoch
    let captured = router.get_page_epoch();
    assert!(router.is_epoch_current(captured));

    // After page change, captured epoch is no longer current
    router.next_page().await;
    assert!(!router.is_epoch_current(captured));

    // But current epoch is current
    let new_epoch = router.get_page_epoch();
    assert!(router.is_epoch_current(new_epoch));
}

#[tokio::test]
async fn test_page_epoch_survives_set_page() {
    let config = make_test_config(vec![
        make_test_page("Voicemeeter"),
        make_test_page("OBS"),
        make_test_page("QLC"),
    ]);

    let router = make_test_router(config);
    let initial = router.get_page_epoch();

    // set_active_page also calls refresh_page which increments epoch
    router.set_active_page("OBS").await.unwrap();
    assert_eq!(router.get_page_epoch(), initial + 1);

    router.set_active_page("QLC").await.unwrap();
    assert_eq!(router.get_page_epoch(), initial + 2);

    // Setting to same page still increments (refresh_page is called)
    router.set_active_page("QLC").await.unwrap();
    assert_eq!(router.get_page_epoch(), initial + 3);
}

// ===== Driver unregister / profile-prune tests =====

#[tokio::test]
async fn test_unregister_driver_removes_from_map() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    router
        .register_driver(
            "winaudio".to_string(),
            Arc::new(ConsoleDriver::new("winaudio")),
        )
        .await
        .unwrap();
    assert!(router.get_driver("winaudio").await.is_some());

    let removed = router.unregister_driver("winaudio").await.unwrap();
    assert!(removed.is_some(), "expected the driver Arc to be returned");
    assert!(router.get_driver("winaudio").await.is_none());
    assert!(!router.list_drivers().await.contains(&"winaudio".into()));
}

#[tokio::test]
async fn test_unregister_driver_unknown_name_is_ok_none() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    let result = router.unregister_driver("never_registered").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_unregister_drivers_not_in_keeps_needed_and_drops_rest() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    for name in ["voicemeeter", "obs", "winaudio"] {
        router
            .register_driver(name.to_string(), Arc::new(ConsoleDriver::new(name)))
            .await
            .unwrap();
    }
    assert_eq!(router.list_drivers().await.len(), 3);

    let needed: std::collections::HashSet<String> =
        ["voicemeeter".to_string(), "winaudio".to_string()]
            .into_iter()
            .collect();
    let removed = router.unregister_drivers_not_in(&needed).await;

    assert_eq!(
        removed,
        vec!["obs".to_string()],
        "obs should be reported as removed"
    );

    let remaining: std::collections::HashSet<String> =
        router.list_drivers().await.into_iter().collect();
    assert_eq!(remaining, needed, "obs should have been pruned");
}

#[tokio::test]
async fn test_unregister_drivers_not_in_with_empty_needed_drops_all() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    router
        .register_driver("obs".to_string(), Arc::new(ConsoleDriver::new("obs")))
        .await
        .unwrap();
    router
        .register_driver(
            "winaudio".to_string(),
            Arc::new(ConsoleDriver::new("winaudio")),
        )
        .await
        .unwrap();

    let removed = router
        .unregister_drivers_not_in(&std::collections::HashSet::new())
        .await;

    let removed_set: std::collections::HashSet<String> = removed.into_iter().collect();
    let expected: std::collections::HashSet<String> = ["obs".to_string(), "winaudio".to_string()]
        .into_iter()
        .collect();
    assert_eq!(
        removed_set, expected,
        "all registered drivers should be reported as removed"
    );
    assert!(router.list_drivers().await.is_empty());
}

/// Audit #55: `Router::update_config` must purge drivers (and their app_state
/// entries) for apps the new config no longer references. Previously this
/// cleanup lived in `app.rs::prune_unused_drivers`, which only ran from the
/// file-watcher reload path; any other caller of `update_config` (REPL,
/// direct API, tests) leaked state forever for apps removed by the new
/// config. After the fix it lives inside `update_config` itself.
#[tokio::test]
async fn test_update_config_unregisters_drivers_removed_from_new_config() {
    let mut control_a = HashMap::new();
    control_a.insert(
        "fader1".to_string(),
        ControlMapping {
            app: "obs".to_string(),
            action: None,
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );
    control_a.insert(
        "fader2".to_string(),
        ControlMapping {
            app: "voicemeeter".to_string(),
            action: None,
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );
    let mut page_a = make_test_page("AB");
    page_a.controls = Some(control_a);
    let config_a = make_test_config(vec![page_a]);

    let router = make_test_router(config_a);
    router
        .register_driver("obs".to_string(), Arc::new(ConsoleDriver::new("obs")))
        .await
        .unwrap();
    router
        .register_driver(
            "voicemeeter".to_string(),
            Arc::new(ConsoleDriver::new("voicemeeter")),
        )
        .await
        .unwrap();
    assert_eq!(router.list_drivers().await.len(), 2);

    // New config drops obs from any page reference.
    let mut control_b = HashMap::new();
    control_b.insert(
        "fader2".to_string(),
        ControlMapping {
            app: "voicemeeter".to_string(),
            action: None,
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: None,
        },
    );
    let mut page_b = make_test_page("B");
    page_b.controls = Some(control_b);
    let config_b = make_test_config(vec![page_b]);

    router
        .update_config(config_b)
        .await
        .expect("update_config should succeed");

    let remaining: std::collections::HashSet<String> =
        router.list_drivers().await.into_iter().collect();
    assert_eq!(
        remaining,
        std::iter::once("voicemeeter".to_string()).collect(),
        "obs must be unregistered because the new config no longer references it"
    );
}

#[tokio::test]
async fn test_unregister_drivers_not_in_is_idempotent() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = make_test_router(config);

    router
        .register_driver("obs".to_string(), Arc::new(ConsoleDriver::new("obs")))
        .await
        .unwrap();

    let needed: std::collections::HashSet<String> = ["obs".to_string()].into_iter().collect();
    let first = router.unregister_drivers_not_in(&needed).await;
    let second = router.unregister_drivers_not_in(&needed).await;

    assert!(first.is_empty(), "nothing to remove on first call");
    assert!(
        second.is_empty(),
        "nothing to remove on second call (idempotent)"
    );
    assert_eq!(router.list_drivers().await, vec!["obs".to_string()]);
}

// ===== Multi-action buttons (`also`) =====

/// A control with `also` must fan out to its primary effect AND every extra
/// step. Action steps fire on press only (release filtered); MIDI direct steps
/// fire on both edges (press + release). Mirrors the real Record button:
/// passthrough primary + OBS `selectCamera` (action) + QLC CC (midi).
///
/// mute1 in MCU mode is note=16 (see docs/xtouch-matching.csv), outside the
/// paging note ranges, so it falls through to the driver/MIDI dispatch path.
#[tokio::test]
async fn test_multi_action_also_fans_out_press_and_release_semantics() {
    use crate::config::{ActionStep, MidiSpec, MidiType};

    let mut page = make_test_page("Page 1");
    let mut controls = HashMap::new();
    controls.insert(
        "mute1".to_string(),
        ControlMapping {
            app: "primary".to_string(),
            action: Some("trigger".to_string()),
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: Some(vec![
                // Action step (OBS-like) → press-only.
                ActionStep {
                    app: "secondary".to_string(),
                    action: Some("selectCamera".to_string()),
                    params: Some(vec![json!("Main")]),
                    midi: None,
                },
                // MIDI direct step (QLC-like CC) → both edges.
                ActionStep {
                    app: "midiapp".to_string(),
                    action: None,
                    params: None,
                    midi: Some(MidiSpec {
                        midi_type: MidiType::Cc,
                        channel: Some(1),
                        cc: Some(20),
                        note: None,
                    }),
                },
            ]),
            toggle: None,
        },
    );
    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = make_test_router(config);

    let primary = Arc::new(ConsoleDriver::new("primary"));
    let secondary = Arc::new(ConsoleDriver::new("secondary"));
    let midiapp = Arc::new(ConsoleDriver::new("midiapp"));
    router
        .register_driver("primary".to_string(), primary.clone())
        .await
        .unwrap();
    router
        .register_driver("secondary".to_string(), secondary.clone())
        .await
        .unwrap();
    router
        .register_driver("midiapp".to_string(), midiapp.clone())
        .await
        .unwrap();

    // Press: Note On, ch1, note 16, velocity 127 → all three fire once.
    router.on_midi_from_xtouch(&[0x90, 16, 127]).await;
    assert_eq!(
        primary.execution_count().await,
        1,
        "primary action fires on press"
    );
    assert_eq!(
        secondary.execution_count().await,
        1,
        "also action fires on press"
    );
    assert_eq!(
        midiapp.execution_count().await,
        1,
        "also midi fires on press"
    );

    // Release: real Note Off (0x80). Action steps must NOT re-fire; the MIDI
    // direct step fires again (both edges → momentary CC 127/0).
    router.on_midi_from_xtouch(&[0x80, 16, 0]).await;
    assert_eq!(
        primary.execution_count().await,
        1,
        "primary action ignores release"
    );
    assert_eq!(
        secondary.execution_count().await,
        1,
        "also action ignores release"
    );
    assert_eq!(
        midiapp.execution_count().await,
        2,
        "also midi fires on release too"
    );
}

// ===== Feedback-driven toggles (`toggle`) =====

use crate::config::{ActionStep, GlobalPageDefaults, MidiSpec, MidiType, ToggleConfig};

/// A `record` toggle whose `on` steps target driver "obs" (selectCamera) and
/// `off` steps target a distinct driver "obsoff" (changeScene), so a test can
/// tell which edge fired. `watch` is left to the caller (explicit vs derived).
fn record_toggle(watch: Option<MidiSpec>) -> ToggleConfig {
    ToggleConfig {
        source: Some("voicemeeter".to_string()),
        watch,
        on: vec![ActionStep {
            app: "obs".to_string(),
            action: Some("selectCamera".to_string()),
            params: Some(vec![json!("Main"), json!("program")]),
            midi: None,
        }],
        off: vec![ActionStep {
            app: "obsoff".to_string(),
            action: Some("changeScene".to_string()),
            params: Some(vec![json!("End")]),
            midi: None,
        }],
    }
}

/// Config with a global `record` control carrying `toggle`, plus one empty page.
fn make_toggle_config(toggle: ToggleConfig) -> AppConfig {
    let mut controls = HashMap::new();
    controls.insert(
        "record".to_string(),
        ControlMapping {
            app: "voicemeeter".to_string(),
            action: None,
            params: None,
            midi: None,
            overlay: None,
            indicator: None,
            also: None,
            toggle: Some(toggle),
        },
    );
    let mut config = make_test_config(vec![make_test_page("P1")]);
    config.pages_global = Some(GlobalPageDefaults {
        controls: Some(controls),
        lcd: None,
        passthroughs: None,
    });
    config
}

/// Register the `on`/`off` target drivers and return them for assertions.
async fn register_toggle_targets(router: &Router) -> (Arc<ConsoleDriver>, Arc<ConsoleDriver>) {
    let on_drv = Arc::new(ConsoleDriver::new("obs"));
    let off_drv = Arc::new(ConsoleDriver::new("obsoff"));
    router
        .register_driver("obs".to_string(), on_drv.clone())
        .await
        .unwrap();
    router
        .register_driver("obsoff".to_string(), off_drv.clone())
        .await
        .unwrap();
    (on_drv, off_drv)
}

/// Note On, ch1, note 95 (the record button's MCU address), given velocity.
fn note95(velocity: u8) -> [u8; 3] {
    [0x90, 95, velocity]
}

/// A toggle must fire `on` on the OFF→ON edge and `off` on the ON→OFF edge,
/// stay silent on repeated identical states, and never fire on the first
/// (baseline) observation.
#[tokio::test]
async fn test_feedback_toggle_fires_on_state_transitions() {
    let watch = Some(MidiSpec {
        midi_type: MidiType::Note,
        channel: Some(1),
        cc: None,
        note: Some(95),
    });
    let router = make_test_router(make_toggle_config(record_toggle(watch)));
    let (on_drv, off_drv) = register_toggle_targets(&router).await;

    // 1) First feedback (OFF): baseline only, no fire.
    router
        .on_midi_from_app("voicemeeter", &note95(0), "voicemeeter")
        .await;
    assert_eq!(
        on_drv.execution_count().await,
        0,
        "baseline OFF must not fire"
    );
    assert_eq!(off_drv.execution_count().await, 0);

    // 2) OFF→ON edge: `on` fires once.
    router
        .on_midi_from_app("voicemeeter", &note95(127), "voicemeeter")
        .await;
    assert_eq!(on_drv.execution_count().await, 1, "ON edge fires `on`");
    assert_eq!(off_drv.execution_count().await, 0);

    // 3) Repeated ON (different velocity, still > 0): idempotent, no re-fire.
    router
        .on_midi_from_app("voicemeeter", &note95(100), "voicemeeter")
        .await;
    assert_eq!(
        on_drv.execution_count().await,
        1,
        "repeated ON must not re-fire"
    );

    // 4) ON→OFF edge: `off` fires once.
    router
        .on_midi_from_app("voicemeeter", &note95(0), "voicemeeter")
        .await;
    assert_eq!(off_drv.execution_count().await, 1, "OFF edge fires `off`");
    assert_eq!(
        on_drv.execution_count().await,
        1,
        "`on` unchanged on OFF edge"
    );
}

/// The very first feedback only records state, even when it is ON — so
/// connecting (or a config reload) never triggers an unexpected action.
#[tokio::test]
async fn test_feedback_toggle_initial_on_does_not_fire() {
    let watch = Some(MidiSpec {
        midi_type: MidiType::Note,
        channel: Some(1),
        cc: None,
        note: Some(95),
    });
    let router = make_test_router(make_toggle_config(record_toggle(watch)));
    let (on_drv, off_drv) = register_toggle_targets(&router).await;

    router
        .on_midi_from_app("voicemeeter", &note95(127), "voicemeeter")
        .await;
    assert_eq!(on_drv.execution_count().await, 0, "initial ON only records");
    assert_eq!(off_drv.execution_count().await, 0);
}

/// With no explicit `watch`, the watched address is derived from the control's
/// hardware mapping (`record` = Note 95 in MCU mode). Unrelated notes are
/// ignored.
#[tokio::test]
async fn test_feedback_toggle_default_watch_uses_hardware_address() {
    let router = make_test_router(make_toggle_config(record_toggle(None)));
    let (on_drv, _off_drv) = register_toggle_targets(&router).await;

    // Baseline OFF, then ON edge on Note 95 fires `on`.
    router
        .on_midi_from_app("voicemeeter", &note95(0), "voicemeeter")
        .await;
    router
        .on_midi_from_app("voicemeeter", &note95(127), "voicemeeter")
        .await;
    assert_eq!(
        on_drv.execution_count().await,
        1,
        "default-derived Note 95 watch fires on the ON edge"
    );

    // A different note is not the record address → ignored.
    router
        .on_midi_from_app("voicemeeter", &[0x90, 80, 127], "voicemeeter")
        .await;
    assert_eq!(
        on_drv.execution_count().await,
        1,
        "unrelated note must not affect the toggle"
    );
}

/// A toggle only reacts to feedback from its `source` app, not other apps.
#[tokio::test]
async fn test_feedback_toggle_ignores_other_source_app() {
    let watch = Some(MidiSpec {
        midi_type: MidiType::Note,
        channel: Some(1),
        cc: None,
        note: Some(95),
    });
    let router = make_test_router(make_toggle_config(record_toggle(watch)));
    let (on_drv, off_drv) = register_toggle_targets(&router).await;

    // Same Note 95, but from OBS (not the toggle's source) → no effect.
    router.on_midi_from_app("obs", &note95(0), "obs").await;
    router.on_midi_from_app("obs", &note95(127), "obs").await;
    assert_eq!(
        on_drv.execution_count().await,
        0,
        "wrong source must not fire"
    );
    assert_eq!(off_drv.execution_count().await, 0);
}
