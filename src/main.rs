//! XTouch GW v3 - Rust implementation
//! 
//! Gateway to control Voicemeeter, QLC+, and OBS from Behringer X-Touch MIDI controller.

use anyhow::Result;
use clap::Parser;
use tracing::{info, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod router;
mod state;
mod xtouch;
mod midi;
mod drivers;
mod cli;
mod sniffer;

use crate::config::AppConfig;
use crate::router::Router;

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
    // This will be implemented as we build out the modules
    info!("Starting main application loop...");
    
    // For now, just wait for shutdown
    shutdown.await;
    
    info!("Shutting down...");
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