//! Gamepad diagnostics tool for troubleshooting detection issues

#![allow(dead_code)]

use gilrs::{Gilrs, Button, Axis, Event, EventType};
use rusty_xinput::XInputHandle;
use tracing::info;
use std::thread;
use std::time::Duration;

/// Print detailed information about all detected gamepads
///
/// This is useful for troubleshooting gamepad detection issues,
/// especially for Bluetooth controllers that may have non-obvious names.
pub fn print_gamepad_diagnostics() {
    info!("=== Hybrid Gamepad Diagnostics ===");
    info!("Platform: {}", std::env::consts::OS);
    info!("");

    // Check XInput backend
    info!("--- XInput Backend ---");
    match XInputHandle::load_default() {
        Ok(handle) => {
            info!("✅ XInput library loaded successfully");
            info!("");

            let mut xinput_count = 0;
            for user_index in 0..4 {
                match handle.get_state(user_index) {
                    Ok(state) => {
                        xinput_count += 1;
                        info!("📋 XInput User Index {}: CONNECTED", user_index);
                        info!("   Product name: \"XInput Controller {}\"", user_index + 1);
                        info!("   Packet number: {}", state.raw.dwPacketNumber);
                        info!("   Buttons: 0x{:04X}", state.raw.Gamepad.wButtons);
                        info!("   Left stick: ({}, {})", state.raw.Gamepad.sThumbLX, state.raw.Gamepad.sThumbLY);
                        info!("   Right stick: ({}, {})", state.raw.Gamepad.sThumbRX, state.raw.Gamepad.sThumbRY);
                        info!("   Triggers: (L={}, R={})", state.left_trigger(), state.right_trigger());
                        info!("");
                        info!("   📌 Config pattern suggestion:");
                        info!("      product_match: \"XInput\"  # Matches all XInput controllers");
                        info!("");
                    }
                    Err(rusty_xinput::XInputUsageError::DeviceNotConnected) => {
                        // Not an error, just not connected
                    }
                    Err(e) => {
                        info!("   XInput slot {}: Error - {:?}", user_index, e);
                    }
                }
            }

            if xinput_count == 0 {
                info!("⚠️  No XInput controllers detected");
            } else {
                info!("✅ Found {} XInput controller(s)", xinput_count);
            }
        }
        Err(e) => {
            info!("❌ XInput library not available: {:?}", e);
            info!("   This is normal if XInput DLLs are not installed.");
            info!("   Non-XInput controllers will still work via WGI backend.");
        }
    }

    info!("");
    info!("--- gilrs Backend (WGI) ---");
    info!("Initializing gilrs...");

    let mut gilrs = match Gilrs::new() {
        Ok(g) => {
            info!("✅ gilrs initialized successfully");
            g
        }
        Err(e) => {
            info!("❌ Failed to initialize GilRs: {:?}", e);
            info!("This may indicate missing system libraries or permissions issues.");
            return;
        }
    };

    info!("⏳ Waiting for gamepads to connect (5 seconds)...");
    info!("   (Bluetooth controllers may take a moment to wake up)");

    // Poll events for 5 seconds to allow Bluetooth gamepads to connect
    let start = std::time::Instant::now();
    let wait_duration = Duration::from_secs(5);

    while start.elapsed() < wait_duration {
        // Process events to trigger connection detection
        while let Some(Event { event, .. }) = gilrs.next_event() {
            match event {
                EventType::Connected => {
                    info!("   📶 Gamepad connection detected...");
                }
                EventType::Disconnected => {
                    info!("   📵 Gamepad disconnection detected...");
                }
                _ => {} // Ignore other events during scan
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    info!("");
    info!("📋 Scan complete. Enumerating detected gamepads...");
    info!("");

    let gamepads: Vec<_> = gilrs.gamepads().collect();

    if gamepads.is_empty() {
        info!("⚠️  No gamepads detected");
        info!("   Please check:");
        info!("   - Gamepad is connected (USB or Bluetooth paired)");
        info!("   - Drivers are installed");
        info!("   - Device appears in Windows Device Manager");
        return;
    }

    info!("✅ Found {} gamepad(s):", gamepads.len());
    info!("");

    for (id, gamepad) in gamepads {
        info!("📋 Gamepad ID: {:?}", id);
        info!("   Name: \"{}\"", gamepad.name());
        info!("   Connected: {}", gamepad.is_connected());
        info!("   Power Info: {:?}", gamepad.power_info());

        // UUID for stable identification
        let uuid = gamepad.uuid();
        info!("   UUID: {:?}", uuid);

        info!("");
        info!("   📌 Config pattern suggestion:");
        info!("      product_match: \"{}\"", gamepad.name());
        info!("");

        // Show currently pressed buttons (if any)
        info!("   🎮 Current button states:");
        let mut has_pressed = false;
        for button in &[
            Button::South,
            Button::East,
            Button::West,
            Button::North,
            Button::LeftTrigger,
            Button::RightTrigger,
            Button::LeftTrigger2,
            Button::RightTrigger2,
            Button::Select,
            Button::Start,
            Button::Mode,
            Button::LeftThumb,
            Button::RightThumb,
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
        ] {
            if gamepad.is_pressed(*button) {
                info!("      {:?}: PRESSED", button);
                has_pressed = true;
            }
        }
        if !has_pressed {
            info!("      (no buttons currently pressed)");
        }

        info!("");
        info!("   🕹️  Current axis values:");
        let mut has_axis_movement = false;
        for axis in &[
            Axis::LeftStickX,
            Axis::LeftStickY,
            Axis::RightStickX,
            Axis::RightStickY,
            Axis::LeftZ,
            Axis::RightZ,
        ] {
            let value = gamepad.value(*axis);
            if value.abs() > 0.01 {
                info!("      {:?}: {:.3}", axis, value);
                has_axis_movement = true;
            }
        }
        if !has_axis_movement {
            info!("      (all axes centered, move sticks to see values)");
        }

        info!("");
        info!("   ─────────────────────────────────");
        info!("");
    }

    info!("=== End Diagnostics ===");
    info!("");
    info!("💡 Tips:");
    info!("   - Use the 'Name' field value in your config's product_match");
    info!("   - Product matching is case-insensitive substring matching");
    info!("   - For multi-gamepad mode, add each device to the gamepads array");
    info!("   - XInput controllers appear with generic names: 'XInput Controller 1', etc.");
    info!("   - If an Xbox controller appears in BOTH backends, XInput will be preferred");
    info!("");
}
