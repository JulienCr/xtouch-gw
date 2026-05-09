//! Windows audio driver: master + per-app session volume/mute control.
//!
//! Backed by Win32 Core Audio (`IMMDeviceEnumerator`, `IAudioEndpointVolume`,
//! `IAudioSessionManager2`, `ISimpleAudioVolume`) and registered COM event
//! callbacks for bidirectional sync. All COM work runs on a dedicated STA
//! thread owned by the driver; the public surface is async and crosses
//! the thread boundary via `mpsc` / `broadcast` channels.
//!
//! On non-Windows targets this driver is a no-op stub that logs every
//! action — useful for testing the YAML routing path without hardware.

mod actions;
#[cfg(target_os = "windows")]
mod callback;
#[cfg(target_os = "windows")]
mod com_thread;
mod mapping;
#[cfg(target_os = "windows")]
mod master;
#[cfg(target_os = "windows")]
mod session;
#[cfg(target_os = "windows")]
mod session_events;

use crate::config::WinAudioConfig;
use crate::drivers::{Driver, ExecutionContext};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

pub use actions::{parse_session_target, SessionTarget};

/// Driver name used for `app: "winaudio"` in YAML control mappings.
pub const DRIVER_NAME: &str = "winaudio";

pub struct WinAudioDriver {
    config: Arc<RwLock<WinAudioConfig>>,
    initialized: AtomicBool,
    /// Wired post-construction via [`WinAudioDriver::set_router`].
    router: Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    /// Wired post-construction. The COM event consumer task uses this to
    /// inject synthetic `("winaudio", raw_midi)` feedback into the unified
    /// router feedback path.
    feedback_tx: Arc<RwLock<Option<mpsc::Sender<(String, Vec<u8>)>>>>,
    /// Stable FIFO of non-pinned process names; never reorders existing entries.
    discovery: Arc<RwLock<mapping::DiscoveryState>>,
    #[cfg(target_os = "windows")]
    com: Arc<RwLock<Option<com_thread::ComThreadHandle>>>,
}

impl WinAudioDriver {
    pub fn new(config: WinAudioConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            initialized: AtomicBool::new(false),
            router: Arc::new(RwLock::new(None)),
            feedback_tx: Arc::new(RwLock::new(None)),
            discovery: Arc::new(RwLock::new(mapping::DiscoveryState::default())),
            #[cfg(target_os = "windows")]
            com: Arc::new(RwLock::new(None)),
        }
    }

    /// Wire the driver to the router so it can push LCD updates.
    /// Called by driver_setup after construction.
    pub async fn set_router(&self, router: Arc<crate::router::Router>) {
        *self.router.write().await = Some(router);
    }

    /// Wire the driver to the unified feedback channel.
    pub async fn set_feedback_sender(&self, tx: mpsc::Sender<(String, Vec<u8>)>) {
        *self.feedback_tx.write().await = Some(tx);
    }
}

#[async_trait]
impl Driver for WinAudioDriver {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    async fn init(&self, _ctx: ExecutionContext) -> Result<()> {
        info!("WinAudio driver initializing");
        #[cfg(target_os = "windows")]
        {
            match com_thread::ComThreadHandle::spawn() {
                Ok(handle) => {
                    info!("WinAudio COM thread started");
                    if let Some(event_rx) = handle.take_event_rx().await {
                        let feedback = self.feedback_tx.read().await.clone();
                        if let Some(feedback) = feedback {
                            tokio::spawn(run_event_consumer(
                                event_rx,
                                feedback,
                                self.config.clone(),
                                self.discovery.clone(),
                                self.router.clone(),
                            ));
                        } else {
                            warn!(
                                "WinAudio: no feedback sender wired; volume changes won't reach the X-Touch"
                            );
                        }
                    }

                    // Initial discovery snapshot so `discovered:N` slots
                    // are populated before the user moves a fader.
                    self.refresh_discovery_with(&handle).await;

                    *self.com.write().await = Some(handle);

                    // Subscribe to page changes so we re-emit master state
                    // every time "Windows Audio" becomes active.
                    self.spawn_page_watcher().await;

                    // Initial state push, after a short delay so the
                    // VM auto-switch (poll every 5s, but first tick is
                    // immediate) has a chance to land us on the right
                    // page. The page filter inside the router would
                    // otherwise drop a feedback emit while we're still
                    // on "Voicemeeter+QLC".
                    let com = self.com.clone();
                    let cfg = self.config.clone();
                    let disc = self.discovery.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                        refresh_full_state(&com, &cfg, &disc).await;
                    });
                },
                Err(e) => {
                    warn!(
                        "WinAudio COM thread failed to start: {} — driver will be inert",
                        e
                    );
                },
            }
        }
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        if !self.initialized.load(Ordering::Acquire) {
            warn!(
                "WinAudio driver not initialized, dropping action '{}'",
                action
            );
            return Ok(());
        }

        // X-Touch faders deliver 14-bit PitchBend values (0..=16383); the
        // router forwards the raw integer in `ctx.value`. Normalize here
        // for *_volume actions so 100% Windows volume corresponds to a
        // fully-up fader, not just any non-zero value.
        let fader_scalar = ctx
            .value
            .as_ref()
            .and_then(|v| v.as_f64())
            .map(normalize_fader_value);

        match action {
            "master_volume" => self.handle_master_volume(fader_scalar).await,
            "master_mute" => self.handle_master_mute(ctx.is_button_release()).await,
            "session_volume" => {
                let target = parse_session_target(&params)?;
                self.handle_session_volume(target, fader_scalar).await
            },
            "session_mute" => {
                let target = parse_session_target(&params)?;
                self.handle_session_mute(target, ctx.is_button_release())
                    .await
            },
            _ => {
                warn!("Unknown winaudio action '{}'", action);
                Ok(())
            },
        }
    }

    async fn sync(&self) -> Result<()> {
        // Future: rebuild pinned/discovered mapping from new config.
        debug!("WinAudio sync requested (no-op for now)");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.initialized.store(false, Ordering::Release);
        #[cfg(target_os = "windows")]
        {
            if let Some(handle) = self.com.write().await.take() {
                handle.shutdown().await;
            }
        }
        info!("WinAudio driver shut down");
        Ok(())
    }
}

impl WinAudioDriver {
    async fn handle_master_volume(&self, normalized: Option<f32>) -> Result<()> {
        let Some(scalar) = normalized else {
            debug!("master_volume: no value, ignored");
            return Ok(());
        };
        debug!("master_volume <- {:.3}", scalar);
        #[cfg(target_os = "windows")]
        {
            if let Some(com) = self.com.read().await.as_ref() {
                com.set_master_scalar(scalar);
            }
        }
        Ok(())
    }

    async fn handle_master_mute(&self, is_release: bool) -> Result<()> {
        if is_release {
            return Ok(());
        }
        debug!("master_mute toggle (press)");
        #[cfg(target_os = "windows")]
        {
            if let Some(com) = self.com.read().await.as_ref() {
                com.toggle_master_mute();
            }
        }
        Ok(())
    }

    async fn handle_session_volume(
        &self,
        target: SessionTarget,
        normalized: Option<f32>,
    ) -> Result<()> {
        let Some(scalar) = normalized else {
            return Ok(());
        };
        let Some(process_name_lc) = self.resolve_target(target).await else {
            debug!("session_volume {:?}: no process bound", target);
            return Ok(());
        };
        debug!("session_volume {} <- {:.3}", process_name_lc, scalar);
        #[cfg(target_os = "windows")]
        {
            if let Some(com) = self.com.read().await.as_ref() {
                com.set_session_scalar(process_name_lc, scalar);
            }
        }
        Ok(())
    }

    async fn handle_session_mute(&self, target: SessionTarget, is_release: bool) -> Result<()> {
        if is_release {
            return Ok(());
        }
        let Some(process_name_lc) = self.resolve_target(target).await else {
            return Ok(());
        };
        debug!("session_mute {} toggle", process_name_lc);
        #[cfg(target_os = "windows")]
        {
            if let Some(com) = self.com.read().await.as_ref() {
                com.toggle_session_mute(process_name_lc);
            }
        }
        Ok(())
    }

    async fn resolve_target(&self, target: SessionTarget) -> Option<String> {
        let cfg = self.config.read().await;
        match target {
            SessionTarget::Pinned(fader) => mapping::pinned_target(&cfg.pinned_apps, fader),
            SessionTarget::Discovered(slot) => {
                let discovery = self.discovery.read().await;
                mapping::discovered_target(&cfg.pinned_apps, &discovery, slot)
            },
        }
    }

    /// Spawn a background task that re-emits master + per-session state
    /// whenever the "Windows Audio" page becomes active. Necessary
    /// because the `IAudioEndpointVolumeCallback` only fires on actual
    /// volume *changes*, never on page activation — so without this
    /// the X-Touch fader stays at its old position the first time
    /// the user switches to the Windows Audio page.
    async fn spawn_page_watcher(&self) {
        let router_arc = self.router.read().await.clone();
        let Some(router) = router_arc else {
            debug!("WinAudio: no router wired, skipping page watcher");
            return;
        };
        let Some(live_tx) = router.live_tx_snapshot().await else {
            debug!("WinAudio: live_tx not yet wired, skipping page watcher");
            return;
        };
        let mut rx = live_tx.subscribe();
        #[cfg(target_os = "windows")]
        let com = self.com.clone();
        #[cfg(target_os = "windows")]
        let config = self.config.clone();
        #[cfg(target_os = "windows")]
        let discovery = self.discovery.clone();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if let crate::event_bus::LiveEvent::PageChanged { name, .. } = event {
                    if name == "Windows Audio" {
                        debug!("WinAudio: page activated, refreshing master + sessions");
                        #[cfg(target_os = "windows")]
                        {
                            refresh_full_state(&com, &config, &discovery).await;
                        }
                    }
                }
            }
        });
    }

    #[cfg(target_os = "windows")]
    async fn refresh_discovery_with(&self, handle: &com_thread::ComThreadHandle) {
        let names = handle.enumerate_sessions().await;
        let cfg = self.config.read().await;
        let pinned_lc: std::collections::HashSet<String> = cfg
            .pinned_apps
            .iter()
            .map(|p| p.process_name.to_lowercase())
            .collect();
        drop(cfg);
        let mut state = self.discovery.write().await;
        state.observe(&names, &pinned_lc);
        debug!(
            "WinAudio discovery: {} active session(s), order={:?}",
            names.len(),
            state.discovered_order
        );
    }
}

/// Re-enumerate discovery so newly opened apps land in the FIFO order,
/// then trigger master + per-session feedback emission. Used both at
/// startup and on every "Windows Audio" page activation.
#[cfg(target_os = "windows")]
async fn refresh_full_state(
    com: &Arc<RwLock<Option<com_thread::ComThreadHandle>>>,
    config: &Arc<RwLock<WinAudioConfig>>,
    discovery: &Arc<RwLock<mapping::DiscoveryState>>,
) {
    let com_guard = com.read().await;
    let Some(handle) = com_guard.as_ref() else {
        return;
    };

    let names = handle.enumerate_sessions().await;
    let cfg = config.read().await;
    let pinned_lc: std::collections::HashSet<String> = cfg
        .pinned_apps
        .iter()
        .map(|p| p.process_name.to_lowercase())
        .collect();
    drop(cfg);
    {
        let mut state = discovery.write().await;
        state.observe(&names, &pinned_lc);
        debug!(
            "WinAudio refresh: {} active session(s), discovery order={:?}",
            names.len(),
            state.discovered_order
        );
    }

    handle.refresh_master();
    handle.refresh_sessions();
}

/// Convert a raw 14-bit PitchBend value from the router into a `[0.0, 1.0]`
/// scalar. The router forwards `ctx.value` verbatim as the integer 14-bit
/// reading.
fn normalize_fader_value(v: f64) -> f32 {
    ((v / 16383.0) as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_fader_value;

    #[test]
    fn zero_stays_zero() {
        assert_eq!(normalize_fader_value(0.0), 0.0);
    }

    #[test]
    fn full_14bit_becomes_one() {
        assert!((normalize_fader_value(16383.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn midpoint_14bit_is_half() {
        let mid = normalize_fader_value(8191.5);
        assert!((mid - 0.5).abs() < 1e-3);
    }

    #[test]
    fn out_of_range_clamped() {
        assert_eq!(normalize_fader_value(99999.0), 1.0);
        assert_eq!(normalize_fader_value(-5.0), 0.0);
    }
}

/// Logical winaudio action key used to dedup events in the coalesce
/// buffer. Each tuple maps to at most one fader/LED on the active page;
/// any matching action+target rebinding in YAML is honored without code
/// changes.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum FeedbackKey {
    Master,
    /// Session slot target string, e.g. `"pinned:1"` or `"discovered:0"`.
    Session(String),
}

#[cfg(target_os = "windows")]
struct PendingFeedback {
    scalar: f32,
    mute: bool,
}

/// Pump `AudioEvent`s emitted by the COM thread into the router's
/// unified feedback channel as synthetic `"winaudio"` MIDI messages.
///
/// **Mapping resolution:** the consumer reads the active page YAML and
/// the canonical `control_mapping.csv` to resolve which X-Touch control
/// (and therefore which MIDI channel/note) carries each `winaudio`
/// action. There is no hardcoded fader/note assumption in the driver —
/// pages can rebind master/session controls freely and the feedback
/// follows.
///
/// **Coalescing:** the OS audio engine fires `OnSimpleVolumeChanged`
/// once per intermediate value while the user drags the Windows mixer
/// slider. Sending each one as a discrete fader command makes the
/// motorized fader chase every step instead of jumping to the final
/// position. We buffer the latest `(scalar, mute)` per logical action
/// and flush at `FLUSH_INTERVAL_MS`, which lets the motor settle on the
/// final value while keeping perceived latency well under the 50 ms
/// feel threshold.
#[cfg(target_os = "windows")]
async fn run_event_consumer(
    mut event_rx: mpsc::Receiver<com_thread::AudioEvent>,
    feedback_tx: mpsc::Sender<(String, Vec<u8>)>,
    config: Arc<RwLock<WinAudioConfig>>,
    discovery: Arc<RwLock<mapping::DiscoveryState>>,
    router: Arc<RwLock<Option<Arc<crate::router::Router>>>>,
) {
    use std::collections::HashMap;
    use tokio::time::{interval, Duration, MissedTickBehavior};

    /// Max emit rate per channel (~20 Hz). The X-Touch motorized fader
    /// physically can't follow updates faster than this without lagging.
    const FLUSH_INTERVAL_MS: u64 = 50;

    debug!("WinAudio event consumer task started");

    let mut pending: HashMap<FeedbackKey, PendingFeedback> = HashMap::new();

    let mut ticker = interval(Duration::from_millis(FLUSH_INTERVAL_MS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            event = event_rx.recv() => {
                let Some(event) = event else {
                    debug!("WinAudio event consumer: source closed");
                    break;
                };
                buffer_event(&mut pending, event, &config, &discovery).await;
            }
            _ = ticker.tick() => {
                if pending.is_empty() {
                    continue;
                }
                let drained: Vec<(FeedbackKey, PendingFeedback)> = pending.drain().collect();
                if !flush_pending(&drained, &feedback_tx, &router).await {
                    debug!("WinAudio feedback channel closed, exiting consumer");
                    return;
                }
            }
        }
    }
}

/// Stash an event into the coalesce buffer. Sessions are keyed by their
/// resolved YAML target (`pinned:N` / `discovered:N`); unresolvable
/// sessions are dropped silently.
#[cfg(target_os = "windows")]
async fn buffer_event(
    pending: &mut std::collections::HashMap<FeedbackKey, PendingFeedback>,
    event: com_thread::AudioEvent,
    config: &Arc<RwLock<WinAudioConfig>>,
    discovery: &Arc<RwLock<mapping::DiscoveryState>>,
) {
    match event {
        com_thread::AudioEvent::MasterVolumeChanged { scalar, mute } => {
            pending.insert(FeedbackKey::Master, PendingFeedback { scalar, mute });
        },
        com_thread::AudioEvent::SessionVolumeSnapshot {
            process_name_lc,
            scalar,
            mute,
        } => {
            let cfg = config.read().await;
            let disc = discovery.read().await;
            let target = mapping::target_for_process(&cfg.pinned_apps, &disc, &process_name_lc);
            drop(disc);
            drop(cfg);
            let Some(target) = target else {
                debug!(
                    "session snapshot for '{}' has no fader slot, ignored",
                    process_name_lc
                );
                return;
            };
            pending.insert(
                FeedbackKey::Session(target),
                PendingFeedback { scalar, mute },
            );
        },
    }
}

/// Resolve every pending event to MIDI bytes (via active page +
/// `control_mapping.csv`) and emit them. Returns `false` if the
/// feedback channel is closed.
#[cfg(target_os = "windows")]
async fn flush_pending(
    drained: &[(FeedbackKey, PendingFeedback)],
    feedback_tx: &mpsc::Sender<(String, Vec<u8>)>,
    router: &Arc<RwLock<Option<Arc<crate::router::Router>>>>,
) -> bool {
    let Some(router_arc) = router.read().await.clone() else {
        debug!(
            "WinAudio flush: no router wired, dropping {} event(s)",
            drained.len()
        );
        return true;
    };
    let Some(page) = router_arc.get_active_page().await else {
        return true;
    };
    let mcu_mode = is_mcu_mode(&router_arc).await;

    let Ok(db) = crate::control_mapping::load_default_mappings() else {
        warn!("WinAudio flush: control_mapping DB unavailable");
        return true;
    };

    for (key, p) in drained {
        let (volume_action, mute_action, target) = match key {
            FeedbackKey::Master => ("master_volume", "master_mute", None),
            FeedbackKey::Session(t) => ("session_volume", "session_mute", Some(t.as_str())),
        };

        // Volume → PitchBend or CC.
        if let Some(spec) = resolve_action_spec(&page, volume_action, target, db, mcu_mode) {
            let bytes = bytes_for_volume(&spec, p.scalar);
            if !try_send(feedback_tx, bytes).await {
                return false;
            }
        }

        // Mute → Note (LED).
        if let Some(spec) = resolve_action_spec(&page, mute_action, target, db, mcu_mode) {
            let bytes = bytes_for_mute(&spec, p.mute);
            if !try_send(feedback_tx, bytes).await {
                return false;
            }
        }
    }
    true
}

#[cfg(target_os = "windows")]
async fn is_mcu_mode(router: &Arc<crate::router::Router>) -> bool {
    let cfg = router.config.read().await;
    cfg.xtouch
        .as_ref()
        .map(|x| matches!(x.mode, crate::config::XTouchMode::Mcu))
        .unwrap_or(true)
}

/// Find the page control bound to `(action, target)` and resolve it to
/// a hardware MIDI spec via `control_mapping.csv`. Page controls are
/// checked first, then global controls.
#[cfg(target_os = "windows")]
fn resolve_action_spec(
    page: &crate::config::PageConfig,
    action: &str,
    target: Option<&str>,
    db: &crate::control_mapping::ControlMappingDB,
    mcu_mode: bool,
) -> Option<crate::control_mapping::MidiSpec> {
    let control_id = find_winaudio_control_id(page, action, target)?;
    db.get_midi_spec(&control_id, mcu_mode)
}

#[cfg(target_os = "windows")]
fn find_winaudio_control_id(
    page: &crate::config::PageConfig,
    action: &str,
    target: Option<&str>,
) -> Option<String> {
    let controls = page.controls.as_ref()?;
    controls
        .iter()
        .find(|(_, m)| {
            m.app == DRIVER_NAME
                && m.action.as_deref() == Some(action)
                && match target {
                    None => true,
                    Some(want) => {
                        m.params
                            .as_ref()
                            .and_then(|p| p.first())
                            .and_then(|v| v.as_str())
                            == Some(want)
                    },
                }
        })
        .map(|(id, _)| id.clone())
}

#[cfg(target_os = "windows")]
fn bytes_for_volume(spec: &crate::control_mapping::MidiSpec, scalar: f32) -> Vec<u8> {
    use crate::control_mapping::MidiSpec;
    use crate::midi::{convert, MidiMessage};
    let scalar = scalar.clamp(0.0, 1.0) as f64;
    match *spec {
        MidiSpec::PitchBend { channel } => MidiMessage::PitchBend {
            channel: channel & 0x0F,
            value: convert::denormalize_to_14bit(scalar),
        }
        .to_bytes(),
        MidiSpec::ControlChange { cc } => MidiMessage::ControlChange {
            channel: 0,
            cc,
            value: convert::denormalize_to_7bit(scalar),
        }
        .to_bytes(),
        MidiSpec::Note { note } => mute_note_bytes(note, scalar > 0.0),
    }
}

#[cfg(target_os = "windows")]
fn bytes_for_mute(spec: &crate::control_mapping::MidiSpec, muted: bool) -> Vec<u8> {
    use crate::control_mapping::MidiSpec;
    match *spec {
        MidiSpec::Note { note } => mute_note_bytes(note, muted),
        // Some surfaces wire mute to a CC indicator instead of a note.
        MidiSpec::ControlChange { cc } => crate::midi::MidiMessage::ControlChange {
            channel: 0,
            cc,
            value: if muted { 127 } else { 0 },
        }
        .to_bytes(),
        // PitchBend doesn't make sense for mute LEDs, but emit something
        // sane rather than panicking.
        MidiSpec::PitchBend { channel } => crate::midi::MidiMessage::PitchBend {
            channel: channel & 0x0F,
            value: if muted { 16383 } else { 0 },
        }
        .to_bytes(),
    }
}

#[cfg(target_os = "windows")]
async fn try_send(feedback_tx: &mpsc::Sender<(String, Vec<u8>)>, raw: Vec<u8>) -> bool {
    feedback_tx
        .send((DRIVER_NAME.to_string(), raw))
        .await
        .is_ok()
}

/// X-Touch button-LED convention: always `NoteOn`, with velocity 127 to
/// light and velocity 0 to extinguish. The hardware treats `NoteOff` as
/// a button-release event, not an LED-off — see `xtouch.rs::set_button_led`
/// for the canonical comment.
fn mute_note_bytes(note: u8, muted: bool) -> Vec<u8> {
    crate::midi::MidiMessage::NoteOn {
        channel: 0,
        note,
        velocity: if muted { 127 } else { 0 },
    }
    .to_bytes()
}
