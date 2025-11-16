//! XTouch GW v3 - Rust implementation
//! 
//! Gateway to control Voicemeeter, QLC+, and OBS from Behringer X-Touch MIDI controller.

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod router;
mod state;
mod xtouch;
mod midi;
mod drivers;
mod cli;
mod sniffer;
mod control_mapping;

use crate::config::AppConfig;
use crate::router::Router;
use crate::xtouch::{XTouchDriver, XTouchEvent};
use crate::drivers::midibridge::MidiBridgeDriver;
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

    // Load configuration
    let config = AppConfig::load(&args.config).await?;
    info!("Configuration loaded successfully");

    // Initialize router
    let router = Router::new(config.clone());
    info!("Router initialized");

    // Set up shutdown signal
    let shutdown_signal = shutdown_signal();

    // Start the main application
    run_app(router, config, shutdown_signal).await?;

    info!("XTouch GW shutdown complete");
    Ok(())
}

async fn run_app(
    router: Router,
    config: AppConfig,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    use tracing::{debug};
    
    info!("Starting main application loop...");
    
    // Create and connect X-Touch driver
    let mut xtouch = XTouchDriver::new(&config)?;
    info!("X-Touch driver created");
    
    xtouch.connect().await?;
    info!("X-Touch connected successfully");
    
    // Take the event receiver from XTouch
    let mut xtouch_rx = xtouch
        .take_event_receiver()
        .ok_or_else(|| anyhow::anyhow!("Failed to get X-Touch event receiver"))?;
    
    // Register MIDI bridge drivers for each configured app
    if let Some(apps) = &config.midi.apps {
        for app_config in apps {
            let driver = Arc::new(MidiBridgeDriver::new(
                app_config.output_port.clone().unwrap_or_default(), // to_port: where we send
                app_config.input_port.clone().unwrap_or_default(), // from_port: where we receive
                None, // No filter for now
                None, // No transforms for now
                false, // Not optional
            ));
            
            router.register_driver(app_config.name.clone(), driver).await?;
            info!("Registered MIDI bridge driver for: {}", app_config.name);
        }
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
    println!("  Total controls: {}", db.mappings.len().to_string().green());
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
            println!("    {}: CTRL={}, MCU={}", 
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