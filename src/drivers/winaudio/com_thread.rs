//! Dedicated COM (STA) thread for Win32 audio operations.
//!
//! Public API is the [`ComThreadHandle`]: spawn the thread, send commands
//! over an `mpsc` channel, and shut it down on driver teardown. All COM
//! interface usage is confined to this thread.
//!
//! Module is `#[cfg(target_os = "windows")]`-gated at its parent declaration
//! in `mod.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolumeCallback;
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionEvents, IAudioSessionNotification,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use super::callback::EndpointVolumeCallback;
use super::com_handlers::{
    handle_enumerate_sessions, handle_master_mute, handle_master_scalar, handle_refresh_master,
    handle_refresh_sessions, handle_set_session_scalar, handle_toggle_session_mute,
    register_session_events_for_all,
};
use super::master::MasterEndpoint;
use super::session::{SessionInfo, SessionManager};
use super::session_events::NewSessionCallback;

/// Max age for the cached session enumeration before a hot-path access
/// triggers a fresh re-enumerate. `OnSessionCreated` /
/// `OnStateChanged(Expired)` proactively invalidate the cache, so this
/// is a safety net for transient sessions that slipped between events.
pub(super) const SESSION_CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum AudioCmd {
    SetMasterScalar(f32),
    ToggleMasterMute,
    /// Read the current master endpoint state and push it as an
    /// `AudioEvent::MasterVolumeChanged` event. Used to re-sync the
    /// X-Touch fader after a page change to "Windows Audio" (the
    /// `IAudioEndpointVolumeCallback` only fires on volume *changes*,
    /// not on demand).
    RefreshMaster,
    /// Re-enumerate active sessions and emit one
    /// `AudioEvent::SessionVolumeSnapshot` per session.
    RefreshSessions,
    /// Set volume (0.0..=1.0) for the first session whose process name
    /// matches `process_name_lc` (lowercase). Silently ignored if no
    /// matching session exists.
    SetSessionScalar {
        process_name_lc: String,
        scalar: f32,
    },
    ToggleSessionMute {
        process_name_lc: String,
    },
    /// Re-enumerate active sessions and reply with the lowercase exe
    /// names via `reply` (oneshot).
    EnumerateSessions {
        reply: tokio::sync::oneshot::Sender<Vec<String>>,
    },
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    MasterVolumeChanged {
        scalar: f32,
        mute: bool,
    },
    /// Emitted on demand (after `RefreshSessions`) for each active session.
    /// `process_name_lc` is the lowercase exe name (e.g. "discord.exe").
    SessionVolumeSnapshot {
        process_name_lc: String,
        scalar: f32,
        mute: bool,
    },
    /// Emitted at the end of every `RefreshSessions` pass — once the
    /// current set of active sessions has been fully enumerated. The
    /// vector contains the lowercase exe name of every session
    /// currently producing audio (no duplicates). Acts as an atomic
    /// marker so the consumer can replace its "active" set in one shot
    /// rather than trying to derive it from the snapshot burst.
    ActiveSessionsChanged {
        names_lc: Vec<String>,
    },
}

/// Bounded capacity for the command queue. Faders generate ~30 PB/s; 64
/// is plenty of headroom while keeping memory bounded if the COM thread
/// stalls. Senders use `try_send` and drop on full — losing a stale
/// fader sample is preferred over unbounded growth.
const CMD_QUEUE: usize = 64;
/// Bounded capacity for the event queue. The OS audio engine fires
/// `OnNotify` per channel-change; 256 covers a burst from a sweeping
/// system mixer without growing the queue indefinitely if the consumer
/// stalls (e.g. router lock contention).
const EVENT_QUEUE: usize = 256;

pub struct ComThreadHandle {
    cmd_tx: mpsc::Sender<AudioCmd>,
    event_rx: Arc<Mutex<Option<mpsc::Receiver<AudioEvent>>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl ComThreadHandle {
    pub fn spawn() -> anyhow::Result<Self> {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<AudioCmd>(CMD_QUEUE);
        let (event_tx, event_rx) = mpsc::channel::<AudioEvent>(EVENT_QUEUE);

        // Cloned into the COM thread so the new-session COM callback
        // (registered on `IAudioSessionManager2`) can request a refresh
        // when an app starts producing audio after init.
        let cmd_tx_for_loop = cmd_tx.clone();
        let join = std::thread::Builder::new()
            .name("xtouch-gw-winaudio".into())
            .spawn(move || run_com_loop(&mut cmd_rx, event_tx, cmd_tx_for_loop))?;

        Ok(Self {
            cmd_tx,
            event_rx: Arc::new(Mutex::new(Some(event_rx))),
            join: Mutex::new(Some(join)),
        })
    }

    pub fn set_master_scalar(&self, scalar: f32) {
        let _ = self.cmd_tx.try_send(AudioCmd::SetMasterScalar(scalar));
    }

    pub fn toggle_master_mute(&self) {
        let _ = self.cmd_tx.try_send(AudioCmd::ToggleMasterMute);
    }

    pub fn refresh_master(&self) {
        let _ = self.cmd_tx.try_send(AudioCmd::RefreshMaster);
    }

    pub fn refresh_sessions(&self) {
        let _ = self.cmd_tx.try_send(AudioCmd::RefreshSessions);
    }

    pub fn set_session_scalar(&self, process_name_lc: String, scalar: f32) {
        let _ = self.cmd_tx.try_send(AudioCmd::SetSessionScalar {
            process_name_lc,
            scalar,
        });
    }

    pub fn toggle_session_mute(&self, process_name_lc: String) {
        let _ = self
            .cmd_tx
            .try_send(AudioCmd::ToggleSessionMute { process_name_lc });
    }

    pub async fn enumerate_sessions(&self) -> Vec<String> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        if self
            .cmd_tx
            .send(AudioCmd::EnumerateSessions { reply })
            .await
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Take the event receiver (one-shot — only one consumer is supported).
    pub async fn take_event_rx(&self) -> Option<mpsc::Receiver<AudioEvent>> {
        self.event_rx.lock().await.take()
    }

    pub async fn shutdown(self) {
        let _ = self.cmd_tx.send(AudioCmd::Shutdown).await;
        if let Some(join) = self.join.lock().await.take() {
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = join.join() {
                    error!("WinAudio COM thread join failed: {:?}", e);
                }
            })
            .await;
        }
    }
}

/// Per-PID registration: we keep the `IAudioSessionControl2` so we can
/// unregister at shutdown, and the `IAudioSessionEvents` interface so the
/// audio engine has a live reference to call back into.
pub(super) struct SessionReg {
    pub(super) control: IAudioSessionControl2,
    pub(super) events: IAudioSessionEvents,
}

fn run_com_loop(
    cmd_rx: &mut mpsc::Receiver<AudioCmd>,
    event_tx: mpsc::Sender<AudioEvent>,
    cmd_tx: mpsc::Sender<AudioCmd>,
) {
    // SAFETY: STA is required for IAudioSessionEvents callbacks; this thread
    // owns its apartment and never lets COM objects cross thread boundaries.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() {
        error!("CoInitializeEx failed: {:?}", init);
        return;
    }

    let endpoint = match MasterEndpoint::open() {
        Ok(ep) => Some(ep),
        Err(e) => {
            warn!("WinAudio default render endpoint unavailable: {}", e);
            None
        },
    };

    let session_mgr = match SessionManager::open() {
        Ok(m) => Some(m),
        Err(e) => {
            warn!("WinAudio session manager unavailable: {}", e);
            None
        },
    };

    // Register the master volume-change callback. Must keep the COM
    // reference alive in scope so the audio engine can call back into
    // it; we unregister before drop.
    let registered = endpoint.as_ref().and_then(|ep| {
        let cb_impl = EndpointVolumeCallback::new(event_tx.clone());
        let cb: IAudioEndpointVolumeCallback = cb_impl.into();
        match unsafe { ep.iface().RegisterControlChangeNotify(&cb) } {
            Ok(()) => {
                debug!("RegisterControlChangeNotify succeeded");
                Some(cb)
            },
            Err(e) => {
                warn!("RegisterControlChangeNotify failed: {:?}", e);
                None
            },
        }
    });

    // The `Set*Master*` arms below normally rely on the OS callback to
    // fan out the resulting state (avoiding duplicate events — see #41).
    // If callback registration failed above, the callback never fires,
    // so we must keep the legacy synthetic emit path to avoid the
    // X-Touch fader/LED freezing on local master actions.
    let master_callback_active = registered.is_some();

    // Register the new-session notification so we hear about sessions
    // that appear after init (e.g. an app starts producing audio).
    let new_session_reg = session_mgr.as_ref().and_then(|mgr| {
        let cb_impl = NewSessionCallback::new(cmd_tx.clone());
        let cb: IAudioSessionNotification = cb_impl.into();
        match unsafe { mgr.manager.RegisterSessionNotification(&cb) } {
            Ok(()) => {
                debug!("RegisterSessionNotification succeeded");
                Some(cb)
            },
            Err(e) => {
                warn!("RegisterSessionNotification failed: {:?}", e);
                None
            },
        }
    });

    // Track per-session registrations by PID so we register events
    // exactly once per session and can unregister on shutdown.
    let mut session_regs: HashMap<u32, SessionReg> = HashMap::new();

    // Cached session enumeration (live `SessionInfo`s) reused by the
    // `Set*` / `Toggle*` arms instead of re-enumerating per fader event.
    // Invalidated by `RefreshSessions` (which `OnSessionCreated` and
    // session-state callbacks already push) and by the TTL safety net.
    // See #35.
    let mut sessions_cache: Option<(Instant, Vec<SessionInfo>)> = None;

    // Push initial master state so the X-Touch is in sync immediately.
    if let Some(ep) = endpoint.as_ref() {
        if let (Ok(scalar), Ok(mute)) = (ep.get_volume_scalar(), ep.get_mute()) {
            let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute });
        }
    }

    // Initial session pass: register events on every active session and
    // push their current state. This handles apps that already had
    // audio sessions open when the gateway started.
    if let Some(mgr) = session_mgr.as_ref() {
        let sessions = register_session_events_for_all(mgr, &event_tx, &cmd_tx, &mut session_regs);
        sessions_cache = Some((Instant::now(), sessions));
    }

    info!("WinAudio COM loop running");

    while let Some(cmd) = cmd_rx.blocking_recv() {
        match cmd {
            AudioCmd::SetMasterScalar(scalar) => {
                handle_master_scalar(endpoint.as_ref(), scalar, master_callback_active, &event_tx);
            },
            AudioCmd::ToggleMasterMute => {
                handle_master_mute(endpoint.as_ref(), master_callback_active, &event_tx);
            },
            AudioCmd::RefreshMaster => {
                handle_refresh_master(endpoint.as_ref(), &event_tx);
            },
            AudioCmd::RefreshSessions => {
                handle_refresh_sessions(
                    session_mgr.as_ref(),
                    &event_tx,
                    &cmd_tx,
                    &mut session_regs,
                    &mut sessions_cache,
                );
            },
            AudioCmd::SetSessionScalar {
                process_name_lc,
                scalar,
            } => {
                if let Some(mgr) = session_mgr.as_ref() {
                    handle_set_session_scalar(mgr, &mut sessions_cache, &process_name_lc, scalar);
                }
            },
            AudioCmd::ToggleSessionMute { process_name_lc } => {
                if let Some(mgr) = session_mgr.as_ref() {
                    handle_toggle_session_mute(
                        mgr,
                        &mut sessions_cache,
                        &process_name_lc,
                        &event_tx,
                    );
                }
            },
            AudioCmd::EnumerateSessions { reply } => {
                let names = handle_enumerate_sessions(session_mgr.as_ref(), &mut sessions_cache);
                let _ = reply.send(names);
            },
            AudioCmd::Shutdown => {
                debug!("WinAudio COM loop shutting down");
                break;
            },
        }
    }

    // Unregister per-session events.
    for (pid, reg) in session_regs.drain() {
        if let Err(e) = unsafe { reg.control.UnregisterAudioSessionNotification(&reg.events) } {
            warn!(
                "UnregisterAudioSessionNotification(pid={}) failed: {:?}",
                pid, e
            );
        }
    }

    // Unregister the new-session notification.
    if let (Some(mgr), Some(cb)) = (session_mgr.as_ref(), new_session_reg.as_ref()) {
        if let Err(e) = unsafe { mgr.manager.UnregisterSessionNotification(cb) } {
            warn!("UnregisterSessionNotification failed: {:?}", e);
        }
    }

    // Unregister the master volume-change callback.
    if let (Some(ep), Some(cb)) = (endpoint.as_ref(), registered.as_ref()) {
        if let Err(e) = unsafe { ep.iface().UnregisterControlChangeNotify(cb) } {
            warn!("UnregisterControlChangeNotify failed: {:?}", e);
        }
    }
    drop(registered);
    drop(new_session_reg);
    drop(endpoint);
    drop(session_mgr);
    unsafe { CoUninitialize() };
}
