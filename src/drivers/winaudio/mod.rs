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
mod catalog;
#[cfg(target_os = "windows")]
mod com_thread;
mod mapping;
#[cfg(target_os = "windows")]
mod master;
#[cfg(target_os = "windows")]
mod session;
#[cfg(target_os = "windows")]
mod session_events;

use crate::config::{ControlMapping, PageConfig, WinAudioConfig};
use crate::drivers::{Driver, ExecutionContext};
use crate::xtouch::{build_lcd_colors_sysex, build_lcd_strip_sysex};
use anyhow::Result;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

pub use actions::{parse_session_target, SessionTarget};

/// Driver name used for `app: "winaudio"` in YAML control mappings.
pub const DRIVER_NAME: &str = "winaudio";

/// True if at least one control on `page` binds `app: "winaudio"`. Used to
/// decide whether a page is "winaudio-eligible" for state refresh and
/// dynamic LCD rendering. Auto-detected so the YAML page name is no
/// longer load-bearing — renaming `"Windows Audio"` to anything else
/// keeps the driver wired (#39).
fn page_uses_winaudio(page: &PageConfig) -> bool {
    page.controls
        .as_ref()
        .is_some_and(|c| c.values().any(|m| m.app == DRIVER_NAME))
}

/// True if the page at `index` in the router's current config uses the
/// winaudio driver. Lock-friendly: takes a single short read on
/// `Router::config`.
async fn page_eligible_at_index(
    router_arc: &Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    index: usize,
) -> bool {
    let Some(router) = router_arc.read().await.clone() else {
        return false;
    };
    let cfg = router.config.read().await;
    cfg.pages.get(index).is_some_and(page_uses_winaudio)
}

pub struct WinAudioDriver {
    config: Arc<RwLock<WinAudioConfig>>,
    initialized: AtomicBool,
    /// Wired post-construction via [`WinAudioDriver::set_router`].
    router: Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    /// Wired post-construction via [`WinAudioDriver::set_led_sender`].
    /// Raw MIDI bytes pushed here are forwarded to the X-Touch by the main loop.
    led_tx: Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
    /// Wired post-construction. The COM event consumer task uses this to
    /// inject synthetic `("winaudio", raw_midi)` feedback into the unified
    /// router feedback path.
    feedback_tx: Arc<RwLock<Option<mpsc::Sender<(String, Vec<u8>)>>>>,
    /// Stable FIFO of non-pinned process names; never reorders existing entries.
    discovery: Arc<RwLock<mapping::DiscoveryState>>,
    /// Lowercased set of pinned process names. Cached so hot-path
    /// resolves (`mapping::discovered_target`, `target_for_process`,
    /// `compute_slots`) read it lock-free instead of rebuilding from
    /// `config.pinned_apps` per MIDI event. Refreshed by `refresh_pinned_lc`
    /// on init and on every config-touching path (`sync()`, etc.).
    pinned_lc_cache: Arc<ArcSwap<HashSet<String>>>,
    #[cfg(target_os = "windows")]
    com: Arc<RwLock<Option<com_thread::ComThreadHandle>>>,
}

impl WinAudioDriver {
    pub fn new(config: WinAudioConfig) -> Self {
        let pinned_lc = mapping::pinned_lc_set(&config.pinned_apps);
        Self {
            config: Arc::new(RwLock::new(config)),
            initialized: AtomicBool::new(false),
            router: Arc::new(RwLock::new(None)),
            led_tx: Arc::new(RwLock::new(None)),
            feedback_tx: Arc::new(RwLock::new(None)),
            discovery: Arc::new(RwLock::new(mapping::DiscoveryState::default())),
            pinned_lc_cache: Arc::new(ArcSwap::from_pointee(pinned_lc)),
            #[cfg(target_os = "windows")]
            com: Arc::new(RwLock::new(None)),
        }
    }

    /// Rebuild the cached lowercased pinned-set from the current
    /// `config.pinned_apps`. Cheap (one HashSet construction) but only
    /// called from cold paths (init, `sync()`).
    async fn refresh_pinned_lc(&self) {
        let cfg = self.config.read().await;
        let new_set = mapping::pinned_lc_set(&cfg.pinned_apps);
        drop(cfg);
        self.pinned_lc_cache.store(Arc::new(new_set));
    }

    /// Wire the driver to the router so it can push LCD updates.
    /// Called by driver_setup after construction.
    pub async fn set_router(&self, router: Arc<crate::router::Router>) {
        *self.router.write().await = Some(router);
    }

    /// Wire the driver to the LED MIDI channel that the main loop drains
    /// into `xtouch.send_raw`. This is how the driver pushes dynamic
    /// LCD strip text + colors without holding a non-Sync reference to
    /// the X-Touch driver itself.
    pub async fn set_led_sender(&self, tx: mpsc::Sender<Vec<u8>>) {
        *self.led_tx.write().await = Some(tx);
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

        // Cache the lowercased pinned set so hot-path resolves don't
        // rebuild it per MIDI event. See #40.
        self.refresh_pinned_lc().await;

        // Assign cycle colors to all pinned process names so they share
        // the same 1..=7 cycle as discovered apps. Pinned apps with an
        // explicit YAML `color:` field still take precedence at render
        // time — this is just the fallback assignment.
        {
            let cfg = self.config.read().await;
            let pinned_lc: Vec<String> = cfg
                .pinned_apps
                .iter()
                .map(|p| p.process_name.to_lowercase())
                .collect();
            drop(cfg);
            let mut state = self.discovery.write().await;
            state.observe_pinned(&pinned_lc);
        }

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
                                self.pinned_lc_cache.clone(),
                                self.discovery.clone(),
                                self.router.clone(),
                                self.led_tx.clone(),
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
                    // every time a winaudio-eligible page becomes active
                    // (auto-detected — see `page_uses_winaudio`).
                    self.spawn_page_watcher().await;

                    // Initial state push, gated on the `ProfileLoaded`
                    // live event so the active page is fully settled
                    // before we emit (the router's page filter would
                    // otherwise drop the feedback). Replaces the legacy
                    // 800 ms sleep (#36). 2 s timeout is a safety net
                    // for the no-subscriber edge case.
                    let live_rx = match self.router.read().await.as_ref() {
                        Some(r) => r.live_tx_snapshot().await.map(|tx| tx.subscribe()),
                        None => None,
                    };
                    let com = self.com.clone();
                    let cfg = self.config.clone();
                    let pinned_lc_cache = self.pinned_lc_cache.clone();
                    let disc = self.discovery.clone();
                    let router = self.router.clone();
                    let led_tx = self.led_tx.clone();
                    tokio::spawn(async move {
                        wait_for_profile_loaded(live_rx).await;
                        refresh_full_state(&com, &pinned_lc_cache, &disc).await;
                        // If the active page already maps winaudio at
                        // startup (no PageChanged event will fire to wake
                        // the watcher), render the LCD now.
                        render_lcd_if_active(&router, &led_tx, &cfg, &pinned_lc_cache, &disc).await;
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
                self.handle_session_volume(target, fader_scalar, &ctx, action)
                    .await
            },
            "session_mute" => {
                let target = parse_session_target(&params)?;
                self.handle_session_mute(target, ctx.is_button_release(), &ctx, action)
                    .await
            },
            _ => {
                warn!("Unknown winaudio action '{}'", action);
                Ok(())
            },
        }
    }

    async fn sync(&self) -> Result<()> {
        // Refresh the cached pinned-set so a config reload that adds
        // or removes a pinned app is reflected on the next hot-path
        // resolve. See #40.
        self.refresh_pinned_lc().await;
        debug!("WinAudio sync: pinned_lc cache refreshed");
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

    fn action_catalog(&self) -> Vec<crate::api_editor::ActionDescriptor> {
        catalog::winaudio_catalog()
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
        ctx: &ExecutionContext,
        action: &str,
    ) -> Result<()> {
        let Some(scalar) = normalized else {
            return Ok(());
        };
        let Some(process_name_lc) = self.resolve_target(target, ctx, action).await else {
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

    async fn handle_session_mute(
        &self,
        target: SessionTarget,
        is_release: bool,
        ctx: &ExecutionContext,
        action: &str,
    ) -> Result<()> {
        if is_release {
            return Ok(());
        }
        let Some(process_name_lc) = self.resolve_target(target, ctx, action).await else {
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

    /// Resolve a `SessionTarget` to a concrete lowercase process name.
    /// `Auto` requires the active page + control_id from `ctx` so the
    /// driver can find this control's position among other auto-bound
    /// controls and index the discovery FIFO accordingly.
    async fn resolve_target(
        &self,
        target: SessionTarget,
        ctx: &ExecutionContext,
        action: &str,
    ) -> Option<String> {
        match target {
            SessionTarget::Pinned(fader) => {
                let cfg = self.config.read().await;
                mapping::pinned_target(&cfg.pinned_apps, fader)
            },
            SessionTarget::Discovered(slot) => {
                let pinned_lc = self.pinned_lc_cache.load();
                let discovery = self.discovery.read().await;
                mapping::discovered_target(&pinned_lc, &discovery, slot)
            },
            SessionTarget::Auto => {
                let control_id = ctx.control_id.as_deref()?;
                let router = self.router.read().await.clone()?;
                let page = router.get_active_page().await?;
                let auto_idx = auto_strip_index(&page, action, control_id)?;
                let pinned_lc = self.pinned_lc_cache.load();
                let discovery = self.discovery.read().await;
                mapping::discovered_target(&pinned_lc, &discovery, auto_idx)
            },
        }
    }

    /// Spawn a background task that re-emits master + per-session state
    /// whenever a winaudio-eligible page becomes active (auto-detected
    /// via [`page_uses_winaudio`] — no hardcoded page name).
    ///
    /// Necessary because `IAudioEndpointVolumeCallback` only fires on
    /// actual volume *changes*, never on page activation — so without
    /// this the X-Touch fader stays at its old position the first time
    /// the user switches to a winaudio page.
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
        let config = self.config.clone();
        let pinned_lc_cache = self.pinned_lc_cache.clone();
        let discovery = self.discovery.clone();
        let router_for_render = self.router.clone();
        let led_tx = self.led_tx.clone();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                let crate::event_bus::LiveEvent::PageChanged { index, .. } = event else {
                    continue;
                };
                if !page_eligible_at_index(&router_for_render, index).await {
                    continue;
                }
                debug!("WinAudio: page activated, refreshing master + sessions");
                #[cfg(target_os = "windows")]
                {
                    refresh_full_state(&com, &pinned_lc_cache, &discovery).await;
                }
                render_lcd_if_active(
                    &router_for_render,
                    &led_tx,
                    &config,
                    &pinned_lc_cache,
                    &discovery,
                )
                .await;
            }
        });
    }

    #[cfg(target_os = "windows")]
    async fn refresh_discovery_with(&self, handle: &com_thread::ComThreadHandle) {
        let names = handle.enumerate_sessions().await;
        let pinned_lc = self.pinned_lc_cache.load();
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
/// startup and on every winaudio-eligible page activation.
#[cfg(target_os = "windows")]
async fn refresh_full_state(
    com: &Arc<RwLock<Option<com_thread::ComThreadHandle>>>,
    pinned_lc_cache: &Arc<ArcSwap<HashSet<String>>>,
    discovery: &Arc<RwLock<mapping::DiscoveryState>>,
) {
    let com_guard = com.read().await;
    let Some(handle) = com_guard.as_ref() else {
        return;
    };

    let names = handle.enumerate_sessions().await;
    let pinned_lc = pinned_lc_cache.load();
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

/// Block until the startup profile is marked loaded (config parsed,
/// router wired, drivers registered) — or until the 2 s safety timeout
/// elapses. Replaces the legacy 800 ms post-init sleep (#36).
///
/// If `live_rx` is `None` (live bus not wired in tests, unusual prod
/// path), fall straight through.
async fn wait_for_profile_loaded(
    live_rx: Option<tokio::sync::broadcast::Receiver<crate::event_bus::LiveEvent>>,
) {
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let Some(mut rx) = live_rx else {
        debug!("WinAudio init: live bus unavailable, skipping ProfileLoaded await");
        return;
    };
    let fut = async {
        while let Ok(event) = rx.recv().await {
            if matches!(event, crate::event_bus::LiveEvent::ProfileLoaded { .. }) {
                return;
            }
        }
    };
    if tokio::time::timeout(TIMEOUT, fut).await.is_err() {
        debug!(
            "WinAudio init: ProfileLoaded not received within {:?}, proceeding with refresh anyway",
            TIMEOUT
        );
    }
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
#[allow(clippy::too_many_arguments)]
async fn run_event_consumer(
    mut event_rx: mpsc::Receiver<com_thread::AudioEvent>,
    feedback_tx: mpsc::Sender<(String, Vec<u8>)>,
    config: Arc<RwLock<WinAudioConfig>>,
    pinned_lc_cache: Arc<ArcSwap<HashSet<String>>>,
    discovery: Arc<RwLock<mapping::DiscoveryState>>,
    router: Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    led_tx: Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
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
                buffer_event(
                    &mut pending,
                    event,
                    &config,
                    &pinned_lc_cache,
                    &discovery,
                    &router,
                    &led_tx,
                ).await;
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
///
/// `ActiveSessionsChanged` is handled inline (not coalesced) because
/// it's a discrete state-transition event, not a continuous stream:
/// updates the discovery FIFO + active set and triggers a non-blocking
/// LCD render in a spawned task so the 50 ms fader flush isn't stalled.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
async fn buffer_event(
    pending: &mut std::collections::HashMap<FeedbackKey, PendingFeedback>,
    event: com_thread::AudioEvent,
    config: &Arc<RwLock<WinAudioConfig>>,
    pinned_lc_cache: &Arc<ArcSwap<HashSet<String>>>,
    discovery: &Arc<RwLock<mapping::DiscoveryState>>,
    router: &Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    led_tx: &Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
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
            let pinned_lc = pinned_lc_cache.load();
            let disc = discovery.read().await;
            let target =
                mapping::target_for_process(&cfg.pinned_apps, &pinned_lc, &disc, &process_name_lc);
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
        com_thread::AudioEvent::ActiveSessionsChanged { names_lc } => {
            // Update FIFO + active set atomically, then schedule the LCD
            // render in a separate task so 8 strips × 2 SysEx writes
            // (~30 ms USB MIDI) don't block the 50 ms fader flush tick.
            // Skip the spawn when the active set didn't actually change —
            // session-disconnect cascades fire redundant "changed" events.
            let active_changed = {
                let pinned_lc = pinned_lc_cache.load();
                let mut state = discovery.write().await;
                state.observe(&names_lc, &pinned_lc);
                state.set_active(&names_lc)
            };
            if !active_changed {
                return;
            }
            let router = router.clone();
            let led_tx = led_tx.clone();
            let config = config.clone();
            let pinned_lc_cache = pinned_lc_cache.clone();
            let discovery = discovery.clone();
            tokio::spawn(async move {
                render_lcd_if_active(&router, &led_tx, &config, &pinned_lc_cache, &discovery).await;
            });
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
    let mcu_mode = router_arc.config.read().await.is_mcu_mode();

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
            let bytes = spec.led_bytes(p.mute);
            if !try_send(feedback_tx, bytes).await {
                return false;
            }
        }
    }
    true
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
        MidiSpec::Note { note } => MidiSpec::Note { note }.led_bytes(scalar > 0.0),
    }
}

#[cfg(target_os = "windows")]
async fn try_send(feedback_tx: &mpsc::Sender<(String, Vec<u8>)>, raw: Vec<u8>) -> bool {
    feedback_tx
        .send((DRIVER_NAME.to_string(), raw))
        .await
        .is_ok()
}

// -- Auto target resolution + dynamic LCD render -----------------------------

/// Parse the strip number from a control id like "fader4" or "mute4".
/// Returns 1..=8 for valid strip controls; `None` for anything else
/// (e.g. "fader_master", "flip", "rewind").
fn strip_index_of(control_id: &str) -> Option<u8> {
    let suffix = control_id
        .strip_prefix("fader")
        .or_else(|| control_id.strip_prefix("mute"))?;
    let n: u8 = suffix.parse().ok()?;
    (1..=8).contains(&n).then_some(n)
}

/// Position of `control_id` among the page controls bound to
/// `winaudio.<action>` with `params: ["auto"]`, ordered by ascending
/// strip number. The HashMap-backed `controls` field has no inherent
/// declaration order, so we use the strip number as a deterministic
/// key. Returns `None` if `control_id` isn't an `auto`-bound winaudio
/// control on this page.
fn auto_strip_index(page: &PageConfig, action: &str, control_id: &str) -> Option<u8> {
    let controls = page.controls.as_ref()?;
    let mut auto_strips: Vec<(u8, &str)> = controls
        .iter()
        .filter(|(_, m)| is_auto_winaudio_action(m, action))
        .filter_map(|(id, _)| strip_index_of(id).map(|n| (n, id.as_str())))
        .collect();
    auto_strips.sort_by_key(|(n, _)| *n);
    auto_strips
        .iter()
        .position(|(_, id)| *id == control_id)
        .map(|p| p as u8)
}

fn is_auto_winaudio_action(m: &ControlMapping, action: &str) -> bool {
    if m.app != DRIVER_NAME {
        return false;
    }
    if m.action.as_deref() != Some(action) {
        return false;
    }
    let Some(params) = m.params.as_ref() else {
        return false;
    };
    params
        .first()
        .and_then(|v| v.as_str())
        .map(|s| s.trim().eq_ignore_ascii_case("auto"))
        .unwrap_or(false)
}

/// Render the dynamic LCD strips for the winaudio page if it is the
/// currently active page. Cheap no-op otherwise. Pushes raw SysEx
/// bytes through the `led_tx` channel — the main event loop drains
/// them and forwards to `xtouch.send_raw`.
async fn render_lcd_if_active(
    router: &Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    led_tx: &Arc<RwLock<Option<mpsc::Sender<Vec<u8>>>>>,
    config: &Arc<RwLock<WinAudioConfig>>,
    pinned_lc_cache: &Arc<ArcSwap<HashSet<String>>>,
    discovery: &Arc<RwLock<mapping::DiscoveryState>>,
) {
    let Some(router_arc) = router.read().await.clone() else {
        return;
    };
    let Some(page) = router_arc.get_active_page().await else {
        return;
    };
    if !page_uses_winaudio(&page) {
        return;
    }
    let Some(tx) = led_tx.read().await.clone() else {
        debug!("WinAudio render: no led_tx wired, skipping LCD render");
        return;
    };
    render_winaudio_lcd(&page, &tx, config, pinned_lc_cache, discovery).await;
}

/// Compute and push the 8 LCD strips for the winaudio page. Strips
/// whose process is not currently active render as black + empty.
/// Pinned apps with an explicit YAML color use it; otherwise the
/// cycle color from `assigned_color` is used.
///
/// Emits raw SysEx via `led_tx` rather than calling
/// `apply_lcd_for_page`: keeps the 7-segment display untouched and
/// avoids needing a non-Sync `Arc<XTouchDriver>` reference.
async fn render_winaudio_lcd(
    page: &PageConfig,
    led_tx: &mpsc::Sender<Vec<u8>>,
    config: &Arc<RwLock<WinAudioConfig>>,
    pinned_lc_cache: &Arc<ArcSwap<HashSet<String>>>,
    discovery: &Arc<RwLock<mapping::DiscoveryState>>,
) {
    let cfg = config.read().await;
    let pinned_lc = pinned_lc_cache.load();
    let disc = discovery.read().await;

    let mut colors: [u8; 8] = [0; 8];
    let mut labels: [String; 8] = Default::default();

    for strip_idx in 1u8..=8 {
        let control_id = format!("fader{strip_idx}");
        let process_lc =
            resolve_strip_process(&control_id, page, &cfg, &pinned_lc, &disc, "session_volume");

        let Some(process_lc) = process_lc else {
            continue;
        };
        if !disc.is_active(&process_lc) {
            continue;
        }

        labels[(strip_idx - 1) as usize] = label_for_process(&cfg.pinned_apps, &process_lc);
        colors[(strip_idx - 1) as usize] = color_for_process(&cfg.pinned_apps, &disc, &process_lc);
    }

    drop(disc);
    drop(cfg);

    for (i, label) in labels.iter().enumerate() {
        let (upper, lower) = build_lcd_strip_sysex(i as u8, label, "");
        if led_tx.send(upper).await.is_err() {
            debug!("WinAudio LCD: led_tx closed, dropping render");
            return;
        }
        if led_tx.send(lower).await.is_err() {
            debug!("WinAudio LCD: led_tx closed, dropping render");
            return;
        }
    }
    let color_msg = build_lcd_colors_sysex(&colors);
    if led_tx.send(color_msg).await.is_err() {
        debug!("WinAudio LCD: led_tx closed, dropping color update");
    }
}

/// Resolve `fader{N}` on the active page to a process name, walking
/// the same path as runtime action dispatch (`pinned`, `discovered`,
/// `auto`). Returns `None` if the strip isn't bound to a winaudio
/// session action or no process is currently mapped to it.
fn resolve_strip_process(
    control_id: &str,
    page: &PageConfig,
    cfg: &WinAudioConfig,
    pinned_lc: &HashSet<String>,
    disc: &mapping::DiscoveryState,
    action: &str,
) -> Option<String> {
    let controls = page.controls.as_ref()?;
    let m = controls.get(control_id)?;
    if m.app != DRIVER_NAME || m.action.as_deref() != Some(action) {
        return None;
    }
    let params = m.params.as_deref().unwrap_or(&[]);
    let target = parse_session_target(params).ok()?;
    match target {
        SessionTarget::Pinned(fader) => mapping::pinned_target(&cfg.pinned_apps, fader),
        SessionTarget::Discovered(slot) => mapping::discovered_target(pinned_lc, disc, slot),
        SessionTarget::Auto => {
            let auto_idx = auto_strip_index(page, action, control_id)?;
            mapping::discovered_target(pinned_lc, disc, auto_idx)
        },
    }
}

/// LCD label for a process: pinned `display_name` if set, otherwise
/// `derive_label`.
fn label_for_process(pinned: &[crate::config::PinnedApp], process_lc: &str) -> String {
    if let Some(pin) = pinned
        .iter()
        .find(|p| p.process_name.to_lowercase() == process_lc)
    {
        pin.display_name
            .clone()
            .unwrap_or_else(|| mapping::derive_label(&pin.process_name))
    } else {
        mapping::derive_label(process_lc)
    }
}

/// LCD color for a process: pinned `color` if set in YAML, else the
/// cycle color from `assigned_color`. Falls back to white (7) if a
/// process somehow has no assigned color (defensive — should not
/// happen with normal lifecycle).
fn color_for_process(
    pinned: &[crate::config::PinnedApp],
    disc: &mapping::DiscoveryState,
    process_lc: &str,
) -> u8 {
    if let Some(pin) = pinned
        .iter()
        .find(|p| p.process_name.to_lowercase() == process_lc)
    {
        if let Some(color) = pin.color.as_ref() {
            return color.to_u8();
        }
    }
    disc.color_for(process_lc).unwrap_or(7)
}

#[cfg(test)]
mod auto_strip_tests {
    use super::*;
    use std::collections::HashMap;

    fn ctl(action: &str, target: &str) -> ControlMapping {
        ControlMapping {
            app: DRIVER_NAME.to_string(),
            action: Some(action.to_string()),
            params: Some(vec![serde_json::json!(target)]),
            midi: None,
            indicator: None,
            overlay: None,
        }
    }

    #[test]
    fn strip_index_parses_fader_and_mute() {
        assert_eq!(strip_index_of("fader1"), Some(1));
        assert_eq!(strip_index_of("fader8"), Some(8));
        assert_eq!(strip_index_of("mute4"), Some(4));
        assert_eq!(strip_index_of("fader_master"), None);
        assert_eq!(strip_index_of("flip"), None);
        assert_eq!(strip_index_of("fader9"), None);
    }

    #[test]
    fn auto_strip_index_orders_by_strip_number() {
        let mut controls: HashMap<String, ControlMapping> = HashMap::new();
        controls.insert("fader7".into(), ctl("session_volume", "auto"));
        controls.insert("fader4".into(), ctl("session_volume", "auto"));
        controls.insert("fader6".into(), ctl("session_volume", "auto"));
        controls.insert("fader5".into(), ctl("session_volume", "auto"));
        // Pinned strips are not in the auto list.
        controls.insert("fader1".into(), ctl("session_volume", "pinned:1"));
        // Mute auto strip — different action, must not affect volume ordering.
        controls.insert("mute4".into(), ctl("session_mute", "auto"));

        let page = PageConfig {
            name: "Windows Audio".into(),
            controls: Some(controls),
            ..Default::default()
        };

        assert_eq!(auto_strip_index(&page, "session_volume", "fader4"), Some(0));
        assert_eq!(auto_strip_index(&page, "session_volume", "fader5"), Some(1));
        assert_eq!(auto_strip_index(&page, "session_volume", "fader6"), Some(2));
        assert_eq!(auto_strip_index(&page, "session_volume", "fader7"), Some(3));
        assert_eq!(auto_strip_index(&page, "session_volume", "fader1"), None);
        assert_eq!(auto_strip_index(&page, "session_mute", "mute4"), Some(0));
    }
}
