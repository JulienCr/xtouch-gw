//! Tests for Router module

use super::*;
use crate::config::{ControlMapping, MidiConfig, PageConfig};
use crate::drivers::ConsoleDriver;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

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
        pages,
        tray: None,
    }
}

fn make_test_page(name: &str) -> PageConfig {
    PageConfig {
        name: name.to_string(),
        controls: None,
        lcd: None,
        passthrough: None,
        passthroughs: None,
    }
}

#[tokio::test]
async fn test_page_navigation() {
    let config = make_test_config(vec![
        make_test_page("Page 1"),
        make_test_page("Page 2"),
        make_test_page("Page 3"),
    ]);

    let router = Router::new(config);

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

    let router = Router::new(config);

    router.set_active_page("OBS").await.unwrap();
    assert_eq!(router.get_active_page_name().await, "OBS");

    router.set_active_page("voicemeeter").await.unwrap(); // Case insensitive
    assert_eq!(router.get_active_page_name().await, "Voicemeeter");
}

#[tokio::test]
async fn test_set_page_by_index() {
    let config = make_test_config(vec![make_test_page("Page 0"), make_test_page("Page 1")]);

    let router = Router::new(config);

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

    let router = Router::new(config);

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

    let router = Router::new(config);

    // Note Off (velocity 0) should be ignored
    let note_off = [0x90, 47, 0]; // Note On with velocity 0 = Note Off
    router.on_midi_from_xtouch(&note_off).await;
    assert_eq!(router.get_active_page_name().await, "Page 1"); // Should stay on Page 1
}

// ===== PHASE 4: Driver Framework Integration Tests =====

#[tokio::test]
async fn test_driver_registration_and_initialization() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = Router::new(config);

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
    let router = Router::new(config);

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
    let initial_config =
        make_test_config(vec![make_test_page("Page 1"), make_test_page("Page 2")]);

    let router = Router::new(initial_config);

    // Register a driver
    router
        .register_driver(
            "test_driver".to_string(),
            Arc::new(ConsoleDriver::new("test_driver")),
        )
        .await
        .unwrap();

    // Update config with different pages
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

    // Driver should still be registered
    assert!(router.get_driver("test_driver").await.is_some());
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
        },
    );
    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = Router::new(config);

    // Register driver
    router
        .register_driver(
            "test_console".to_string(),
            Arc::new(ConsoleDriver::new("test_console")),
        )
        .await
        .unwrap();

    // Execute control action
    let result = router.handle_control("fader1", Some(json!(127))).await;
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
        },
    );
    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = Router::new(config);

    // Attempt to execute control action (should fail)
    let result = router.handle_control("fader1", Some(json!(127))).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Driver 'missing_driver' not registered"));
}

#[tokio::test]
async fn test_driver_execution_missing_control() {
    let config = make_test_config(vec![make_test_page("Test Page")]);
    let router = Router::new(config);

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
        .handle_control("non_existent_control", Some(json!(127)))
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
        },
    );

    page.controls = Some(controls);

    let config = make_test_config(vec![page]);
    let router = Router::new(config);

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
    let result1 = router.handle_control("obs_control", None).await;
    assert!(result1.is_ok());

    let result2 = router.handle_control("vm_control", Some(json!(64))).await;
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

    let router = Router::new(config);

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
    let config = make_test_config(vec![
        make_test_page("Page 1"),
        make_test_page("Page 2"),
    ]);

    let router = Router::new(config);

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

    let router = Router::new(config);
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

