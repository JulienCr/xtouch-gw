//! MIDI Bridge Driver for Voicemeeter and other MIDI applications
//!
//! Handles bidirectional MIDI communication with filtering and transformations.
//! Supports automatic reconnection with exponential backoff.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, trace, warn};

use super::{Driver, ExecutionContext};
use crate::config::{MidiFilterConfig, TransformConfig};
use crate::midi::{find_port_by_substring, parse_message, MidiMessage};

/// Callback type for MIDI feedback from applications
pub type FeedbackCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Cap on the per-port reconnect retry counter. Past this point the backoff
/// delay is already saturated at 10s and further growth only risks overflow
/// in the delay math or in metrics export.
const RECONNECT_COUNTER_CAP: usize = 40;

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
        },
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

    // Feedback callback for routing MIDI from app to X-Touch
    feedback_callback: Arc<Mutex<Option<FeedbackCallback>>>,

    // Connection status tracking
    status_callbacks: Arc<parking_lot::RwLock<Vec<crate::tray::StatusCallback>>>,
    current_status: Arc<parking_lot::RwLock<crate::tray::ConnectionStatus>>,

    // Activity tracking
    activity_tracker: Arc<parking_lot::RwLock<Option<Arc<crate::tray::ActivityTracker>>>>,

    // Reconnection state
    reconnect_count_out: Arc<Mutex<usize>>,
    reconnect_count_in: Arc<Mutex<usize>>,
    shutdown_flag: Arc<Mutex<bool>>,
    /// Single-flight guards: `true` while a reconnect loop for that direction
    /// is running, so a send failure or health probe can't spawn a second
    /// loop racing the first on the exclusive Windows port.
    reconnecting_out: Arc<AtomicBool>,
    reconnecting_in: Arc<AtomicBool>,
    /// Ensures the port-health monitor task is spawned at most once.
    health_monitor_started: Arc<AtomicBool>,
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
            feedback_callback: Arc::new(Mutex::new(None)),
            status_callbacks: Arc::new(parking_lot::RwLock::new(Vec::new())),
            current_status: Arc::new(parking_lot::RwLock::new(
                crate::tray::ConnectionStatus::Disconnected,
            )),
            activity_tracker: Arc::new(parking_lot::RwLock::new(None)),
            reconnect_count_out: Arc::new(Mutex::new(0)),
            reconnect_count_in: Arc::new(Mutex::new(0)),
            shutdown_flag: Arc::new(Mutex::new(false)),
            reconnecting_out: Arc::new(AtomicBool::new(false)),
            reconnecting_in: Arc::new(AtomicBool::new(false)),
            health_monitor_started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Clone every shared `Arc` handle for use in a spawned background task.
    /// Centralises the field-by-field clone so adding a shared field only
    /// needs updating one place (this was previously duplicated inline twice
    /// in `init`).
    fn clone_handle(&self) -> Self {
        Self {
            name: self.name.clone(),
            to_port: self.to_port.clone(),
            from_port: self.from_port.clone(),
            filter: self.filter.clone(),
            transform: self.transform.clone(),
            optional: self.optional,
            midi_out: self.midi_out.clone(),
            midi_in: self.midi_in.clone(),
            feedback_callback: self.feedback_callback.clone(),
            status_callbacks: self.status_callbacks.clone(),
            current_status: self.current_status.clone(),
            activity_tracker: self.activity_tracker.clone(),
            reconnect_count_out: self.reconnect_count_out.clone(),
            reconnect_count_in: self.reconnect_count_in.clone(),
            shutdown_flag: self.shutdown_flag.clone(),
            reconnecting_out: self.reconnecting_out.clone(),
            reconnecting_in: self.reconnecting_in.clone(),
            health_monitor_started: self.health_monitor_started.clone(),
        }
    }

    /// Spawn the OUT reconnect loop unless one is already running.
    fn spawn_out_reconnect(&self) {
        if self.reconnecting_out.swap(true, Ordering::AcqRel) {
            return; // a loop is already running
        }
        let me = self.clone_handle();
        tokio::spawn(async move {
            me.schedule_out_reconnect().await;
            me.reconnecting_out.store(false, Ordering::Release);
        });
    }

    /// Spawn the IN reconnect loop unless one is already running.
    fn spawn_in_reconnect(&self) {
        if self.reconnecting_in.swap(true, Ordering::AcqRel) {
            return; // a loop is already running
        }
        let me = self.clone_handle();
        tokio::spawn(async move {
            me.schedule_in_reconnect().await;
            me.reconnecting_in.store(false, Ordering::Release);
        });
    }

    /// True if the configured OUT port is currently enumerable. On a probe
    /// failure returns `true` (assume present) so a transient enumeration
    /// error never tears down a working connection.
    fn out_port_present(&self) -> bool {
        midir::MidiOutput::new("XTouch-GW-Bridge-Probe")
            .ok()
            .map(|m| find_port_by_substring(&m, &self.to_port).is_some())
            .unwrap_or(true)
    }

    /// True if the configured IN port is currently enumerable. See
    /// `out_port_present` for the probe-failure policy.
    fn in_port_present(&self) -> bool {
        midir::MidiInput::new("XTouch-GW-Bridge-Probe")
            .ok()
            .map(|m| find_port_by_substring(&m, &self.from_port).is_some())
            .unwrap_or(true)
    }

    /// Spawn the periodic port-health monitor (at most once).
    ///
    /// A midir *input* connection goes silent without surfacing any error when
    /// the peer's output port disappears (app restart, USB replug) — the
    /// callback simply stops firing, so feedback to the X-Touch dies with no
    /// log. Polling port presence lets us detect that and reconnect. The same
    /// poll also catches an OUT port that vanished with no send in flight to
    /// notice it.
    fn spawn_health_monitor(&self) {
        if self.health_monitor_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone_handle();
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(3000)).await;
                if *me.shutdown_flag.lock() {
                    return;
                }
                if me.midi_out.lock().is_some() && !me.out_port_present() {
                    warn!(
                        "MIDI Bridge OUT port '{}' disappeared; reconnecting",
                        me.to_port
                    );
                    *me.midi_out.lock() = None;
                    me.update_status();
                    me.spawn_out_reconnect();
                }
                if me.midi_in.lock().is_some() && !me.in_port_present() {
                    warn!(
                        "MIDI Bridge IN port '{}' disappeared; reconnecting",
                        me.from_port
                    );
                    *me.midi_in.lock() = None;
                    me.update_status();
                    me.spawn_in_reconnect();
                }
            }
        });
    }

    /// Set the feedback callback for routing MIDI from app to X-Touch
    pub fn set_feedback_callback(&self, callback: FeedbackCallback) {
        *self.feedback_callback.lock() = Some(callback);
    }

    /// Check current connection status based on port states
    fn compute_status(&self) -> crate::tray::ConnectionStatus {
        let out_connected = self.midi_out.lock().is_some();
        let in_connected = self.midi_in.lock().is_some();

        if out_connected && in_connected {
            crate::tray::ConnectionStatus::Connected
        } else {
            // Check if reconnecting
            let out_count = *self.reconnect_count_out.lock();
            let in_count = *self.reconnect_count_in.lock();
            let max_count = out_count.max(in_count);

            if max_count > 0 {
                crate::tray::ConnectionStatus::Reconnecting { attempt: max_count }
            } else {
                crate::tray::ConnectionStatus::Disconnected
            }
        }
    }

    /// Emit connection status to all subscribers
    fn emit_status(&self, status: crate::tray::ConnectionStatus) {
        *self.current_status.write() = status.clone();
        for callback in self.status_callbacks.read().iter() {
            callback(status.clone());
        }
    }

    /// Update and emit status based on current port states
    fn update_status(&self) {
        let status = self.compute_status();
        self.emit_status(status);
    }

    /// Try to open the output port once
    fn try_open_out(&self) -> Result<()> {
        let midi_out = midir::MidiOutput::new("XTouch-GW-Bridge-Out")?;

        let port = find_port_by_substring(&midi_out, &self.to_port)
            .ok_or_else(|| anyhow!("Output port '{}' not found", self.to_port))?;

        let connection = midi_out.connect(&port, &format!("xtouch-gw-{}", self.to_port))?;

        *self.midi_out.lock() = Some(connection);
        *self.reconnect_count_out.lock() = 0;

        // Update connection status
        self.update_status();

        debug!("MIDI Bridge OUT opened: '{}'", self.to_port);
        Ok(())
    }

    /// Try to open the input port once
    fn try_open_in(&self) -> Result<()> {
        let midi_in = midir::MidiInput::new("XTouch-GW-Bridge-In")?;

        let port = find_port_by_substring(&midi_in, &self.from_port)
            .ok_or_else(|| anyhow!("Input port '{}' not found", self.from_port))?;

        // Clone the callback Arc and activity tracker for use in the MIDI callback
        let feedback_callback = self.feedback_callback.clone();
        let activity_tracker = self.activity_tracker.clone();
        let driver_name = self.name.clone();

        let connection = midi_in.connect(
            &port,
            &format!("xtouch-gw-{}", self.from_port),
            move |_timestamp, data, _| {
                debug!("🔙 Bridge RX <- {} bytes: {:02X?}", data.len(), data);

                // Record inbound activity
                if let Some(ref tracker) = *activity_tracker.read() {
                    tracker.record(&driver_name, crate::tray::ActivityDirection::Inbound);
                }

                // Call the feedback callback if set
                if let Some(callback) = feedback_callback.lock().as_ref() {
                    callback(data);
                }
            },
            (),
        )?;

        *self.midi_in.lock() = Some(connection);
        *self.reconnect_count_in.lock() = 0;

        // Update connection status
        self.update_status();

        debug!("MIDI Bridge IN opened: '{}'", self.from_port);
        Ok(())
    }

    /// Schedule reconnection for output port. See `reconnect_loop`.
    async fn schedule_out_reconnect(&self) {
        self.reconnect_loop("OUT", &self.to_port, &self.reconnect_count_out, |s| {
            s.try_open_out()
        })
        .await;
    }

    /// Schedule reconnection for input port. See `reconnect_loop`.
    async fn schedule_in_reconnect(&self) {
        self.reconnect_loop("IN", &self.from_port, &self.reconnect_count_in, |s| {
            s.try_open_in()
        })
        .await;
    }

    /// Generic exponential-backoff reconnect loop shared by IN and OUT.
    ///
    /// Uses an iterative loop (not recursive `Box::pin`) so retrying a
    /// permanently-absent port (`optional: true`) doesn't accumulate boxed
    /// futures. The retry counter is `saturating_add`-capped so the delay
    /// math can't overflow and the value stays meaningful for metrics.
    async fn reconnect_loop(
        &self,
        direction: &str,
        port_name: &str,
        counter: &Mutex<usize>,
        try_open: impl Fn(&Self) -> Result<()>,
    ) {
        loop {
            if *self.shutdown_flag.lock() {
                return;
            }

            let retry_count = {
                let mut count = counter.lock();
                *count = count.saturating_add(1).min(RECONNECT_COUNTER_CAP);
                *count
            };

            let delay_ms = std::cmp::min(10_000, 250 * retry_count);
            debug!(
                "MIDI Bridge {} reconnect #{} for '{}' in {}ms",
                direction, retry_count, port_name, delay_ms
            );

            self.update_status();
            sleep(Duration::from_millis(delay_ms as u64)).await;

            if *self.shutdown_flag.lock() {
                return;
            }

            match try_open(self) {
                Ok(_) => return,
                Err(e) => warn!("MIDI Bridge {} reconnect failed: {}", direction, e),
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
                // Use centralized conversion function (matches TypeScript to7bitFrom14bit)
                let value_7bit = crate::midi::convert::to_7bit_from_14bit(value);

                let target_channel = pb_to_cc.target_channel.unwrap_or(1);

                // Resolve CC number
                let cc = if let Some(cc_map) = &pb_to_cc.cc_by_channel {
                    if let Some(cc_val) = cc_map.get(&channel) {
                        parse_number_maybe_hex(cc_val, 0)
                    } else {
                        let base = pb_to_cc
                            .base_cc
                            .as_ref()
                            .map(|v| parse_number_maybe_hex(v, 45))
                            .unwrap_or(45);
                        base + (channel as u16 - 1)
                    }
                } else {
                    let base = pb_to_cc
                        .base_cc
                        .as_ref()
                        .map(|v| parse_number_maybe_hex(v, 45))
                        .unwrap_or(45);
                    base + (channel as u16 - 1)
                };

                let cc = std::cmp::min(127, cc) as u8;

                trace!(
                    "Transform PB→CC: ch{} value={} → ch{} CC{} value={}",
                    channel,
                    value,
                    target_channel,
                    cc,
                    value_7bit
                );

                return Some(
                    MidiMessage::ControlChange {
                        channel: target_channel,
                        cc,
                        value: value_7bit,
                    }
                    .to_bytes(),
                );
            }
        }

        // PitchBend → Note transformation
        if let Some(pb_to_note) = &transform.pb_to_note {
            if let MidiMessage::PitchBend { channel, value } = msg {
                let velocity = ((value as f32 / 16383.0) * 127.0) as u8;
                let note = pb_to_note.note.unwrap_or(60).min(127);

                trace!(
                    "Transform PB→Note: ch{} value={} → note={} vel={}",
                    channel,
                    value,
                    note,
                    velocity
                );

                return Some(
                    MidiMessage::NoteOn {
                        channel,
                        note,
                        velocity,
                    }
                    .to_bytes(),
                );
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
                },
            };

            // Send the message, capturing the outcome and releasing the port
            // lock before any reconnect bookkeeping (never hold the lock
            // across a spawn). `bool` flags whether we were connected, so the
            // not-connected case stays quiet (trace) while a genuine send
            // failure warns and triggers recovery.
            let send_outcome: Result<(), (String, bool)> = {
                let mut midi_out = self.midi_out.lock();
                match &mut *midi_out {
                    Some(conn) => {
                        debug!("Bridge TX -> {}: {:02X?}", self.to_port, transformed);
                        match conn.send(&transformed) {
                            Ok(_) => Ok(()),
                            Err(e) => {
                                *midi_out = None; // Close broken connection
                                Err((format!("MIDI send failed: {}", e), true))
                            },
                        }
                    },
                    None => Err((
                        format!("MIDI Bridge '{}' not connected", self.to_port),
                        false,
                    )),
                }
            };

            match send_outcome {
                Ok(()) => Ok(()),
                Err((msg, was_connected)) => {
                    if was_connected {
                        warn!("MIDI Bridge OUT '{}': {} — reconnecting", self.to_port, msg);
                        self.update_status();
                    } else {
                        trace!("Bridge TX skipped (not connected): {}", self.to_port);
                    }
                    // Ensure a reconnect loop is running (single-flight guarded,
                    // so repeated failures don't pile up tasks).
                    self.spawn_out_reconnect();
                    Err(anyhow!(msg))
                },
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

    async fn init(&self, ctx: ExecutionContext) -> Result<()> {
        debug!(
            "Initializing MIDI Bridge: '{}' ⇄ '{}'",
            self.to_port, self.from_port
        );

        // Store activity tracker if available
        if let Some(tracker) = ctx.activity_tracker {
            *self.activity_tracker.write() = Some(tracker);
        }

        // Try to open output port
        match self.try_open_out() {
            Ok(_) => {},
            Err(e) if self.optional => {
                warn!("MIDI Bridge OUT open failed (optional): {}", e);
                self.spawn_out_reconnect();
            },
            Err(e) => return Err(e),
        }

        // Try to open input port
        match self.try_open_in() {
            Ok(_) => {},
            Err(e) if self.optional => {
                warn!("MIDI Bridge IN open failed (optional): {}", e);
                self.spawn_in_reconnect();
            },
            Err(e) => return Err(e),
        }

        // Start the port-health monitor so a later *silent* disconnect (the
        // input callback going dead, or the output port vanishing without a
        // send to notice) is detected and recovered instead of going
        // permanently dark.
        self.spawn_health_monitor();

        debug!(
            "MIDI Bridge active: '{}' ⇄ '{}'",
            self.to_port, self.from_port
        );
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        match action {
            "passthrough" => {
                // Extract raw MIDI bytes from context value
                if let Some(Value::Array(bytes_array)) = ctx.value {
                    let bytes: Vec<u8> = bytes_array
                        .iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect();
                    if !bytes.is_empty() {
                        debug!("→ Passthrough {} bytes to '{}'", bytes.len(), self.to_port);
                        self.send_message(&bytes)?;

                        // Record outbound activity
                        if let Some(ref tracker) = ctx.activity_tracker {
                            tracker.record(&self.name, crate::tray::ActivityDirection::Outbound);
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

                    // Record outbound activity
                    if let Some(ref tracker) = ctx.activity_tracker {
                        tracker.record(&self.name, crate::tray::ActivityDirection::Outbound);
                    }
                }
                Ok(())
            },
            _ => {
                warn!("Unknown MIDI bridge action: {}", action);
                Ok(())
            },
        }
    }

    async fn sync(&self) -> Result<()> {
        debug!("MIDI Bridge sync (no-op)");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        debug!("Shutting down MIDI Bridge: '{}'", self.to_port);

        *self.shutdown_flag.lock() = true;

        // Close MIDI connections
        *self.midi_out.lock() = None;
        *self.midi_in.lock() = None;

        // Update status to disconnected
        self.update_status();

        debug!("MIDI Bridge shutdown complete");
        Ok(())
    }

    fn connection_status(&self) -> crate::tray::ConnectionStatus {
        self.current_status.read().clone()
    }

    fn subscribe_connection_status(&self, callback: crate::tray::StatusCallback) {
        debug!("MIDI Bridge: new connection status subscription");

        // Emit current status immediately to new subscriber
        let current = self.current_status.read().clone();
        callback(current);

        // Add to callbacks list
        self.status_callbacks.write().push(callback);
    }
}
