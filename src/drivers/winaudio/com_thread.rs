//! Dedicated COM (STA) thread for Win32 audio operations.
//!
//! Public API is the [`ComThreadHandle`]: spawn the thread, send commands
//! over an `mpsc` channel, and shut it down on driver teardown. All COM
//! interface usage is confined to this thread.

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolumeCallback;
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionEvents, IAudioSessionNotification,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use super::callback::EndpointVolumeCallback;
use super::master::MasterEndpoint;
use super::session::{set_session_volume, toggle_session_mute, SessionManager};
use super::session_events::{NewSessionCallback, SessionEventsCallback};

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
struct SessionReg {
    control: IAudioSessionControl2,
    events: IAudioSessionEvents,
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
        register_session_events_for_all(mgr, &event_tx, &cmd_tx, &mut session_regs);
    }

    info!("WinAudio COM loop running");

    while let Some(cmd) = cmd_rx.blocking_recv() {
        match cmd {
            AudioCmd::SetMasterScalar(scalar) => {
                if let Some(ep) = endpoint.as_ref() {
                    if let Err(e) = ep.set_volume_scalar(scalar) {
                        warn!("set_volume_scalar({}) failed: {}", scalar, e);
                    } else {
                        let mute = ep.get_mute().unwrap_or(false);
                        let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute });
                    }
                }
            },
            AudioCmd::ToggleMasterMute => {
                if let Some(ep) = endpoint.as_ref() {
                    let cur = ep.get_mute().unwrap_or(false);
                    if let Err(e) = ep.set_mute(!cur) {
                        warn!("set_mute failed: {}", e);
                    } else {
                        let scalar = ep.get_volume_scalar().unwrap_or(0.0);
                        let _ = event_tx
                            .try_send(AudioEvent::MasterVolumeChanged { scalar, mute: !cur });
                    }
                }
            },
            AudioCmd::RefreshMaster => {
                if let Some(ep) = endpoint.as_ref() {
                    if let (Ok(scalar), Ok(mute)) = (ep.get_volume_scalar(), ep.get_mute()) {
                        let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute });
                    }
                }
            },
            AudioCmd::RefreshSessions => {
                if let Some(mgr) = session_mgr.as_ref() {
                    register_session_events_for_all(mgr, &event_tx, &cmd_tx, &mut session_regs);
                }
            },
            AudioCmd::SetSessionScalar {
                process_name_lc,
                scalar,
            } => {
                if let Some(mgr) = session_mgr.as_ref() {
                    match mgr.enumerate() {
                        Ok(sessions) => {
                            if let Some(s) =
                                super::mapping::find_session(&sessions, &process_name_lc)
                            {
                                if let Err(e) = set_session_volume(s, scalar) {
                                    warn!(
                                        "SetSessionVolume({}, {}) failed: {}",
                                        process_name_lc, scalar, e
                                    );
                                }
                            } else {
                                trace!(
                                    "No active session for '{}'; ignoring volume command",
                                    process_name_lc
                                );
                            }
                        },
                        Err(e) => warn!("session enumerate failed: {}", e),
                    }
                }
            },
            AudioCmd::ToggleSessionMute { process_name_lc } => {
                // Sessions have no callback equivalent to
                // `IAudioEndpointVolumeCallback`, so the mute LED would
                // never refresh unless we push a snapshot here ourselves.
                if let Some(mgr) = session_mgr.as_ref() {
                    match mgr.enumerate() {
                        Ok(sessions) => {
                            if let Some(s) =
                                super::mapping::find_session(&sessions, &process_name_lc)
                            {
                                match toggle_session_mute(s) {
                                    Ok(()) => {
                                        let scalar =
                                            unsafe { s.volume.GetMasterVolume().unwrap_or(0.0) };
                                        let mute = unsafe {
                                            s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false)
                                        };
                                        let _ =
                                            event_tx.try_send(AudioEvent::SessionVolumeSnapshot {
                                                process_name_lc: process_name_lc.clone(),
                                                scalar,
                                                mute,
                                            });
                                    },
                                    Err(e) => warn!(
                                        "ToggleSessionMute({}) failed: {}",
                                        process_name_lc, e
                                    ),
                                }
                            }
                        },
                        Err(e) => warn!("session enumerate failed: {}", e),
                    }
                }
            },
            AudioCmd::EnumerateSessions { reply } => {
                let names = session_mgr
                    .as_ref()
                    .and_then(|m| m.enumerate().ok())
                    .map(|sessions| {
                        sessions
                            .into_iter()
                            .map(|s| s.process_name)
                            .filter(|n| !n.is_empty())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
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

/// Enumerate all sessions, register an `IAudioSessionEvents` callback on
/// each new one, push a `SessionVolumeSnapshot` so the consumer sees
/// every session's current state immediately, then emit a single
/// `ActiveSessionsChanged` summary at the end. Re-running this is safe:
/// already-registered sessions are skipped (looked up by PID), and PIDs
/// that have disappeared since the previous pass are unregistered to
/// prevent the `session_regs` map from growing unbounded.
fn register_session_events_for_all(
    mgr: &SessionManager,
    event_tx: &mpsc::Sender<AudioEvent>,
    cmd_tx: &mpsc::Sender<AudioCmd>,
    session_regs: &mut HashMap<u32, SessionReg>,
) {
    let sessions = match mgr.enumerate() {
        Ok(s) => s,
        Err(e) => {
            warn!("session enumerate failed: {}", e);
            return;
        },
    };

    let mut active_names: Vec<String> = Vec::with_capacity(sessions.len());
    let mut active_pids: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(sessions.len());

    for s in sessions {
        if s.process_name.is_empty() {
            continue;
        }
        active_pids.insert(s.pid);
        if !active_names.contains(&s.process_name) {
            active_names.push(s.process_name.clone());
        }

        // Always push the current state — the consumer is idempotent and
        // this is what catches sessions whose volume changed while we
        // weren't subscribed (or before our callback was registered).
        let scalar = unsafe { s.volume.GetMasterVolume().unwrap_or(0.0) };
        let mute = unsafe { s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false) };
        let _ = event_tx.try_send(AudioEvent::SessionVolumeSnapshot {
            process_name_lc: s.process_name.clone(),
            scalar,
            mute,
        });

        // Skip if we already have an events callback registered for this PID.
        if session_regs.contains_key(&s.pid) {
            continue;
        }

        let cb_impl =
            SessionEventsCallback::new(event_tx.clone(), cmd_tx.clone(), s.process_name.clone());
        let events: IAudioSessionEvents = cb_impl.into();
        match unsafe { s.control.RegisterAudioSessionNotification(&events) } {
            Ok(()) => {
                debug!(
                    "RegisterAudioSessionNotification(pid={}, {}) succeeded",
                    s.pid, s.process_name
                );
                session_regs.insert(
                    s.pid,
                    SessionReg {
                        control: s.control,
                        events,
                    },
                );
            },
            Err(e) => warn!(
                "RegisterAudioSessionNotification(pid={}, {}) failed: {:?}",
                s.pid, s.process_name, e
            ),
        }
    }

    // Unregister callbacks for PIDs that have disappeared since the last
    // pass. Prevents the map from growing unboundedly across long-lived
    // sessions where many short-lived audio sources come and go.
    let stale_pids: Vec<u32> = session_regs
        .keys()
        .filter(|pid| !active_pids.contains(pid))
        .copied()
        .collect();
    for pid in stale_pids {
        if let Some(reg) = session_regs.remove(&pid) {
            if let Err(e) = unsafe { reg.control.UnregisterAudioSessionNotification(&reg.events) } {
                trace!(
                    "UnregisterAudioSessionNotification(pid={}) on stale session failed: {:?}",
                    pid,
                    e
                );
            }
        }
    }

    // Atomic "active set" marker — the consumer uses this to swap its
    // active-session view in one go.
    let _ = event_tx.try_send(AudioEvent::ActiveSessionsChanged {
        names_lc: active_names,
    });
}
