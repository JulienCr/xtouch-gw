//! XTouch GW v3 - Rust implementation
//!
//! Gateway to control Voicemeeter, QLC+, and OBS from Behringer X-Touch MIDI controller.

use anyhow::Result;
use clap::Parser;
use tokio::sync::mpsc;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cli;
mod config;
mod control_mapping;
mod drivers;
mod midi;
mod router;
mod sniffer;
mod state;
mod xtouch;

use crate::config::{watcher::ConfigWatcher, AppConfig};
use crate::drivers::midibridge::MidiBridgeDriver;
use crate::drivers::obs::ObsDriver;
use crate::drivers::qlc::QlcDriver;
use crate::router::Router;
use crate::xtouch::XTouchDriver;
use std::sync::Arc;

/// XTouch Gateway - Control Voicemeeter, QLC+, and OBS from Behringer X-Touch
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.yaml")]
    config: String,

    /// Log level (error, warn, info, debug, trace)
    #[arg(short, long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Run in sniffer mode
    #[arg(long)]
    sniffer: bool,

    /// Enable web sniffer interface
    #[arg(long)]
    web_sniffer: bool,

    /// Web sniffer port
    #[arg(long, default_value = "8123")]
    web_port: u16,

    /// List available MIDI ports
    #[arg(long)]
    list_ports: bool,

    /// Test control mappings
    #[arg(long)]
    test_mappings: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();

    // Parse command line arguments
    let args = Args::parse();

    // Initialize logging
    init_logging(&args.log_level)?;

    info!("Starting XTouch GW v3...");
    info!("Configuration file: {}", args.config);

    // Handle list ports
    if args.list_ports {
        sniffer::list_ports_formatted();
        return Ok(());
    }

    // Handle test mappings
    if args.test_mappings {
        test_control_mappings().await?;
        return Ok(());
    }

    // Handle sniffer mode
    if args.sniffer || args.web_sniffer {
        if args.web_sniffer {
            info!("Starting web sniffer on port {}", args.web_port);
            sniffer::run_web_sniffer(args.web_port).await?;
        } else {
            sniffer::run_cli_sniffer().await?;
        }
        return Ok(());
    }

    // Load configuration with hot-reload watcher
    let (config_watcher, initial_config) = ConfigWatcher::new(args.config.clone()).await?;
    info!("Configuration loaded successfully with hot-reload enabled");

    // Initialize router
    let router = Router::new((*initial_config).clone());
    info!("Router initialized");

    // Set up shutdown signal
    let shutdown_signal = shutdown_signal();

    // Start the main application
    run_app(
        router,
        (*initial_config).clone(),
        config_watcher,
        shutdown_signal,
    )
    .await?;

    info!("XTouch GW shutdown complete");
    Ok(())
}

async fn run_app(
    router: Router,
    config: AppConfig,
    mut config_watcher: ConfigWatcher,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    use tracing::{debug, warn};

    info!("Starting main application loop...");

    // Create and connect X-Touch driver
    let mut xtouch = XTouchDriver::new(&config)?;
    info!("X-Touch driver created");

    xtouch.connect().await?;
    info!("X-Touch connected successfully");

    // Initialize LCD and LEDs for the active page
    info!("Initializing X-Touch display...");

    // Clear all displays first
    if let Err(e) = xtouch.clear_all_lcds().await {
        warn!("Failed to clear LCDs: {}", e);
    }

    // Get active page config
    let active_page = router.get_active_page().await;
    let active_page_name = router.get_active_page_name().await;

    if let Some(page) = active_page {
        // Apply LCD labels and colors
        let labels = page.lcd.as_ref().and_then(|lcd| lcd.labels.as_ref());

        // Convert LcdColor to u8 values
        let colors_u8: Option<Vec<u8>> = page.lcd.as_ref().and_then(|lcd| {
            lcd.colors.as_ref().map(|colors| {
                colors
                    .iter()
                    .map(|c| match c {
                        crate::config::LcdColor::Numeric(n) => (*n as u8).min(7),
                        crate::config::LcdColor::Named(_) => 0, // TODO: Parse named colors
                    })
                    .collect()
            })
        });

        if let Err(e) = xtouch
            .apply_lcd_for_page(labels, colors_u8.as_ref(), &active_page_name)
            .await
        {
            warn!("Failed to apply LCD for page: {}", e);
        }
    }

    // Update F-key LEDs to show active page
    let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1) as u8;
    if let Err(e) = router
        .update_fkey_leds_for_active_page(&xtouch, paging_channel)
        .await
    {
        warn!("Failed to update F-key LEDs: {}", e);
    }

    // Update prev/next navigation LEDs (always on)
    if let Some(paging) = &config.paging {
        if let Err(e) = router
            .update_prev_next_leds(&xtouch, paging.prev_note as u8, paging.next_note as u8)
            .await
        {
            warn!("Failed to update prev/next LEDs: {}", e);
        }
    }

    info!("✅ X-Touch display initialized");

    // Create a channel for feedback from apps to X-Touch
    let (feedback_tx, mut feedback_rx) = mpsc::channel::<(String, Vec<u8>)>(1000);

    // Take the event receiver from XTouch
    let mut xtouch_rx = xtouch
        .take_event_receiver()
        .ok_or_else(|| anyhow::anyhow!("Failed to get X-Touch event receiver"))?;

    // Register MIDI bridge drivers for each configured app
    if let Some(apps) = &config.midi.apps {
        for app_config in apps {
            let driver = Arc::new(MidiBridgeDriver::new(
                app_config.output_port.clone().unwrap_or_default(), // to_port: where we send
                app_config.input_port.clone().unwrap_or_default(),  // from_port: where we receive
                None,                                               // No filter for now
                None,                                               // No transforms for now
                false,                                              // Not optional
            ));

            // Set up feedback callback to route MIDI from app back to X-Touch via channel
            let feedback_tx_clone = feedback_tx.clone();
            let app_name = app_config.name.clone();
            driver.set_feedback_callback(Arc::new(move |data: &[u8]| {
                debug!("📥 Feedback from {}: {:02X?}", app_name, data);

                // Send to channel for main loop to forward to X-Touch
                if let Err(e) = feedback_tx_clone.try_send((app_name.clone(), data.to_vec())) {
                    warn!("Failed to send feedback to channel: {}", e);
                }
            }));

            router
                .register_driver(app_config.name.clone(), driver)
                .await?;
            info!("Registered MIDI bridge driver for: {}", app_config.name);
        }
    }

    // Drop the original sender so the channel closes when all drivers are shut down
    drop(feedback_tx);

    // Register OBS driver if configured
    if let Some(obs_config) = &config.obs {
        let obs_driver = Arc::new(ObsDriver::new(
            obs_config.host.clone(),
            obs_config.port,
            obs_config.password.clone(),
        ));
        router
            .register_driver("obs".to_string(), obs_driver)
            .await?;
        info!("Registered OBS driver");
    }

    // Register QLC driver (stub - uses MIDI passthrough)
    // Only register if not already registered (e.g. by MIDI bridge)
    if router.get_driver("qlc").await.is_none() {
        let qlc_driver = Arc::new(QlcDriver::new());
        router
            .register_driver("qlc".to_string(), qlc_driver)
            .await?;
        info!("Registered QLC+ driver (stub)");
    } else {
        info!("Skipping QLC+ stub driver registration (MIDI bridge 'qlc' already active)");
    }

    info!("All drivers registered and initialized");
    info!("Ready to process MIDI events!");

    // Main event loop
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            // Handle X-Touch events
            Some(event) = xtouch_rx.recv() => {
                debug!("Received X-Touch event: raw={:02X?}", event.raw_data);

                // Route the event through the router
                router.on_midi_from_xtouch(&event.raw_data).await;

                // Check if page changed and display needs update
                if router.check_and_clear_display_update().await {
                    info!("📺 Updating display after page change...");

                    // Get active page config
                    let active_page = router.get_active_page().await;
                    let active_page_name = router.get_active_page_name().await;

                    if let Some(page) = active_page {
                        // Apply LCD labels and colors
                        let labels = page.lcd.as_ref().and_then(|lcd| lcd.labels.as_ref());

                        // Convert LcdColor to u8 values
                        let colors_u8: Option<Vec<u8>> = page.lcd.as_ref().and_then(|lcd| {
                            lcd.colors.as_ref().map(|colors| {
                                colors.iter().map(|c| match c {
                                    crate::config::LcdColor::Numeric(n) => (*n as u8).min(7),
                                    crate::config::LcdColor::Named(_) => 0, // TODO: Parse named colors
                                }).collect()
                            })
                        });

                        if let Err(e) = xtouch.apply_lcd_for_page(labels, colors_u8.as_ref(), &active_page_name).await {
                            warn!("Failed to apply LCD for page: {}", e);
                        }
                    }

                    // Update F-key LEDs to show active page
                    let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1) as u8;
                    if let Err(e) = router.update_fkey_leds_for_active_page(&xtouch, paging_channel).await {
                        warn!("Failed to update F-key LEDs: {}", e);
                    }

                    // Also update prev/next navigation LEDs (keep them on)
                    if let Some(paging) = &config.paging {
                        if let Err(e) = router.update_prev_next_leds(&xtouch, paging.prev_note as u8, paging.next_note as u8).await {
                            warn!("Failed to update prev/next LEDs: {}", e);
                        }
                    }

                    info!("✅ Display updated for page: {}", active_page_name);
                }
            }

            // Handle feedback from applications → X-Touch
            Some((app_name, feedback_data)) = feedback_rx.recv() => {
                // Process feedback through router (reverse transformation)
                if let Some(transformed) = router.process_feedback(&app_name, &feedback_data).await {
                    debug!("📤 Forwarding feedback to X-Touch: {:02X?}", transformed);
                    if let Err(e) = xtouch.send_raw(&transformed).await {
                        warn!("Failed to send feedback to X-Touch: {}", e);
                    }
                }
            }

            // Handle config reload
            Some(new_config) = config_watcher.next_config() => {
                info!("📝 Configuration file changed, reloading...");

                match router.update_config(new_config).await {
                    Ok(()) => {
                        info!("✅ Configuration reloaded successfully without dropping events");
                    }
                    Err(e) => {
                        warn!("⚠️  Failed to reload config (keeping old config): {}", e);
                    }
                }
            }

            // Handle shutdown signal
            _ = &mut shutdown => {
                info!("Shutdown signal received, stopping event loop");
                break;
            }
        }
    }

    // Cleanup
    info!("Shutting down...");
    xtouch.disconnect();
    router.shutdown_all_drivers().await?;
    info!("All drivers shut down");

    Ok(())
}

fn init_logging(level: &str) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_thread_names(false),
        )
        .init();

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install CTRL+C signal handler");
    info!("Shutdown signal received");
}

async fn test_control_mappings() -> Result<()> {
    use crate::control_mapping::{load_default_mappings, MidiSpec};
    use colored::*;

    println!("\n{}", "=== Testing Control Mappings ===".bold().cyan());

    let db = load_default_mappings()?;

    println!("\n{}", "Loaded Mappings:".bold());
    println!(
        "  Total controls: {}",
        db.mappings.len().to_string().green()
    );
    println!("  Groups: {}", db.groups().count().to_string().green());

    println!("\n{}", "Groups:".bold());
    for group in db.groups() {
        let count = db.get_group(group).map(|g| g.len()).unwrap_or(0);
        println!("  {} ({} controls)", group.yellow(), count);
    }

    println!("\n{}", "Sample Mappings:".bold());

    // Test fader1
    if let Some(mapping) = db.get("fader1") {
        println!("\n  {}:", "fader1".bright_white());
        println!("    Group: {}", mapping.group.cyan());
        println!("    CTRL mode: {}", mapping.ctrl_message.green());
        println!("    MCU mode:  {}", mapping.mcu_message.green());

        // Parse and display
        if let Ok(spec) = MidiSpec::parse(&mapping.ctrl_message) {
            println!("    Parsed CTRL: {:?}", spec);
        }
        if let Ok(spec) = MidiSpec::parse(&mapping.mcu_message) {
            println!("    Parsed MCU:  {:?}", spec);
        }
    }

    // Test transport controls
    println!("\n  {}:", "Transport Controls".bright_white());
    for control in &["play", "stop", "record", "rewind", "fast_forward"] {
        if let Some(mapping) = db.get(control) {
            println!(
                "    {}: CTRL={}, MCU={}",
                control.yellow(),
                mapping.ctrl_message.green(),
                mapping.mcu_message.green()
            );
        }
    }

    // Test reverse lookup
    println!("\n{}", "Reverse Lookup Test:".bold());
    let test_spec = MidiSpec::ControlChange { cc: 70 };
    if let Some(control) = db.find_control_by_midi(&test_spec, false) {
        println!("  CC 70 in CTRL mode maps to: {}", control.green());
    }

    let test_spec = MidiSpec::PitchBend { channel: 0 };
    if let Some(control) = db.find_control_by_midi(&test_spec, true) {
        println!("  PitchBend ch1 in MCU mode maps to: {}", control.green());
    }

    println!("\n{}", "✅ Control mapping test complete!".green().bold());

    Ok(())
}
