//! Behringer X-Touch driver
//! 
//! Handles MIDI communication with the X-Touch control surface.

pub mod fader_setpoint;

use anyhow::{Context, Result, bail};
use midir::{MidiInput, MidiOutput, MidiInputConnection, MidiOutputConnection};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, debug};

use crate::midi::{MidiMessage, format_hex};
use crate::config::{AppConfig, XTouchMode};

/// MIDI event from X-Touch
#[derive(Debug, Clone)]
pub struct XTouchEvent {
    pub timestamp: Instant,
    pub message: MidiMessage,
    pub raw_data: Vec<u8>,
}

/// X-Touch driver for hardware communication
pub struct XTouchDriver {
    /// MIDI input connection
    input_conn: Option<MidiInputConnection<()>>,
    
    /// MIDI output connection  
    output_conn: Option<Arc<Mutex<MidiOutputConnection>>>,
    
    /// Event sender for incoming MIDI
    event_tx: mpsc::Sender<XTouchEvent>,
    
    /// Event receiver
    event_rx: Option<mpsc::Receiver<XTouchEvent>>,
    
    /// X-Touch mode (MCU or Ctrl)
    mode: XTouchMode,
    
    /// Input port name pattern
    input_port_name: String,
    
    /// Output port name pattern
    output_port_name: String,
}

impl XTouchDriver {
    /// Create a new X-Touch driver
    pub fn new(config: &AppConfig) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel(1000);
        
        let mode = config.xtouch
            .as_ref()
            .map(|x| x.mode)
            .unwrap_or(XTouchMode::Mcu);
        
        Ok(Self {
            input_conn: None,
            output_conn: None,
            event_tx,
            event_rx: Some(event_rx),
            mode,
            input_port_name: config.midi.input_port.clone(),
            output_port_name: config.midi.output_port.clone(),
        })
    }
    
    /// List available MIDI input ports
    pub fn list_input_ports() -> Result<Vec<String>> {
        let midi_in = MidiInput::new("XTouch-GW-Scanner")?;
        
        let mut port_names = Vec::new();
        for port in midi_in.ports() {
            if let Ok(name) = midi_in.port_name(&port) {
                port_names.push(name);
            }
        }
        
        Ok(port_names)
    }
    
    /// List available MIDI output ports
    pub fn list_output_ports() -> Result<Vec<String>> {
        let midi_out = MidiOutput::new("XTouch-GW-Scanner")?;
        
        let mut port_names = Vec::new();
        for port in midi_out.ports() {
            if let Ok(name) = midi_out.port_name(&port) {
                port_names.push(name);
            }
        }
        
        Ok(port_names)
    }
    
    /// Find an input port by substring match (Windows-friendly)
    fn find_input_port(
        midi_in: &MidiInput,
        pattern: &str,
    ) -> Option<(midir::MidiInputPort, String)> {
        let ports = midi_in.ports();
        for port in ports {
            if let Ok(name) = midi_in.port_name(&port) {
                // Case-insensitive substring match
                if name.to_lowercase().contains(&pattern.to_lowercase()) {
                    debug!("Found port '{}' matching pattern '{}'", name, pattern);
                    return Some((port, name));
                }
            }
        }
        None
    }
    
    /// Find an output port by substring match (Windows-friendly)
    fn find_output_port(
        midi_out: &MidiOutput,
        pattern: &str,
    ) -> Option<(midir::MidiOutputPort, String)> {
        let ports = midi_out.ports();
        for port in ports {
            if let Ok(name) = midi_out.port_name(&port) {
                // Case-insensitive substring match
                if name.to_lowercase().contains(&pattern.to_lowercase()) {
                    debug!("Found port '{}' matching pattern '{}'", name, pattern);
                    return Some((port, name));
                }
            }
        }
        None
    }
    
    /// Connect to X-Touch MIDI ports
    pub async fn connect(&mut self) -> Result<()> {
        // Disconnect existing connections
        self.disconnect();
        
        info!("Connecting to X-Touch - Input: '{}', Output: '{}'", 
            self.input_port_name, self.output_port_name);
        
        // Connect input
        let midi_in = MidiInput::new("XTouch-GW-Input")
            .context("Failed to create MIDI input")?;
        
        let port_count = midi_in.port_count();
        debug!("Found {} MIDI input ports", port_count);
        
        let (in_port, port_name) = Self::find_input_port(&midi_in, &self.input_port_name)
            .ok_or_else(|| anyhow::anyhow!("Input port '{}' not found", self.input_port_name))?;
        
        info!("Connecting to input port: {}", port_name);
        
        // Set up callback for incoming MIDI
        let event_tx = self.event_tx.clone();
        
        let input_conn = midi_in.connect(
            &in_port,
            "XTouch-GW",
            move |_timestamp, data, _| {
                let timestamp = Instant::now();
                
                // Parse the message
                if let Some(message) = MidiMessage::parse(data) {
                    let event = XTouchEvent {
                        timestamp,
                        message,
                        raw_data: data.to_vec(),
                    };
                    
                    // Try to send event, but don't block or panic
                    let _ = event_tx.try_send(event);
                } else {
                    debug!("Failed to parse MIDI: {}", format_hex(data));
                }
            },
            (),
        ).context("Failed to connect to input port")?;
        
        // Don't ignore any messages - we want everything
        
        self.input_conn = Some(input_conn);
        
        // Connect output
        let midi_out = MidiOutput::new("XTouch-GW-Output")
            .context("Failed to create MIDI output")?;
        
        let port_count = midi_out.port_count();
        debug!("Found {} MIDI output ports", port_count);
        
        let (out_port, port_name) = Self::find_output_port(&midi_out, &self.output_port_name)
            .ok_or_else(|| anyhow::anyhow!("Output port '{}' not found", self.output_port_name))?;
        
        info!("Connecting to output port: {}", port_name);
        
        let output_conn = midi_out.connect(&out_port, "XTouch-GW")
            .context("Failed to connect to output port")?;
        
        self.output_conn = Some(Arc::new(Mutex::new(output_conn)));
        
        info!("X-Touch connected successfully in {:?} mode", self.mode);
        
        // Send initialization sequence if in MCU mode
        if self.mode == XTouchMode::Mcu {
            self.init_mcu_mode().await?;
        }
        
        Ok(())
    }
    
    /// Disconnect from MIDI ports
    pub fn disconnect(&mut self) {
        self.input_conn = None;
        self.output_conn = None;
        info!("X-Touch disconnected");
    }
    
    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.input_conn.is_some() && self.output_conn.is_some()
    }
    
    /// Send a MIDI message to X-Touch
    pub async fn send(&self, message: &MidiMessage) -> Result<()> {
        let output = self.output_conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to output port"))?;
        
        let data = message.encode();
        
        let mut conn = output.lock().unwrap();
        conn.send(&data)
            .context("Failed to send MIDI message")?;
        
        debug!("Sent: {} | {}", format_hex(&data), message);
        
        Ok(())
    }
    
    /// Send raw MIDI bytes to X-Touch
    pub async fn send_raw(&self, data: &[u8]) -> Result<()> {
        let output = self.output_conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to output port"))?;
        
        let mut conn = output.lock().unwrap();
        conn.send(data)
            .context("Failed to send raw MIDI data")?;
        
        debug!("Sent raw: {}", format_hex(data));
        
        Ok(())
    }
    
    /// Take the event receiver (for router to consume)
    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<XTouchEvent>> {
        self.event_rx.take()
    }
    
    /// Initialize MCU mode
    async fn init_mcu_mode(&self) -> Result<()> {
        info!("Initializing MCU mode");
        
        // Send device inquiry
        let device_inquiry = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7];
        self.send_raw(&device_inquiry).await?;
        
        // Reset all faders to center
        for channel in 0..8 {
            let msg = MidiMessage::PitchBend {
                channel,
                value: 8192, // Center position
            };
            self.send(&msg).await?;
        }
        
        // Turn off all LEDs
        for note in 0..128 {
            let msg = MidiMessage::NoteOff {
                channel: 0,
                note,
                velocity: 0,
            };
            self.send(&msg).await?;
        }
        
        Ok(())
    }
    
    /// Set fader position (0-16383 for MCU mode, 0-127 for Ctrl mode)
    pub async fn set_fader(&self, fader_num: u8, value: u16) -> Result<()> {
        if fader_num > 8 {
            bail!("Invalid fader number: {} (must be 0-8)", fader_num);
        }
        
        let message = match self.mode {
            XTouchMode::Mcu => {
                // MCU mode uses PitchBend, channel = fader
                MidiMessage::PitchBend {
                    channel: if fader_num == 8 { 8 } else { fader_num },
                    value: value.min(16383),
                }
            }
            XTouchMode::Ctrl => {
                // Ctrl mode uses CC
                MidiMessage::ControlChange {
                    channel: 0,
                    cc: 70 + fader_num, // CC 70-78 for faders
                    value: (value >> 7) as u8, // Convert to 7-bit
                }
            }
        };
        
        self.send(&message).await
    }
    
    /// Set button LED state
    pub async fn set_button_led(&self, note: u8, on: bool) -> Result<()> {
        let message = if on {
            MidiMessage::NoteOn {
                channel: 0,
                note,
                velocity: 127,
            }
        } else {
            MidiMessage::NoteOff {
                channel: 0,
                note,
                velocity: 0,
            }
        };
        
        self.send(&message).await
    }
    
    /// Set encoder LED ring (0-11 for position, 12-15 for modes)
    pub async fn set_encoder_led(&self, encoder: u8, value: u8) -> Result<()> {
        if encoder > 7 {
            bail!("Invalid encoder number: {} (must be 0-7)", encoder);
        }
        
        let message = MidiMessage::ControlChange {
            channel: 0,
            cc: 48 + encoder, // CC 48-55 for encoder LEDs
            value: value.min(15),
        };
        
        self.send(&message).await
    }
    
    /// Send LCD text (using SysEx)
    pub async fn set_lcd_text(&self, position: u8, line: u8, text: &str) -> Result<()> {
        if position > 7 {
            bail!("Invalid LCD position: {} (must be 0-7)", position);
        }
        if line > 1 {
            bail!("Invalid LCD line: {} (must be 0-1)", line);
        }
        
        // MCU LCD SysEx format
        let mut data = vec![
            0x00, 0x00, 0x66, 0x14, // MCU header
            0x12, // LCD command
            position * 7 + line * 0x38, // Position offset
        ];
        
        // Add text (max 7 chars per position)
        let text_bytes: Vec<u8> = text.chars()
            .take(7)
            .map(|c| (c as u8).min(0x7F))
            .collect();
        
        data.extend_from_slice(&text_bytes);
        
        // Pad with spaces if needed
        for _ in text_bytes.len()..7 {
            data.push(b' ');
        }
        
        let sysex = MidiMessage::SysEx { data };
        self.send(&sysex).await
    }
}

/// Port discovery utilities
pub mod discovery {
    use super::*;
    
    /// Information about a MIDI port
    #[derive(Debug, Clone)]
    pub struct PortInfo {
        pub index: usize,
        pub name: String,
        pub is_virtual: bool,
    }
    
    /// Discover all available MIDI ports
    pub fn discover_all_ports() -> Result<(Vec<PortInfo>, Vec<PortInfo>)> {
        let input_ports = discover_input_ports()?;
        let output_ports = discover_output_ports()?;
        Ok((input_ports, output_ports))
    }
    
    /// Discover input ports
    pub fn discover_input_ports() -> Result<Vec<PortInfo>> {
        let midi_in = MidiInput::new("XTouch-GW-Discovery")?;
        
        let mut port_infos = Vec::new();
        for (index, port) in midi_in.ports().iter().enumerate() {
            if let Ok(name) = midi_in.port_name(port) {
                let is_virtual = name.contains("Virtual") || 
                                 name.contains("loopMIDI") ||
                                 name.contains("IAC");
                                 
                port_infos.push(PortInfo {
                    index,
                    name,
                    is_virtual,
                });
            }
        }
        
        Ok(port_infos)
    }
    
    /// Discover output ports
    pub fn discover_output_ports() -> Result<Vec<PortInfo>> {
        let midi_out = MidiOutput::new("XTouch-GW-Discovery")?;
        
        let mut port_infos = Vec::new();
        for (index, port) in midi_out.ports().iter().enumerate() {
            if let Ok(name) = midi_out.port_name(port) {
                let is_virtual = name.contains("Virtual") || 
                                 name.contains("loopMIDI") ||
                                 name.contains("IAC");
                                 
                port_infos.push(PortInfo {
                    index,
                    name,
                    is_virtual,
                });
            }
        }
        
        Ok(port_infos)
    }
    
    /// Find X-Touch ports automatically
    pub fn find_xtouch_ports() -> Option<(String, String)> {
        // Common X-Touch port name patterns
        let patterns = vec![
            "X-Touch",
            "XTOUCH",
            "Behringer",
            "UM-One", // Common MIDI interface
        ];
        
        if let Ok((inputs, outputs)) = discover_all_ports() {
            for pattern in patterns {
                // Look for matching input
                let input = inputs.iter()
                    .find(|p| p.name.contains(pattern) && !p.is_virtual);
                    
                // Look for matching output
                let output = outputs.iter()
                    .find(|p| p.name.contains(pattern) && !p.is_virtual);
                    
                if let (Some(inp), Some(out)) = (input, output) {
                    return Some((inp.name.clone(), out.name.clone()));
                }
            }
        }
        
        None
    }
    
    /// Print discovered ports for debugging
    pub fn print_ports() {
        println!("\n=== MIDI Input Ports ===");
        if let Ok(ports) = discover_input_ports() {
            for (i, port) in ports.iter().enumerate() {
                let virtual_tag = if port.is_virtual { " [VIRTUAL]" } else { "" };
                println!("  {}: {}{}", i, port.name, virtual_tag);
            }
        }
        
        println!("\n=== MIDI Output Ports ===");
        if let Ok(ports) = discover_output_ports() {
            for (i, port) in ports.iter().enumerate() {
                let virtual_tag = if port.is_virtual { " [VIRTUAL]" } else { "" };
                println!("  {}: {}{}", i, port.name, virtual_tag);
            }
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_port_discovery() {
        // This test just ensures the discovery functions don't panic
        let _ = discovery::discover_input_ports();
        let _ = discovery::discover_output_ports();
        let _ = discovery::find_xtouch_ports();
    }
}