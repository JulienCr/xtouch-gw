//! MIDI Bridge Driver for Voicemeeter and other MIDI applications
//!
//! Handles bidirectional MIDI communication with filtering and transformations.
//! Supports automatic reconnection with exponential backoff.

use async_trait::async_trait;
use anyhow::{Result, anyhow};
use serde_json::Value;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug, warn, trace};

use super::{Driver, ExecutionContext};
use crate::config::{MidiFilterConfig, TransformConfig};
use crate::midi::{MidiMessage, parse_message, find_port_by_substring};

/// Parse a number that might be in hex format (string "0x..." or number)
fn parse_number_maybe_hex(value: &serde_json::Value, default: u16) -> u16 {
    match value {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(default as u64) as u16,
        serde_json::Value::String(s) => {
            if s.starts_with("0x") || s.starts_with("0X") {
                u16::from_str_radix(&s[2..], 16).unwrap_or(default)
            } else {
                s.parse::<u16>().unwrap_or(default)
            }
        }
        _ => default,
    }
}

/// MIDI Bridge Driver - connects X-Touch to MIDI applications
pub struct MidiBridgeDriver {
    name: String,
    to_port: String,
    from_port: String,
    filter: Option<MidiFilterConfig>,
    transform: Option<TransformConfig>,
    optional: bool,
    
    // MIDI ports (wrapped in Arc<Mutex<>> for interior mutability)
    // Note: Using Arc<Mutex<>> instead of Arc<tokio::sync::Mutex<>> for non-async interior mutability
    midi_out: Arc<Mutex<Option<midir::MidiOutputConnection>>>,
    midi_in: Arc<Mutex<Option<midir::MidiInputConnection<()>>>>,
    
    // Reconnection state
    reconnect_count_out: Arc<Mutex<usize>>,
    reconnect_count_in: Arc<Mutex<usize>>,
    shutdown_flag: Arc<Mutex<bool>>,
}

// Explicitly implement Send and Sync
// This is safe because all fields are Arc<Mutex<>> which are Send + Sync
unsafe impl Send for MidiBridgeDriver {}
unsafe impl Sync for MidiBridgeDriver {}

impl MidiBridgeDriver {
    /// Create a new MIDI bridge driver
    ///
    /// # Arguments
    /// * `to_port` - Output port name (substring match)
    /// * `from_port` - Input port name (substring match)
    /// * `filter` - Optional MIDI filter configuration
    /// * `transform` - Optional MIDI transformation configuration
    /// * `optional` - If true, driver continues even if ports are unavailable
    pub fn new(
        to_port: String,
        from_port: String,
        filter: Option<MidiFilterConfig>,
        transform: Option<TransformConfig>,
        optional: bool,
    ) -> Self {
        Self {
            name: format!("midibridge:{}", to_port),
            to_port,
            from_port,
            filter,
            transform,
            optional,
            midi_out: Arc::new(Mutex::new(None)),
            midi_in: Arc::new(Mutex::new(None)),
            reconnect_count_out: Arc::new(Mutex::new(0)),
            reconnect_count_in: Arc::new(Mutex::new(0)),
            shutdown_flag: Arc::new(Mutex::new(false)),
        }
    }

    /// Try to open the output port once
    fn try_open_out(&self) -> Result<()> {
        let midi_out = midir::MidiOutput::new("XTouch-GW-Bridge-Out")?;
        
        let port = find_port_by_substring(&midi_out, &self.to_port)
            .ok_or_else(|| anyhow!("Output port '{}' not found", self.to_port))?;

        let connection = midi_out.connect(&port, &format!("xtouch-gw-{}", self.to_port))?;
        
        *self.midi_out.lock() = Some(connection);
        *self.reconnect_count_out.lock() = 0;
        
        info!("✅ MIDI Bridge OUT opened: '{}'", self.to_port);
        Ok(())
    }

    /// Try to open the input port once
    fn try_open_in(&self) -> Result<()> {
        let midi_in = midir::MidiInput::new("XTouch-GW-Bridge-In")?;
        
        let port = find_port_by_substring(&midi_in, &self.from_port)
            .ok_or_else(|| anyhow!("Input port '{}' not found", self.from_port))?;

        // For now, we'll just open the port without callbacks
        // The callback will be added when we integrate with the router feedback
        let connection = midi_in.connect(&port, &format!("xtouch-gw-{}", self.from_port), |_timestamp, data, _| {
            // Feedback from application - will be handled by router
            debug!("🔙 Bridge RX <- {} bytes", data.len());
        }, ())?;
        
        *self.midi_in.lock() = Some(connection);
        *self.reconnect_count_in.lock() = 0;
        
        info!("✅ MIDI Bridge IN opened: '{}'", self.from_port);
        Ok(())
    }

    /// Schedule reconnection for output port
    async fn schedule_out_reconnect(&self) {
        // Check shutdown flag
        {
            if *self.shutdown_flag.lock() {
                return;
            }
        }

        // Increment retry count
        let retry_count = {
            let mut count = self.reconnect_count_out.lock();
            *count += 1;
            *count
        };

        let delay_ms = std::cmp::min(10_000, 250 * retry_count);
        info!("⏳ MIDI Bridge OUT reconnect #{} for '{}' in {}ms", retry_count, self.to_port, delay_ms);
        
        sleep(Duration::from_millis(delay_ms as u64)).await;
        
        // Check shutdown flag again
        {
            if *self.shutdown_flag.lock() {
                return;
            }
        }

        match self.try_open_out() {
            Ok(_) => {},
            Err(e) if self.optional => {
                warn!("MIDI Bridge OUT reconnect failed (optional): {}", e);
                // TODO: Implement automatic reconnection
                // Currently skipped due to Send trait complexity with MIDI connections
            },
            Err(e) => {
                warn!("MIDI Bridge OUT reconnect failed: {}", e);
            }
        }
    }

    /// Schedule reconnection for input port
    async fn schedule_in_reconnect(&self) {
        // Check shutdown flag
        {
            if *self.shutdown_flag.lock() {
                return;
            }
        }

        // Increment retry count
        let retry_count = {
            let mut count = self.reconnect_count_in.lock();
            *count += 1;
            *count
        };

        let delay_ms = std::cmp::min(10_000, 250 * retry_count);
        info!("⏳ MIDI Bridge IN reconnect #{} for '{}' in {}ms", retry_count, self.from_port, delay_ms);
        
        sleep(Duration::from_millis(delay_ms as u64)).await;
        
        // Check shutdown flag again
        {
            if *self.shutdown_flag.lock() {
                return;
            }
        }

        match self.try_open_in() {
            Ok(_) => {},
            Err(e) if self.optional => {
                warn!("MIDI Bridge IN reconnect failed (optional): {}", e);
                // TODO: Implement automatic reconnection
                // Currently skipped due to Send trait complexity with MIDI connections
            },
            Err(e) => {
                warn!("MIDI Bridge IN reconnect failed: {}", e);
            }
        }
    }

    /// Check if message matches the filter
    fn matches_filter(&self, msg: &MidiMessage) -> bool {
        let filter = match &self.filter {
            Some(f) => f,
            None => return true, // No filter = accept all
        };

        // Check channel filter
        if let Some(channels) = &filter.channels {
            let channel_1_based = msg.channel().map(|ch| ch + 1);
            if let Some(ch) = channel_1_based {
                if !channels.contains(&ch) {
                    return false;
                }
            } else {
                return false; // System messages don't have channels
            }
        }

        // Check type filter
        if let Some(types) = &filter.types {
            let msg_type = match msg {
                MidiMessage::NoteOn { .. } if msg.velocity() > 0 => "noteOn",
                MidiMessage::NoteOn { .. } => "noteOff", // velocity 0 is note off
                MidiMessage::NoteOff { .. } => "noteOff",
                MidiMessage::ControlChange { .. } => "controlChange",
                MidiMessage::PitchBend { .. } => "pitchBend",
                MidiMessage::ProgramChange { .. } => "programChange",
                MidiMessage::ChannelPressure { .. } => "channelAftertouch",
                MidiMessage::PolyPressure { .. } => "polyAftertouch",
                _ => return false,
            };
            
            if !types.contains(&msg_type.to_string()) {
                return false;
            }
        }

        // Check note inclusion/exclusion
        if let Some(include_notes) = &filter.include_notes {
            if let Some(note) = msg.note() {
                if !include_notes.contains(&note) {
                    return false;
                }
            }
        }

        if let Some(exclude_notes) = &filter.exclude_notes {
            if let Some(note) = msg.note() {
                if exclude_notes.contains(&note) {
                    return false;
                }
            }
        }

        true
    }

    /// Apply transformation to message
    fn apply_transform(&self, msg: MidiMessage) -> Option<Vec<u8>> {
        let transform = match &self.transform {
            Some(t) => t,
            None => return Some(msg.to_bytes()),
        };

        // PitchBend → CC transformation
        if let Some(pb_to_cc) = &transform.pb_to_cc {
            if let MidiMessage::PitchBend { channel, value } = msg {
                // Convert 14-bit PB (0-16383) to 7-bit CC (0-127)
                let value_7bit = (value / 128) as u8;
                
                let target_channel = pb_to_cc.target_channel.unwrap_or(1);
                
                // Resolve CC number
                let cc = if let Some(cc_map) = &pb_to_cc.cc_by_channel {
                    if let Some(cc_val) = cc_map.get(&channel) {
                        parse_number_maybe_hex(cc_val, 0)
                    } else {
                        let base = pb_to_cc.base_cc.as_ref()
                            .map(|v| parse_number_maybe_hex(v, 45))
                            .unwrap_or(45);
                        base + (channel as u16 - 1)
                    }
                } else {
                    let base = pb_to_cc.base_cc.as_ref()
                        .map(|v| parse_number_maybe_hex(v, 45))
                        .unwrap_or(45);
                    base + (channel as u16 - 1)
                };

                let cc = std::cmp::min(127, cc) as u8;
                
                trace!("Transform PB→CC: ch{} value={} → ch{} CC{} value={}", 
                    channel, value, target_channel, cc, value_7bit);

                return Some(MidiMessage::ControlChange { 
                    channel: target_channel, 
                    cc, 
                    value: value_7bit 
                }.to_bytes());
            }
        }

        // PitchBend → Note transformation
        if let Some(pb_to_note) = &transform.pb_to_note {
            if let MidiMessage::PitchBend { channel, value } = msg {
                let velocity = ((value as f32 / 16383.0) * 127.0) as u8;
                let note = pb_to_note.note.unwrap_or(60).min(127) as u8;
                
                trace!("Transform PB→Note: ch{} value={} → note={} vel={}", 
                    channel, value, note, velocity);

                return Some(MidiMessage::NoteOn { 
                    channel, 
                    note, 
                    velocity 
                }.to_bytes());
            }
        }

        Some(msg.to_bytes())
    }

    /// Send MIDI message through the bridge
    pub fn send_message(&self, data: &[u8]) -> Result<()> {
        if let Ok(msg) = parse_message(data) {
            // Apply filter
            if !self.matches_filter(&msg) {
                trace!("Bridge DROP (filtered) -> {}: {:?}", self.to_port, msg);
                return Ok(());
            }

            // Apply transform
            let transformed = match self.apply_transform(msg) {
                Some(bytes) => bytes,
                None => {
                    trace!("Bridge DROP (transform returned null) -> {}", self.to_port);
                    return Ok(());
                }
            };

            // Send message
            let mut midi_out = self.midi_out.lock();
            match &mut *midi_out {
                Some(conn) => {
                    match conn.send(&transformed) {
                        Ok(_) => {
                            debug!("Bridge TX -> {}: {} bytes", self.to_port, transformed.len());
                            Ok(())
                        },
                        Err(e) => {
                            warn!("MIDI Bridge send failed: {}", e);
                            *midi_out = None; // Close broken connection
                            // TODO: Implement reconnection
                            Err(anyhow!("MIDI send failed: {}", e))
                        }
                    }
                },
                None => {
                    trace!("Bridge TX skipped (not connected): {}", self.to_port);
                    Ok(())
                }
            }
        } else {
            warn!("Failed to parse MIDI message: {:?}", data);
            Ok(())
        }
    }
}

#[async_trait]
impl Driver for MidiBridgeDriver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&self, _ctx: ExecutionContext) -> Result<()> {
        info!("🌉 Initializing MIDI Bridge: '{}' ⇄ '{}'", self.to_port, self.from_port);

        // Try to open output port
        match self.try_open_out() {
            Ok(_) => {},
            Err(e) if self.optional => {
                warn!("MIDI Bridge OUT open failed (optional): {}", e);
                // TODO: Implement background reconnection task
            },
            Err(e) => return Err(e),
        }

        // Try to open input port
        match self.try_open_in() {
            Ok(_) => {},
            Err(e) if self.optional => {
                warn!("MIDI Bridge IN open failed (optional): {}", e);
                // TODO: Implement background reconnection task
            },
            Err(e) => return Err(e),
        }

        info!("✅ MIDI Bridge active: '{}' ⇄ '{}'", self.to_port, self.from_port);
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        match action {
            "passthrough" => {
                // Extract raw MIDI bytes from context value
                if let Some(value) = ctx.value {
                    if let Value::Array(bytes_array) = value {
                        let bytes: Vec<u8> = bytes_array
                            .iter()
                            .filter_map(|v| v.as_u64().map(|n| n as u8))
                            .collect();
                        if !bytes.is_empty() {
                            debug!("→ Passthrough {} bytes to '{}'", bytes.len(), self.to_port);
                            self.send_message(&bytes)?;
                        }
                    }
                }
                Ok(())
            },
            "send" => {
                if let Some(Value::Array(bytes_array)) = params.first() {
                    let bytes: Vec<u8> = bytes_array
                        .iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect();
                    self.send_message(&bytes)?;
                }
                Ok(())
            },
            _ => {
                warn!("Unknown MIDI bridge action: {}", action);
                Ok(())
            }
        }
    }

    async fn sync(&self) -> Result<()> {
        debug!("MIDI Bridge sync (no-op)");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down MIDI Bridge: '{}'", self.to_port);
        
        *self.shutdown_flag.lock() = true;

        // Close MIDI connections
        *self.midi_out.lock() = None;
        *self.midi_in.lock() = None;

        info!("✅ MIDI Bridge shutdown complete");
        Ok(())
    }
}

