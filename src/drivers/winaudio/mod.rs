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

use crate::config::WinAudioConfig;
use crate::drivers::{Driver, ExecutionContext};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

pub use actions::{parse_session_target, SessionTarget};

/// Driver name used for `app: "winaudio"` in YAML control mappings.
pub const DRIVER_NAME: &str = "winaudio";

pub struct WinAudioDriver {
    /// Pinned-app + (later) discovered-session configuration cloned from AppConfig.
    config: Arc<RwLock<WinAudioConfig>>,
    /// Set by `init`, cleared by `shutdown`.
    initialized: Arc<RwLock<bool>>,
    /// Optional Router handle for emitting LCD updates and reading state.
    /// Wired post-construction via [`WinAudioDriver::set_router`].
    router: Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    /// Optional feedback channel; wired post-construction. The COM event
    /// consumer task uses this to inject synthetic feedback messages
    /// (`("winaudio", raw_midi)`) into the unified router feedback path.
    feedback_tx: Arc<RwLock<Option<mpsc::Sender<(String, Vec<u8>)>>>>,
    /// Stable discovery order of non-pinned process names. Updated whenever
    /// we re-enumerate; never reorders existing entries.
    discovery: Arc<RwLock<mapping::DiscoveryState>>,
    #[cfg(target_os = "windows")]
    com: Arc<RwLock<Option<com_thread::ComThreadHandle>>>,
}

impl WinAudioDriver {
    pub fn new(config: WinAudioConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            initialized: Arc::new(RwLock::new(false)),
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
        *self.initialized.write().await = true;
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        if !*self.initialized.read().await {
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
        *self.initialized.write().await = false;
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
        let _ = scalar; // silence unused warning on non-windows
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

/// MIDI channel (0-based) the master fader lives on, in MCU mode.
/// Channel 9 (1-based) is the master strip, encoded as 0x08 internally.
const MASTER_FADER_CHANNEL_0BASED: u8 = 8;

/// Convert a raw fader value from the router into a `[0.0, 1.0]` scalar.
///
/// X-Touch fader controls produce 14-bit PitchBend (0..=16383), forwarded
/// verbatim through `ctx.value`. A value <= 1.0 is assumed to already be
/// normalized (e.g. from a future CC mapping wired through a transform).
/// Anything larger is rescaled by dividing by 16383.
fn normalize_fader_value(v: f64) -> f32 {
    let scaled = if v > 1.0 { v / 16383.0 } else { v };
    scaled.clamp(0.0, 1.0) as f32
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
    fn already_normalized_passes_through() {
        assert!((normalize_fader_value(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn out_of_range_clamped() {
        assert_eq!(normalize_fader_value(99999.0), 1.0);
        assert_eq!(normalize_fader_value(-5.0), 0.0);
    }
}

/// Pump `AudioEvent`s emitted by the COM thread into the router's
/// unified feedback channel as synthetic `"winaudio"` MIDI messages.
///
/// This lets the existing feedback pipeline (anti-echo, fader_setpoint,
/// state actor, page-aware filtering) handle Windows audio updates the
/// same way it handles OBS / Voicemeeter feedback — no special-casing
/// in the router.
///
/// `config` and `discovery` are read on every session event to resolve
/// process names to fader slots, so the consumer always reflects the
/// current pinned/discovered mapping.
#[cfg(target_os = "windows")]
async fn run_event_consumer(
    mut event_rx: mpsc::UnboundedReceiver<com_thread::AudioEvent>,
    feedback_tx: mpsc::Sender<(String, Vec<u8>)>,
    config: Arc<RwLock<WinAudioConfig>>,
    discovery: Arc<RwLock<mapping::DiscoveryState>>,
) {
    debug!("WinAudio event consumer task started");
    while let Some(event) = event_rx.recv().await {
        match event {
            com_thread::AudioEvent::MasterVolumeChanged { scalar, mute: _ } => {
                let raw = build_pitchbend_raw(MASTER_FADER_CHANNEL_0BASED, scalar);
                if feedback_tx
                    .send((DRIVER_NAME.to_string(), raw))
                    .await
                    .is_err()
                {
                    debug!("WinAudio feedback channel closed, exiting consumer");
                    break;
                }
            },
            com_thread::AudioEvent::SessionVolumeSnapshot {
                process_name_lc,
                scalar,
                mute: _,
            } => {
                // Resolve the fader slot this session currently occupies.
                let cfg = config.read().await;
                let disc = discovery.read().await;
                let slots = mapping::compute_slots(&cfg.pinned_apps, &disc);
                let Some(binding) = slots
                    .iter()
                    .find(|b| b.process_name.as_deref() == Some(process_name_lc.as_str()))
                else {
                    drop(disc);
                    drop(cfg);
                    debug!(
                        "session snapshot for '{}' has no fader slot, ignored",
                        process_name_lc
                    );
                    continue;
                };
                let channel0 = binding.fader.saturating_sub(1);
                drop(disc);
                drop(cfg);

                let raw = build_pitchbend_raw(channel0, scalar);
                if feedback_tx
                    .send((DRIVER_NAME.to_string(), raw))
                    .await
                    .is_err()
                {
                    break;
                }
            },
        }
    }
}

/// Build a raw 3-byte PitchBend message for `channel0` (0-based MIDI channel).
fn build_pitchbend_raw(channel0: u8, scalar: f32) -> Vec<u8> {
    use crate::midi::convert::denormalize_to_14bit;
    let value14 = denormalize_to_14bit(scalar.clamp(0.0, 1.0) as f64);
    let lsb = (value14 & 0x7F) as u8;
    let msb = ((value14 >> 7) & 0x7F) as u8;
    vec![0xE0 | (channel0 & 0x0F), lsb, msb]
}
