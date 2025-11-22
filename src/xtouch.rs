//! Behringer X-Touch driver
//!
//! Handles MIDI communication with the X-Touch control surface.

pub mod fader_setpoint;
pub mod pitch_bend_squelch;

use pitch_bend_squelch::PitchBendSquelch;

use anyhow::{bail, Context, Result};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::config::{AppConfig, XTouchMode};
use crate::midi::{format_hex, MidiMessage};

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

    /// Pitch bend squelch for preventing feedback loops
    pb_squelch: PitchBendSquelch,
}

impl XTouchDriver {
    /// Create a new X-Touch driver
    pub fn new(config: &AppConfig) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel(1000);

        let mode = config
            .xtouch
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
            pb_squelch: PitchBendSquelch::new(),
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

        info!(
            "Connecting to X-Touch - Input: '{}', Output: '{}'",
            self.input_port_name, self.output_port_name
        );

        // Connect input
        let midi_in = MidiInput::new("XTouch-GW-Input").context("Failed to create MIDI input")?;

        let port_count = midi_in.port_count();
        debug!("Found {} MIDI input ports", port_count);

        let (in_port, port_name) = Self::find_input_port(&midi_in, &self.input_port_name)
            .ok_or_else(|| anyhow::anyhow!("Input port '{}' not found", self.input_port_name))?;

        info!("Connecting to input port: {}", port_name);

        // Set up callback for incoming MIDI
        let event_tx = self.event_tx.clone();

        let pb_squelch = self.pb_squelch.clone(); // Clone for callback

        let input_conn = midi_in
            .connect(
                &in_port,
                "XTouch-GW",
                move |_timestamp, data, _| {
                    let timestamp = Instant::now();

                    // Check if this is a pitch bend message
                    if data.len() >= 1 {
                        let status = data[0];
                        let message_type_nibble = (status & 0xF0) >> 4;
                        let is_pitch_bend = message_type_nibble == 0xE; // 0xE0-0xEF

                        // Suppress pitch bend if squelched
                        if is_pitch_bend && pb_squelch.is_squelched() {
                            debug!("Suppressing squelched pitch bend: {:02X?}", data);
                            return; // Don't forward message
                        }
                    }

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
            )
            .context("Failed to connect to input port")?;

        // Don't ignore any messages - we want everything

        self.input_conn = Some(input_conn);

        // Connect output
        let midi_out =
            MidiOutput::new("XTouch-GW-Output").context("Failed to create MIDI output")?;

        let port_count = midi_out.port_count();
        debug!("Found {} MIDI output ports", port_count);

        let (out_port, port_name) = Self::find_output_port(&midi_out, &self.output_port_name)
            .ok_or_else(|| anyhow::anyhow!("Output port '{}' not found", self.output_port_name))?;

        info!("Connecting to output port: {}", port_name);

        let output_conn = midi_out
            .connect(&out_port, "XTouch-GW")
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
        let output = self
            .output_conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to output port"))?;

        let data = message.encode();

        let mut conn = output.lock().unwrap();
        conn.send(&data).context("Failed to send MIDI message")?;

        debug!("Sent: {} | {}", format_hex(&data), message);

        Ok(())
    }

    /// Send raw MIDI data directly to X-Touch (synchronous, for callbacks)
    ///
    /// Used for feedback routing from MIDI bridge drivers.
    /// This is a non-async version safe to call from within MIDI callbacks.
    pub fn send_raw_sync(&self, data: &[u8]) -> Result<()> {
        let output = self
            .output_conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to output port"))?;

        let mut conn = output.lock().unwrap();
        conn.send(data).context("Failed to send raw MIDI data")?;

        debug!("Sent raw feedback: {}", format_hex(data));

        Ok(())
    }

    /// Send raw MIDI bytes to X-Touch
    pub async fn send_raw(&self, data: &[u8]) -> Result<()> {
        let output = self
            .output_conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to output port"))?;

        let mut conn = output.lock().unwrap();
        conn.send(data).context("Failed to send raw MIDI data")?;

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
            },
            XTouchMode::Ctrl => {
                // Ctrl mode uses CC
                MidiMessage::ControlChange {
                    channel: 0,
                    cc: 70 + fader_num,        // CC 70-78 for faders
                    value: (value >> 7) as u8, // Convert to 7-bit
                }
            },
        };

        self.send(&message).await;

        Ok(())
    }

    /// Activate pitch bend squelch for the specified duration (milliseconds)
    ///
    /// This prevents incoming pitch bend messages from being processed for the
    /// specified duration, helping to prevent feedback loops when commanding
    /// motorized faders from app feedback.
    pub fn activate_squelch(&self, duration_ms: u64) {
        self.pb_squelch.squelch(duration_ms);
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

    /// Send LCD strip text (upper and lower lines)
    ///
    /// Matches TypeScript sendLcdStripText() from api-lcd.ts
    pub async fn send_lcd_strip_text(
        &self,
        strip_index: u8,
        upper: &str,
        lower: &str,
    ) -> Result<()> {
        if strip_index > 7 {
            bail!("Invalid LCD strip index: {} (must be 0-7)", strip_index);
        }

        // Convert text to 7-byte ASCII arrays
        let upper_bytes = Self::ascii7(upper, 7);
        let lower_bytes = Self::ascii7(lower, 7);

        // SysEx header for X-Touch LCD
        let header = vec![0x00, 0x00, 0x66, 0x14, 0x12];

        // Position for upper line (0x00 + strip * 7)
        let pos_top = 0x00 + strip_index * 7;

        // Position for lower line (0x38 + strip * 7)
        let pos_bot = 0x38 + strip_index * 7;

        // Send upper line
        let mut upper_data = header.clone();
        upper_data.push(pos_top);
        upper_data.extend_from_slice(&upper_bytes);
        self.send(&MidiMessage::SysEx { data: upper_data }).await?;

        // Send lower line
        let mut lower_data = header;
        lower_data.push(pos_bot);
        lower_data.extend_from_slice(&lower_bytes);
        self.send(&MidiMessage::SysEx { data: lower_data }).await?;

        Ok(())
    }

    /// Convert text to 7-bit ASCII array with specific length
    fn ascii7(text: &str, length: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(length);

        for (i, ch) in text.chars().enumerate() {
            if i >= length {
                break;
            }

            // Only printable ASCII (0x20-0x7E), otherwise space
            let code = ch as u32;
            let byte = if (0x20..=0x7E).contains(&code) {
                code as u8
            } else {
                0x20 // Space
            };
            bytes.push(byte);
        }

        // Pad with spaces to reach desired length
        while bytes.len() < length {
            bytes.push(0x20);
        }

        bytes
    }

    /// Send only lower line LCD text (for value overlay)
    pub async fn send_lcd_strip_lower_text(&self, strip_index: u8, lower: &str) -> Result<()> {
        if strip_index > 7 {
            bail!("Invalid LCD strip index: {} (must be 0-7)", strip_index);
        }

        let lower_bytes = Self::ascii7(lower, 7);

        let header = vec![0x00, 0x00, 0x66, 0x14, 0x12];
        let pos_bot = 0x38 + strip_index * 7;

        let mut data = header;
        data.push(pos_bot);
        data.extend_from_slice(&lower_bytes);

        self.send(&MidiMessage::SysEx { data }).await
    }

    /// Set LCD colors for all 8 strips (firmware >= 1.22)
    ///
    /// Colors: 0=black, 1=red, 2=green, 3=yellow, 4=blue, 5=magenta, 6=cyan, 7=white
    pub async fn set_lcd_colors(&self, colors: &[u8]) -> Result<()> {
        // Prepare payload (8 bytes, pad with 0 if needed)
        let mut payload = Vec::with_capacity(8);
        for i in 0..8 {
            let color = colors.get(i).copied().unwrap_or(0);
            payload.push(color.min(7)); // Clamp to 0-7
        }

        // SysEx: F0 00 00 66 14 72 [8 colors] F7
        let data = vec![
            0x00, 0x00, 0x66, 0x14, 0x72, payload[0], payload[1], payload[2], payload[3],
            payload[4], payload[5], payload[6], payload[7],
        ];

        self.send(&MidiMessage::SysEx { data }).await
    }

    /// Set 7-segment display text (timecode display)
    ///
    /// Matches TypeScript setSevenSegmentText() from api-lcd.ts
    pub async fn set_seven_segment_text(&self, text: &str) -> Result<()> {
        // Center text to 12 characters
        let centered = Self::center_to_length(text, 12);

        // Convert each character to 7-segment encoding
        let segs: Vec<u8> = centered
            .chars()
            .take(12)
            .map(Self::seven_seg_for_char)
            .collect();

        // Dots (disabled by default)
        let dots1 = 0x00;
        let dots2 = 0x00;

        // Send to both device IDs (0x14 and 0x15)
        for device_id in [0x14, 0x15] {
            let mut data = vec![0x00, 0x20, 0x32, device_id, 0x37];
            data.extend_from_slice(&segs);
            data.push(dots1);
            data.push(dots2);

            self.send(&MidiMessage::SysEx { data }).await?;
        }

        Ok(())
    }

    /// Center text to specific length (for 7-segment display)
    fn center_to_length(text: &str, length: usize) -> String {
        if text.len() >= length {
            text.chars().take(length).collect()
        } else {
            let padding = length - text.len();
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;

            format!("{}{}{}", " ".repeat(left_pad), text, " ".repeat(right_pad))
        }
    }

    /// Convert character to 7-segment display encoding
    fn seven_seg_for_char(ch: char) -> u8 {
        // Basic 7-segment encoding for common characters
        // This is a simplified version - full implementation in TypeScript seg7.ts
        match ch {
            '0' => 0x3F,
            '1' => 0x06,
            '2' => 0x5B,
            '3' => 0x4F,
            '4' => 0x66,
            '5' => 0x6D,
            '6' => 0x7D,
            '7' => 0x07,
            '8' => 0x7F,
            '9' => 0x6F,
            'A' | 'a' => 0x77,
            'B' | 'b' => 0x7C,
            'C' | 'c' => 0x39,
            'D' | 'd' => 0x5E,
            'E' | 'e' => 0x79,
            'F' | 'f' => 0x71,
            'H' | 'h' => 0x76,
            'L' | 'l' => 0x38,
            'O' | 'o' => 0x3F,
            'P' | 'p' => 0x73,
            'U' | 'u' => 0x3E,
            '-' => 0x40,
            '_' => 0x08,
            ' ' => 0x00,
            _ => 0x00, // Unknown chars = blank
        }
    }

    /// Clear all LCD strips (text and colors)
    pub async fn clear_all_lcds(&self) -> Result<()> {
        // Clear text on all 8 strips
        for i in 0..8 {
            self.send_lcd_strip_text(i, "", "").await?;
        }

        // Reset colors to black
        let black_colors = [0u8; 8];
        self.set_lcd_colors(&black_colors).await?;

        // Clear 7-segment display
        self.set_seven_segment_text("").await?;

        Ok(())
    }

    /// Apply LCD configuration for active page
    ///
    /// Matches TypeScript applyLcdForActivePage() from ui/lcd.ts
    pub async fn apply_lcd_for_page(
        &self,
        labels: Option<&Vec<crate::config::LcdLabel>>,
        colors: Option<&Vec<u8>>,
        page_name: &str,
    ) -> Result<()> {
        // Clear all strips first to avoid leaks from previous pages
        for i in 0..8 {
            self.send_lcd_strip_text(i, "", "").await?;
        }

        // Apply labels if provided
        if let Some(labels) = labels {
            for (i, label) in labels.iter().enumerate().take(8) {
                let (upper, lower) = match label {
                    crate::config::LcdLabel::Simple(text) => {
                        // Split on newline
                        let parts: Vec<&str> = text.splitn(2, '\n').collect();
                        (
                            parts.get(0).copied().unwrap_or(""),
                            parts.get(1).copied().unwrap_or(""),
                        )
                    },
                    crate::config::LcdLabel::Structured { upper, lower } => (
                        upper.as_deref().unwrap_or(""),
                        lower.as_deref().unwrap_or(""),
                    ),
                };

                self.send_lcd_strip_text(i as u8, upper, lower).await?;
            }
        }

        // Apply colors if provided, otherwise set to black
        if let Some(colors) = colors {
            self.set_lcd_colors(colors).await?;
        } else {
            let black_colors = [0u8; 8];
            self.set_lcd_colors(&black_colors).await?;
        }

        // Display page name on 7-segment display
        self.set_seven_segment_text(page_name).await?;

        Ok(())
    }

    /// Reset all hardware to clean state
    ///
    /// Matches TypeScript resetAll() from api-midi.ts
    /// - Turns off all button LEDs (notes 0-101)
    /// - Resets all faders to 0
    /// - Optionally clears LCD displays
    pub async fn reset_all(&self, clear_lcds: bool) -> Result<()> {
        info!("🔄 Resetting X-Touch hardware...");

        // Turn off all button LEDs (notes 0-101 on channel 1)
        // Matches TypeScript setAllButtonsVelocity(driver, 1, 0, 101, 0, 2)
        for note in 0..=101 {
            let message = MidiMessage::NoteOn {
                channel: 0, // Channel 1 (0-indexed)
                note,
                velocity: 0, // Velocity 0 turns off LED
            };
            self.send(&message).await;
            // Small delay between messages to avoid overwhelming MIDI buffer
            tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
        }

        // Reset all faders to 0 (faders 0-8: strips 1-8 + master)
        // Matches TypeScript resetFadersToZero(driver, [1,2,3,4,5,6,7,8,9])
        for fader_num in 0..=8 {
            self.set_fader(fader_num, 0).await?;
        }

        // Optionally clear LCD displays
        if clear_lcds {
            self.clear_all_lcds().await?;
        }

        info!("✅ X-Touch hardware reset complete");
        Ok(())
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
                let is_virtual =
                    name.contains("Virtual") || name.contains("loopMIDI") || name.contains("IAC");

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
                let is_virtual =
                    name.contains("Virtual") || name.contains("loopMIDI") || name.contains("IAC");

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
                let input = inputs
                    .iter()
                    .find(|p| p.name.contains(pattern) && !p.is_virtual);

                // Look for matching output
                let output = outputs
                    .iter()
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
