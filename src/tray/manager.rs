//! Tray manager - Windows system tray integration
//!
//! Runs on a dedicated OS thread to handle Win32 message loop.

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn, trace};

use super::{ActivityDirection, ConnectionStatus, TrayCommand, TrayUpdate};
use crate::config::TrayConfig;
use crate::tray::icons::{generate_icon_bytes, IconColor};

/// Tray manager running on dedicated OS thread
pub struct TrayManager {
    /// Receive updates from Tokio runtime
    update_rx: crossbeam::channel::Receiver<TrayUpdate>,
    /// Send commands to Tokio runtime
    command_tx: crossbeam::channel::Sender<TrayCommand>,
    /// Configuration settings
    config: TrayConfig,
    /// Current status of all drivers
    driver_statuses: HashMap<String, ConnectionStatus>,
    /// Current activity states for all drivers (driver, direction) -> is_active
    driver_activities: HashMap<(String, ActivityDirection), bool>,
    /// Hash of last menu content to avoid unnecessary rebuilds
    last_menu_hash: u64,
}

impl TrayManager {
    /// Create a new tray manager
    pub fn new(
        update_rx: crossbeam::channel::Receiver<TrayUpdate>,
        command_tx: crossbeam::channel::Sender<TrayCommand>,
        config: TrayConfig,
    ) -> Self {
        Self {
            update_rx,
            command_tx,
            config,
            driver_statuses: HashMap::new(),
            driver_activities: HashMap::new(),
            last_menu_hash: 0,
        }
    }

    /// Run the tray manager (blocks until quit)
    ///
    /// This runs the Win32 message loop on the current thread.
    pub fn run(mut self) -> anyhow::Result<()> {
        debug!("Starting system tray manager...");
        debug!("Configuration: activity_leds={}, connection_status={}, led_duration={}ms, poll_interval={}ms",
              self.config.show_activity_leds,
              self.config.show_connection_status,
              self.config.activity_led_duration_ms,
              self.config.status_poll_interval_ms);

        // Generate initial icon (gray - not connected yet)
        let icon_bytes = generate_icon_bytes(IconColor::Gray);

        // Create tray icon
        let icon = tray_icon::Icon::from_rgba(icon_bytes, 16, 16)
            .map_err(|e| anyhow::anyhow!("Failed to create icon: {}", e))?;

        let tray_icon = tray_icon::TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("XTouch GW v3.0.0 - Initializing...")
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create tray icon: {}", e))?;

        debug!("System tray icon created");

        // Create menu
        let menu = self.build_menu()?;
        debug!("Menu created with {} items", menu.items().len());

        tray_icon.set_menu(Some(Box::new(menu.clone())));
        debug!("Menu attached to tray icon");

        // Store menu for later updates
        let menu = Arc::new(parking_lot::Mutex::new(menu));

        // Clone for event handler
        let menu_clone = Arc::clone(&menu);

        // Get menu event receiver
        let menu_channel = muda::MenuEvent::receiver();

        // Process updates from Tokio runtime
        debug!("Tray manager ready, processing updates...");

        // Main event loop - handle both menu events and tray updates
        loop {
            // Pump Windows messages (required for tray/menu events on Windows)
            self.pump_windows_messages();

            // Check for menu events (non-blocking)
            while let Ok(event) = menu_channel.try_recv() {
                debug!("Menu event: {:?}", event.id);

                match event.id.as_ref() {
                    "quit" => {
                        debug!("Quit selected from tray menu");
                        let _ = self.command_tx.try_send(TrayCommand::Shutdown);
                        // Exit the loop to shut down
                        return Ok(());
                    }
                    "connect_obs" => {
                        debug!("Connect OBS selected");
                        let _ = self.command_tx.try_send(TrayCommand::ConnectObs);
                    }
                    "recheck_all" => {
                        debug!("Recheck all selected");
                        let _ = self.command_tx.try_send(TrayCommand::RecheckAll);
                    }
                    "toggle_activity_leds" => {
                        debug!("Toggle activity LEDs");
                        self.config.show_activity_leds = !self.config.show_activity_leds;
                        debug!("Activity LEDs {}", if self.config.show_activity_leds { "enabled" } else { "disabled" });

                        // Rebuild menu to reflect new setting
                        if let Ok(new_menu) = self.build_menu_with_all_statuses() {
                            tray_icon.set_menu(Some(Box::new(new_menu.clone())));
                            *menu_clone.lock() = new_menu;
                        }
                    }
                    "toggle_connection_status" => {
                        debug!("Toggle connection status");
                        self.config.show_connection_status = !self.config.show_connection_status;
                        debug!("Connection status {}", if self.config.show_connection_status { "enabled" } else { "disabled" });

                        // Rebuild menu to reflect new setting
                        if let Ok(new_menu) = self.build_menu_with_all_statuses() {
                            tray_icon.set_menu(Some(Box::new(new_menu.clone())));
                            *menu_clone.lock() = new_menu;
                        }
                    }
                    "about" => {
                        debug!("About selected");
                        // Note: In a real implementation, this would show a dialog
                        // For now, we just log it
                    }
                    _ => {
                        debug!("Unknown menu item: {:?}", event.id);
                    }
                }
            }

            // Check for updates with a timeout
            let update = match self.update_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(update) => update,
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
            };
            match update {
                TrayUpdate::DriverStatus { name, status } => {
                    trace!("Tray: driver {} status changed to {:?}", name, status);

                    // Update our tracking
                    self.driver_statuses.insert(name.clone(), status.clone());

                    // Determine overall icon color (worst status wins)
                    let icon_color = self.calculate_overall_icon_color();
                    trace!("Overall icon color: {:?}", icon_color);

                    let icon_bytes = generate_icon_bytes(icon_color);
                    if let Ok(new_icon) = tray_icon::Icon::from_rgba(icon_bytes, 16, 16) {
                        let _ = tray_icon.set_icon(Some(new_icon));
                        trace!("Tray icon updated");
                    }

                    // Update tooltip with current status summary
                    let tooltip = self.build_tooltip();
                    let _ = tray_icon.set_tooltip(Some(&tooltip));
                    trace!("Tooltip updated: {}", tooltip);

                    // Rebuild menu to show all driver statuses
                    if let Ok(new_menu) = self.build_menu_with_all_statuses() {
                        let item_count = new_menu.items().len();
                        tray_icon.set_menu(Some(Box::new(new_menu.clone())));
                        *menu_clone.lock() = new_menu;
                        trace!("Menu rebuilt with {} items", item_count);
                    }
                }
                TrayUpdate::Activity { driver, direction } => {
                    // Legacy activity update (deprecated)
                    trace!("Tray: activity from {} {:?}", driver, direction);
                }
                TrayUpdate::ActivitySnapshot { activities } => {
                    // Update activity states
                    self.driver_activities = activities;

                    // Calculate hash of current menu content
                    let new_hash = self.calculate_menu_hash();

                    // Only rebuild menu if content actually changed
                    if new_hash != self.last_menu_hash {
                        if let Ok(new_menu) = self.build_menu_with_all_statuses() {
                            tray_icon.set_menu(Some(Box::new(new_menu.clone())));
                            *menu_clone.lock() = new_menu;
                            self.last_menu_hash = new_hash;
                            trace!("Menu rebuilt (hash changed: {} -> {})", self.last_menu_hash, new_hash);
                        }
                    }
                }
            }
        }

        debug!("Tray manager shutting down, removing icon...");

        // Explicitly remove the tray icon to prevent ghost icons
        if let Err(e) = tray_icon.set_visible(false) {
            warn!("Failed to hide tray icon: {}", e);
        }

        drop(tray_icon);
        debug!("Tray icon removed");

        Ok(())
    }

    /// Build the initial menu
    fn build_menu(&self) -> anyhow::Result<muda::Menu> {
        let menu = muda::Menu::new();

        // Title
        let title = muda::MenuItem::new("XTouch GW", false, None);
        menu.append(&title)
            .map_err(|e| anyhow::anyhow!("Failed to append title: {}", e))?;

        menu.append(&muda::PredefinedMenuItem::separator())
            .map_err(|e| anyhow::anyhow!("Failed to append separator: {}", e))?;

        // Status items (will be updated dynamically)
        let status_item = muda::MenuItem::new("⏳ Initializing...", false, None);
        menu.append(&status_item)
            .map_err(|e| anyhow::anyhow!("Failed to append status: {}", e))?;

        menu.append(&muda::PredefinedMenuItem::separator())
            .map_err(|e| anyhow::anyhow!("Failed to append separator: {}", e))?;

        // Actions
        let connect_obs = muda::MenuItem::with_id("connect_obs", "Connect OBS", true, None);
        menu.append(&connect_obs)
            .map_err(|e| anyhow::anyhow!("Failed to append connect: {}", e))?;

        let recheck = muda::MenuItem::with_id("recheck_all", "Recheck All", true, None);
        menu.append(&recheck)
            .map_err(|e| anyhow::anyhow!("Failed to append recheck: {}", e))?;

        menu.append(&muda::PredefinedMenuItem::separator())
            .map_err(|e| anyhow::anyhow!("Failed to append separator: {}", e))?;

        // Quit
        let quit = muda::MenuItem::with_id("quit", "Quit", true, None);
        menu.append(&quit)
            .map_err(|e| anyhow::anyhow!("Failed to append quit: {}", e))?;

        Ok(menu)
    }

    /// Build menu with all driver statuses and activity LEDs
    fn build_menu_with_all_statuses(&self) -> anyhow::Result<muda::Menu> {
        let menu = muda::Menu::new();

        // Title
        let title = muda::MenuItem::new("XTouch GW v3.0.0", false, None);
        menu.append(&title)?;
        menu.append(&muda::PredefinedMenuItem::separator())?;

        // Driver statuses (only show if enabled in config)
        if self.config.show_connection_status {
            if self.driver_statuses.is_empty() {
                let status_item = muda::MenuItem::new("⏳ Initializing...", false, None);
                menu.append(&status_item)?;
            } else {
                let mut drivers: Vec<_> = self.driver_statuses.iter().collect();
                drivers.sort_by_key(|(name, _)| *name);

                for (driver_name, status) in drivers {
                    // Main status line
                    let status_text = match status {
                        ConnectionStatus::Connected => {
                            format!("✓ {}: Connected", driver_name)
                        }
                        ConnectionStatus::Disconnected => {
                            format!("✗ {}: Disconnected", driver_name)
                        }
                        ConnectionStatus::Reconnecting { attempt } => {
                            format!("⏳ {}: Reconnecting ({})", driver_name, attempt)
                        }
                    };

                    let status_item = muda::MenuItem::new(&status_text, false, None);
                    menu.append(&status_item)?;

                    // Activity LEDs (only show for connected drivers and if enabled)
                    if self.config.show_activity_leds && matches!(status, ConnectionStatus::Connected) {
                        let inbound_active = self.driver_activities
                            .get(&(driver_name.to_string(), ActivityDirection::Inbound))
                            .copied()
                            .unwrap_or(false);
                        let outbound_active = self.driver_activities
                            .get(&(driver_name.to_string(), ActivityDirection::Outbound))
                            .copied()
                            .unwrap_or(false);

                        let in_led = if inbound_active { "🟢" } else { "⚪" };
                        let out_led = if outbound_active { "🟢" } else { "⚪" };

                        let activity_text = format!("  In: {}  Out: {}", in_led, out_led);
                        let activity_item = muda::MenuItem::new(&activity_text, false, None);
                        menu.append(&activity_item)?;
                    }
                }
            }

            menu.append(&muda::PredefinedMenuItem::separator())?;
        }

        // Actions
        let connect_obs = muda::MenuItem::with_id("connect_obs", "Connect OBS", true, None);
        menu.append(&connect_obs)?;

        let recheck = muda::MenuItem::with_id("recheck_all", "Recheck All", true, None);
        menu.append(&recheck)?;

        menu.append(&muda::PredefinedMenuItem::separator())?;

        // Settings submenu
        let settings_menu = muda::Submenu::new("Settings", true);

        let led_check = if self.config.show_activity_leds { "✓ " } else { "" };
        let toggle_leds = muda::MenuItem::with_id(
            "toggle_activity_leds",
            &format!("{}Show Activity LEDs", led_check),
            true,
            None,
        );
        settings_menu.append(&toggle_leds)?;

        let status_check = if self.config.show_connection_status { "✓ " } else { "" };
        let toggle_status = muda::MenuItem::with_id(
            "toggle_connection_status",
            &format!("{}Show Connection Status", status_check),
            true,
            None,
        );
        settings_menu.append(&toggle_status)?;

        menu.append(&settings_menu)?;

        // About
        let about = muda::MenuItem::with_id("about", "About XTouch GW", true, None);
        menu.append(&about)?;

        menu.append(&muda::PredefinedMenuItem::separator())?;

        // Quit
        let quit = muda::MenuItem::with_id("quit", "Quit", true, None);
        menu.append(&quit)?;

        Ok(menu)
    }

    /// Calculate overall icon color based on all driver statuses
    ///
    /// Priority (worst to best): Red > Yellow > Green > Gray
    fn calculate_overall_icon_color(&self) -> IconColor {
        if self.driver_statuses.is_empty() {
            return IconColor::Gray;
        }

        let mut has_disconnected = false;
        let mut has_reconnecting = false;
        let mut has_connected = false;

        for status in self.driver_statuses.values() {
            match status {
                ConnectionStatus::Disconnected => has_disconnected = true,
                ConnectionStatus::Reconnecting { .. } => has_reconnecting = true,
                ConnectionStatus::Connected => has_connected = true,
            }
        }

        // Worst status wins
        if has_disconnected {
            IconColor::Red
        } else if has_reconnecting {
            IconColor::Yellow
        } else if has_connected {
            IconColor::Green
        } else {
            IconColor::Gray
        }
    }

    /// Pump Windows messages to process tray/menu events
    ///
    /// On Windows, GUI operations require processing the message queue.
    /// This method peeks and dispatches pending messages.
    #[cfg(target_os = "windows")]
    fn pump_windows_messages(&self) {
        use windows::Win32::UI::WindowsAndMessaging::*;

        unsafe {
            let mut msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// No-op on non-Windows platforms
    #[cfg(not(target_os = "windows"))]
    fn pump_windows_messages(&self) {
        // No-op on non-Windows
    }

    /// Build a tooltip showing current status summary
    fn build_tooltip(&self) -> String {
        if self.driver_statuses.is_empty() {
            return "XTouch GW v3.0.0 - Initializing...".to_string();
        }

        let mut connected = 0;
        let mut disconnected = 0;
        let mut reconnecting = 0;

        for status in self.driver_statuses.values() {
            match status {
                ConnectionStatus::Connected => connected += 1,
                ConnectionStatus::Disconnected => disconnected += 1,
                ConnectionStatus::Reconnecting { .. } => reconnecting += 1,
            }
        }

        let total = self.driver_statuses.len();

        if disconnected > 0 || reconnecting > 0 {
            format!(
                "XTouch GW v3.0.0 - {} connected, {} disconnected, {} reconnecting",
                connected, disconnected, reconnecting
            )
        } else {
            format!("XTouch GW v3.0.0 - All {} drivers connected", total)
        }
    }

    /// Calculate a hash of menu content to detect changes
    ///
    /// This allows us to avoid rebuilding the menu when nothing changed.
    fn calculate_menu_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        // Hash config flags
        self.config.show_activity_leds.hash(&mut hasher);
        self.config.show_connection_status.hash(&mut hasher);

        // Hash driver statuses (sorted for consistency)
        let mut drivers: Vec<_> = self.driver_statuses.iter().collect();
        drivers.sort_by_key(|(name, _)| *name);
        for (name, status) in drivers {
            name.hash(&mut hasher);
            match status {
                ConnectionStatus::Connected => 1u8.hash(&mut hasher),
                ConnectionStatus::Disconnected => 2u8.hash(&mut hasher),
                ConnectionStatus::Reconnecting { attempt } => {
                    3u8.hash(&mut hasher);
                    attempt.hash(&mut hasher);
                }
            }
        }

        // Hash activity states (sorted for consistency)
        let mut activities: Vec<_> = self.driver_activities.iter().collect();
        activities.sort_by(|a, b| a.0.cmp(b.0));
        for ((driver, direction), active) in activities {
            driver.hash(&mut hasher);
            match direction {
                ActivityDirection::Inbound => 1u8.hash(&mut hasher),
                ActivityDirection::Outbound => 2u8.hash(&mut hasher),
            }
            active.hash(&mut hasher);
        }

        hasher.finish()
    }
}
