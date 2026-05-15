//! Windows media transport driver: play/pause, stop, next, previous.
//!
//! Outbound: sends standard Windows multimedia virtual-key events via
//! `SendInput`. Any media-aware app that listens for them (Spotify,
//! Chrome, YouTube, VLC, Media Player, etc.) responds — same behavior
//! as the dedicated media keys on a multimedia keyboard.
//!
//! Inbound: polls the System Media Transport Controls (SMTC) every
//! 500 ms and lights up the X-Touch button bound to `play_pause` while
//! the system reports a session in the `Playing` state. Feedback
//! flows through the unified router channel as a synthetic
//! `("winmedia", note_on_bytes)` message so it shares the X-Touch
//! anti-echo + page-filter pipeline with every other driver.
//!
//! On non-Windows targets this driver is a no-op stub that logs every
//! action — useful for testing the YAML routing path without hardware.

use crate::api_editor::action_catalog::ActionDescriptor;
use crate::drivers::{Driver, ExecutionContext};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

/// Driver name used for `app: "winmedia"` in YAML control mappings.
pub const DRIVER_NAME: &str = "winmedia";

/// SMTC poll cadence. 500 ms is comfortably below the threshold where a
/// user notices the play LED lagging the actual playback state, while
/// keeping per-tick CPU work negligible.
const SMTC_POLL_INTERVAL_MS: u64 = 500;

/// Synthetic feedback channel used to inject `("winmedia", note_on_bytes)`
/// updates into the unified router feedback pipeline.
type FeedbackSender = mpsc::Sender<(String, Vec<u8>)>;

/// Shared optional handle to a feedback sender, populated by `set_feedback_sender`.
type SharedFeedbackTx = Arc<RwLock<Option<FeedbackSender>>>;

pub struct WinMediaDriver {
    initialized: AtomicBool,
    /// Set to `true` when `shutdown()` is called. The polling loop
    /// observes this between ticks and exits.
    shutdown_flag: Arc<AtomicBool>,
    /// Wired post-construction by `set_router`. Used to resolve the
    /// active page → MIDI spec for the play LED.
    router: Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    /// Wired post-construction. The polling task uses this to inject
    /// synthetic `("winmedia", note_on_bytes)` feedback into the unified
    /// router feedback path.
    feedback_tx: SharedFeedbackTx,
    /// Wired post-construction. Reused for every play-LED resolution
    /// instead of reloading from disk on each emit.
    control_db: Arc<RwLock<Option<Arc<crate::control_mapping::ControlMappingDB>>>>,
    /// Last observed SMTC playing state. `None` before the first poll.
    /// Stored so we can re-emit on page change without waiting for the
    /// next state transition.
    last_state: Arc<RwLock<Option<bool>>>,
}

impl WinMediaDriver {
    pub fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            router: Arc::new(RwLock::new(None)),
            feedback_tx: Arc::new(RwLock::new(None)),
            control_db: Arc::new(RwLock::new(None)),
            last_state: Arc::new(RwLock::new(None)),
        }
    }

    /// Wire the driver to the router so it can resolve the active page
    /// to a MIDI control spec for LED feedback.
    pub async fn set_router(&self, router: Arc<crate::router::Router>) {
        *self.router.write().await = Some(router);
    }

    /// Wire the driver to the unified feedback channel.
    pub async fn set_feedback_sender(&self, tx: mpsc::Sender<(String, Vec<u8>)>) {
        *self.feedback_tx.write().await = Some(tx);
    }

    /// Share the application-wide control mapping DB so the play-LED
    /// resolver can avoid re-reading the embedded CSV on every poll.
    pub async fn set_control_db(&self, db: Arc<crate::control_mapping::ControlMappingDB>) {
        *self.control_db.write().await = Some(db);
    }
}

impl Default for WinMediaDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for WinMediaDriver {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    async fn init(&self, _ctx: ExecutionContext) -> Result<()> {
        info!("WinMedia driver initialized");
        // Re-arm shutdown flag on every init so the same instance can be
        // unregistered and re-registered across profile switches.
        self.shutdown_flag.store(false, Ordering::Release);
        self.initialized.store(true, Ordering::Release);

        #[cfg(target_os = "windows")]
        {
            self.spawn_smtc_poller().await;
            self.spawn_page_watcher().await;
        }

        Ok(())
    }

    async fn execute(
        &self,
        action: &str,
        _params: Vec<Value>,
        ctx: ExecutionContext,
    ) -> Result<()> {
        if !self.initialized.load(Ordering::Acquire) {
            warn!(
                "WinMedia driver not initialized, dropping action '{}'",
                action
            );
            return Ok(());
        }

        // All transport actions are press-only — releasing the X-Touch
        // button must not retrigger them. Holding-to-repeat isn't a
        // useful mode for play/pause/next/etc.
        if ctx.is_button_release() {
            return Ok(());
        }

        let Some(vk) = vk_for_action(action) else {
            warn!("Unknown winmedia action '{}'", action);
            return Ok(());
        };

        debug!("WinMedia: action '{}' -> VK 0x{:02X}", action, vk);

        #[cfg(target_os = "windows")]
        send_media_key(vk);

        #[cfg(not(target_os = "windows"))]
        {
            let _ = vk;
            debug!("WinMedia: '{}' is a no-op on non-Windows", action);
        }

        Ok(())
    }

    async fn sync(&self) -> Result<()> {
        debug!("WinMedia sync requested (stateless driver)");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.initialized.store(false, Ordering::Release);
        self.shutdown_flag.store(true, Ordering::Release);
        debug!("WinMedia driver shut down");
        Ok(())
    }

    fn action_catalog(&self) -> Vec<ActionDescriptor> {
        media_catalog()
    }
}

#[cfg(target_os = "windows")]
impl WinMediaDriver {
    /// Background loop that polls SMTC on a fixed cadence and emits a
    /// play LED indicator whenever the system playback state changes.
    async fn spawn_smtc_poller(&self) {
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let router = Arc::clone(&self.router);
        let feedback_tx = Arc::clone(&self.feedback_tx);
        let control_db = Arc::clone(&self.control_db);
        let last_state = Arc::clone(&self.last_state);

        tokio::spawn(async move {
            debug!("WinMedia SMTC poller started");
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_millis(SMTC_POLL_INTERVAL_MS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;
                if shutdown_flag.load(Ordering::Acquire) {
                    debug!("WinMedia SMTC poller exiting (shutdown)");
                    break;
                }

                // Run the COM/WinRT calls on a blocking worker so they
                // can't stall the tokio runtime if the SMTC manager
                // takes longer than expected to respond.
                let playing = match tokio::task::spawn_blocking(read_smtc_playing).await {
                    Ok(state) => state,
                    Err(e) => {
                        debug!("SMTC poller spawn_blocking failed: {}", e);
                        continue;
                    },
                };

                let mut last = last_state.write().await;
                if *last == Some(playing) {
                    continue;
                }
                *last = Some(playing);
                drop(last);

                emit_play_indicator(&feedback_tx, &router, &control_db, playing).await;
            }
        });
    }

    /// Re-emit the cached play state every time the user activates a
    /// page. Necessary because SMTC only reports *changes* — without
    /// this, switching from a non-media page back to one with a play
    /// button would leave the LED in its last-cached state regardless
    /// of what's actually playing.
    async fn spawn_page_watcher(&self) {
        let router_arc = self.router.read().await.clone();
        let Some(router) = router_arc else {
            debug!("WinMedia: no router wired, skipping page watcher");
            return;
        };
        let Some(live_tx) = router.live_tx_snapshot().await else {
            debug!("WinMedia: live_tx not yet wired, skipping page watcher");
            return;
        };
        let mut rx = live_tx.subscribe();
        let feedback_tx = Arc::clone(&self.feedback_tx);
        let router_handle = Arc::clone(&self.router);
        let control_db = Arc::clone(&self.control_db);
        let last_state = Arc::clone(&self.last_state);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);

        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if shutdown_flag.load(Ordering::Acquire) {
                    break;
                }
                if !matches!(event, crate::event_bus::LiveEvent::PageChanged { .. }) {
                    continue;
                }
                let cached = *last_state.read().await;
                if let Some(playing) = cached {
                    emit_play_indicator(&feedback_tx, &router_handle, &control_db, playing).await;
                }
            }
        });
    }
}

/// Map a YAML action string to its Windows virtual-key code.
/// Returns `None` for unknown actions.
fn vk_for_action(action: &str) -> Option<u16> {
    match action {
        "play_pause" | "playpause" | "toggle" | "play" | "pause" => Some(VK_MEDIA_PLAY_PAUSE),
        "stop" => Some(VK_MEDIA_STOP),
        "next" | "next_track" | "nexttrack" | "forward" => Some(VK_MEDIA_NEXT_TRACK),
        "previous" | "prev" | "prev_track" | "previous_track" | "prevtrack" | "back" => {
            Some(VK_MEDIA_PREV_TRACK)
        },
        _ => None,
    }
}

// Windows VK codes for multimedia keys (from WinUser.h). Defined here so
// the constants exist on every platform, even though the SendInput call
// itself is Windows-only.
const VK_MEDIA_NEXT_TRACK: u16 = 0xB0;
const VK_MEDIA_PREV_TRACK: u16 = 0xB1;
const VK_MEDIA_STOP: u16 = 0xB2;
const VK_MEDIA_PLAY_PAUSE: u16 = 0xB3;

#[cfg(target_os = "windows")]
fn send_media_key(vk: u16) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };

    let down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: KEYBD_EVENT_FLAGS(0),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let inputs = [down, up];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        warn!(
            "WinMedia: SendInput sent {}/{} events for VK 0x{:02X}",
            sent,
            inputs.len(),
            vk
        );
    }
}

/// Read whether *any* SMTC session reports a `Playing` status.
///
/// Uses the WinRT `GlobalSystemMediaTransportControlsSessionManager` —
/// the same global pipe Spotify, Chrome, VLC, etc. publish to. Returns
/// `false` if no session exists or the manager fails to report (e.g.
/// briefly during a session handover). Errors are intentionally
/// swallowed; the next 500 ms tick will retry.
#[cfg(target_os = "windows")]
fn read_smtc_playing() -> bool {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };

    let manager_op = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
        Ok(op) => op,
        Err(_) => return false,
    };
    let manager = match manager_op.get() {
        Ok(m) => m,
        Err(_) => return false,
    };
    let session = match manager.GetCurrentSession() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let info = match session.GetPlaybackInfo() {
        Ok(i) => i,
        Err(_) => return false,
    };
    let status = match info.PlaybackStatus() {
        Ok(s) => s,
        Err(_) => return false,
    };
    status == Status::Playing
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn read_smtc_playing() -> bool {
    false
}

/// Resolve the play-pause control on the active page to a MIDI Note
/// spec, then emit a NoteOn feedback message that lights (vel 127) or
/// extinguishes (vel 0) the bound X-Touch button LED.
async fn emit_play_indicator(
    feedback_tx: &SharedFeedbackTx,
    router: &Arc<RwLock<Option<Arc<crate::router::Router>>>>,
    control_db: &Arc<RwLock<Option<Arc<crate::control_mapping::ControlMappingDB>>>>,
    playing: bool,
) {
    let Some(tx) = feedback_tx.read().await.clone() else {
        return;
    };
    let Some(router) = router.read().await.clone() else {
        return;
    };
    let Some(page) = router.get_active_page().await else {
        return;
    };
    let Some(db) = control_db.read().await.clone() else {
        debug!("WinMedia: control DB not yet wired, dropping LED emit");
        return;
    };

    let mcu_mode = router.config.read().await.is_mcu_mode();
    let Some(spec) = resolve_play_pause_spec(&page, &db, mcu_mode) else {
        // No control bound — nothing to light. Common on pages without
        // transport buttons (lighting, etc.).
        return;
    };

    let bytes = spec.led_bytes(playing);
    if let Err(e) = tx.send((DRIVER_NAME.to_string(), bytes)).await {
        debug!("WinMedia: feedback channel closed: {}", e);
    }
}

/// Find the page control bound to `winmedia.play_pause` (or any of its
/// aliases) and resolve it to a hardware MIDI spec via
/// `control_mapping.csv`.
fn resolve_play_pause_spec(
    page: &crate::config::PageConfig,
    db: &crate::control_mapping::ControlMappingDB,
    mcu_mode: bool,
) -> Option<crate::control_mapping::MidiSpec> {
    let controls = page.controls.as_ref()?;
    let control_id = controls.iter().find_map(|(id, m)| {
        if m.app != DRIVER_NAME {
            return None;
        }
        let action = m.action.as_deref()?;
        (vk_for_action(action) == Some(VK_MEDIA_PLAY_PAUSE)).then(|| id.clone())
    })?;
    db.get_midi_spec(&control_id, mcu_mode)
}

fn media_catalog() -> Vec<ActionDescriptor> {
    vec![
        ActionDescriptor::simple("play_pause", "Play / pause")
            .with_description("Toggle playback in the foreground media-aware app."),
        ActionDescriptor::simple("stop", "Stop")
            .with_description("Stop playback in the foreground media-aware app."),
        ActionDescriptor::simple("next", "Next track").with_description("Skip to next track."),
        ActionDescriptor::simple("previous", "Previous track")
            .with_description("Skip to previous track."),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_lookup_canonical_names() {
        assert_eq!(vk_for_action("play_pause"), Some(VK_MEDIA_PLAY_PAUSE));
        assert_eq!(vk_for_action("stop"), Some(VK_MEDIA_STOP));
        assert_eq!(vk_for_action("next"), Some(VK_MEDIA_NEXT_TRACK));
        assert_eq!(vk_for_action("previous"), Some(VK_MEDIA_PREV_TRACK));
    }

    #[test]
    fn vk_lookup_aliases() {
        assert_eq!(vk_for_action("toggle"), Some(VK_MEDIA_PLAY_PAUSE));
        assert_eq!(vk_for_action("play"), Some(VK_MEDIA_PLAY_PAUSE));
        assert_eq!(vk_for_action("pause"), Some(VK_MEDIA_PLAY_PAUSE));
        assert_eq!(vk_for_action("forward"), Some(VK_MEDIA_NEXT_TRACK));
        assert_eq!(vk_for_action("prev"), Some(VK_MEDIA_PREV_TRACK));
        assert_eq!(vk_for_action("back"), Some(VK_MEDIA_PREV_TRACK));
    }

    #[test]
    fn vk_lookup_rejects_unknown() {
        assert_eq!(vk_for_action(""), None);
        assert_eq!(vk_for_action("rewind"), None); // ambiguous — not aliased
        assert_eq!(vk_for_action("foo"), None);
    }

    #[test]
    fn catalog_lists_four_actions() {
        let cat = media_catalog();
        assert_eq!(cat.len(), 4);
        let names: Vec<_> = cat.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"play_pause"));
        assert!(names.contains(&"stop"));
        assert!(names.contains(&"next"));
        assert!(names.contains(&"previous"));
    }

    #[test]
    fn led_bytes_note_lit_uses_velocity_127() {
        let spec = crate::control_mapping::MidiSpec::Note { note: 94 };
        let bytes = spec.led_bytes(true);
        // NoteOn channel 0 = 0x90, note 94, velocity 127.
        assert_eq!(bytes, vec![0x90, 94, 127]);
    }

    #[test]
    fn led_bytes_note_unlit_uses_velocity_0() {
        let spec = crate::control_mapping::MidiSpec::Note { note: 94 };
        let bytes = spec.led_bytes(false);
        assert_eq!(bytes, vec![0x90, 94, 0]);
    }
}
