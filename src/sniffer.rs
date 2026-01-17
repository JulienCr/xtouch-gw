//! MIDI sniffer for debugging and development
//!
//! Provides CLI and web-based MIDI monitoring tools.

use anyhow::{Context, Result};
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::{Html, IntoResponse},
};
use colored::*;
use midir::{MidiInput, MidiInputConnection};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::info;

use crate::midi::{format_hex, MidiMessage};
use crate::xtouch::discovery;

/// Direction of MIDI message
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Input,
    Output,
    Bidirectional,
}

impl Direction {
    fn display(&self) -> ColoredString {
        match self {
            Direction::Input => "IN ".green(),
            Direction::Output => "OUT".red(),
            Direction::Bidirectional => "I/O".yellow(),
        }
    }
}

/// MIDI sniffer event
#[derive(Debug, Clone)]
pub struct SnifferEvent {
    pub timestamp_ms: u64,
    pub direction: Direction,
    pub port_name: String,
    pub data: Vec<u8>,
    pub message: Option<MidiMessage>,
}

/// CLI MIDI sniffer
pub async fn run_cli_sniffer() -> Result<()> {
    println!("{}", "=== MIDI Sniffer ===".bold().cyan());
    println!("Press Ctrl+C to exit\n");

    // First, discover and list ports
    discovery::print_ports();

    // Ask user to select ports
    println!("Enter input port pattern (or press Enter to monitor all): ");
    let mut input_pattern = String::new();
    std::io::stdin().read_line(&mut input_pattern)?;
    let input_pattern = input_pattern.trim();

    println!("Enter output port pattern (or press Enter to skip): ");
    let mut output_pattern = String::new();
    std::io::stdin().read_line(&mut output_pattern)?;
    let output_pattern = output_pattern.trim();

    // Create sniffer
    let mut sniffer = CliSniffer::new();

    // Connect to ports
    if !input_pattern.is_empty() {
        sniffer.connect_input(input_pattern)?;
    } else {
        sniffer.connect_all_inputs()?;
    }

    if !output_pattern.is_empty() {
        // For output monitoring, we'd need to create a virtual port
        // or use a different approach
        println!("Output monitoring not yet implemented");
    }

    println!("\n{}", "Monitoring MIDI traffic...".green());
    println!(
        "{}",
        "Format: [timestamp] DIR PORT | HEX => PARSED".dimmed()
    );
    println!("{}\n", "─".repeat(80).dimmed());

    // Run sniffer
    sniffer.run().await
}

/// CLI sniffer implementation
struct CliSniffer {
    connections: Vec<MidiInputConnection<()>>,
    event_rx: mpsc::Receiver<SnifferEvent>,
    event_tx: mpsc::Sender<SnifferEvent>,
    running: Arc<AtomicBool>,
    start_time: Instant,
}

impl CliSniffer {
    fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(1000);

        Self {
            connections: Vec::new(),
            event_rx,
            event_tx,
            running: Arc::new(AtomicBool::new(true)),
            start_time: Instant::now(),
        }
    }

    fn connect_input(&mut self, pattern: &str) -> Result<()> {
        let midi_in = MidiInput::new("XTouch-Sniffer")?;

        // Check if pattern is a numeric index
        if let Ok(index) = pattern.parse::<usize>() {
            // Try to get port by index
            if let Some(port) = midi_in.ports().into_iter().nth(index) {
                if let Ok(name) = midi_in.port_name(&port) {
                    self.connect_port(midi_in, port, &name)?;
                    return Ok(());
                }
            }
            anyhow::bail!("No port found at index: {}", index)
        } else {
            // Treat as name pattern
            for port in midi_in.ports() {
                if let Ok(name) = midi_in.port_name(&port) {
                    if name.to_lowercase().contains(&pattern.to_lowercase()) {
                        self.connect_port(midi_in, port, &name)?;
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("No port found matching pattern: {}", pattern)
        }
    }

    fn connect_all_inputs(&mut self) -> Result<()> {
        let ports = discovery::discover_input_ports()?;

        for port_info in ports {
            if !port_info.is_virtual {
                let midi_in = MidiInput::new(&format!("Sniffer-{}", port_info.index))?;
                // Get the port by index
                if let Some(port) = midi_in.ports().into_iter().nth(port_info.index) {
                    self.connect_port(midi_in, port, &port_info.name)?;
                }
            }
        }

        if self.connections.is_empty() {
            anyhow::bail!("No physical MIDI ports found");
        }

        Ok(())
    }

    fn connect_port(
        &mut self,
        midi_in: MidiInput,
        port: midir::MidiInputPort,
        port_name: &str,
    ) -> Result<()> {
        let event_tx = self.event_tx.clone();
        let port_name = port_name.to_string();
        let start_time = self.start_time;

        info!("Connecting to: {}", port_name);

        let conn = midi_in.connect(
            &port,
            "Sniffer",
            move |_timestamp, data, _| {
                let elapsed = Instant::now() - start_time;
                let timestamp_ms = elapsed.as_millis() as u64;

                let message = MidiMessage::parse(data);

                let event = SnifferEvent {
                    timestamp_ms,
                    direction: Direction::Input,
                    port_name: port_name.clone(),
                    data: data.to_vec(),
                    message,
                };

                let _ = event_tx.try_send(event);
            },
            (),
        )?;

        self.connections.push(conn);
        Ok(())
    }

    async fn run(mut self) -> Result<()> {
        let running = self.running.clone();

        // Set up Ctrl+C handler
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            running.store(false, Ordering::Relaxed);
        });

        // Process events
        while self.running.load(Ordering::Relaxed) {
            tokio::select! {
                Some(event) = self.event_rx.recv() => {
                    self.print_event(&event);
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                    // Check if we should exit
                    if !self.running.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }
        }

        println!("\n{}", "Sniffer stopped".yellow());
        Ok(())
    }

    fn print_event(&self, event: &SnifferEvent) {
        let timestamp = format!("{:08}", event.timestamp_ms);
        let direction = event.direction.display();
        let port = if event.port_name.len() > 20 {
            format!("{}...", &event.port_name[..17])
        } else {
            event.port_name.clone()
        };

        let hex = format_hex(&event.data);

        // Format the parsed message
        let parsed = if let Some(ref msg) = event.message {
            format!(" => {}", msg.to_string().bright_blue())
        } else {
            String::new()
        };

        // Color code by message type
        let hex_colored = if let Some(ref msg) = event.message {
            match msg {
                MidiMessage::NoteOn { .. } => hex.bright_green(),
                MidiMessage::NoteOff { .. } => hex.bright_red(),
                MidiMessage::ControlChange { .. } => hex.bright_yellow(),
                MidiMessage::PitchBend { .. } => hex.bright_cyan(),
                MidiMessage::SysEx { .. } => hex.bright_magenta(),
                _ => hex.normal(),
            }
        } else {
            hex.bright_black()
        };

        println!(
            "[{}ms] {} {:20} | {}{}",
            timestamp.dimmed(),
            direction,
            port.white(),
            hex_colored,
            parsed
        );
    }
}

/// Web sniffer server
pub async fn run_web_sniffer(port: u16) -> Result<()> {
    use axum::{routing::get, Router};

    info!("Starting web sniffer on http://localhost:{}", port);

    let app = Router::new()
        .route("/", get(serve_html))
        .route("/ws", get(websocket_handler));

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .context("Failed to bind web server")?;

    info!("Web sniffer available at http://localhost:{}", port);

    axum::serve(listener, app)
        .await
        .context("Web server failed")?;

    Ok(())
}

async fn serve_html() -> impl IntoResponse {
    Html(include_str!("../static/sniffer.html"))
}

async fn websocket_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_websocket)
}

async fn handle_websocket(mut socket: WebSocket) {
    use axum::extract::ws::Message;

    // WebSocket MIDI streaming is not yet implemented
    // Close the connection gracefully with a message
    if let Err(e) = socket
        .send(Message::Text(
            r#"{"error": "WebSocket MIDI streaming not yet implemented"}"#.to_string(),
        ))
        .await
    {
        tracing::debug!("Failed to send WebSocket close message: {}", e);
    }
    if let Err(e) = socket.close().await {
        tracing::debug!("Failed to close WebSocket: {}", e);
    }
}

/// List all ports in a formatted way
pub fn list_ports_formatted() {
    use colored::*;

    println!("\n{}", "=== Available MIDI Ports ===".bold().cyan());

    if let Ok(inputs) = discovery::discover_input_ports() {
        println!("\n{}", "Input Ports:".bold());
        if inputs.is_empty() {
            println!("  {}", "No input ports found".dimmed());
        } else {
            for port in inputs {
                let marker = if port.is_virtual {
                    "[VIRTUAL]".yellow()
                } else {
                    "[PHYSICAL]".green()
                };
                println!("  {} {}", marker, port.name);
            }
        }
    }

    if let Ok(outputs) = discovery::discover_output_ports() {
        println!("\n{}", "Output Ports:".bold());
        if outputs.is_empty() {
            println!("  {}", "No output ports found".dimmed());
        } else {
            for port in outputs {
                let marker = if port.is_virtual {
                    "[VIRTUAL]".yellow()
                } else {
                    "[PHYSICAL]".green()
                };
                println!("  {} {}", marker, port.name);
            }
        }
    }

    // Try to auto-detect X-Touch
    if let Some((input, output)) = discovery::find_xtouch_ports() {
        println!("\n{}", "Auto-detected X-Touch:".bold().bright_green());
        println!("  Input:  {}", input.bright_white());
        println!("  Output: {}", output.bright_white());
    }

    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direction_display() {
        // Just ensure display doesn't panic
        let _ = Direction::Input.display();
        let _ = Direction::Output.display();
        let _ = Direction::Bidirectional.display();
    }
}
