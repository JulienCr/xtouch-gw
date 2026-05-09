//! Dedicated COM (STA) thread for Win32 audio operations.
//!
//! Public API is the [`ComThreadHandle`]: spawn the thread, send commands
//! over an `mpsc` channel, and shut it down on driver teardown. All COM
//! interface usage is confined to this thread.

#![cfg(target_os = "windows")]

use std::sync::Arc;
use std::thread::JoinHandle;

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolumeCallback;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use super::callback::EndpointVolumeCallback;
use super::master::MasterEndpoint;
use super::session::{set_session_volume, toggle_session_mute, SessionManager};

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

        let join = std::thread::Builder::new()
            .name("xtouch-gw-winaudio".into())
            .spawn(move || run_com_loop(&mut cmd_rx, event_tx))?;

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

fn run_com_loop(cmd_rx: &mut mpsc::Receiver<AudioCmd>, event_tx: mpsc::Sender<AudioEvent>) {
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

    // Register the volume-change callback. Must keep the COM reference
    // alive in scope so the audio engine can call back into it; we
    // unregister before drop.
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

    // Push initial state so the X-Touch is in sync immediately.
    if let Some(ep) = endpoint.as_ref() {
        if let (Ok(scalar), Ok(mute)) = (ep.get_volume_scalar(), ep.get_mute()) {
            let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute });
        }
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
                        let _ =
                            event_tx.send(AudioEvent::MasterVolumeChanged { scalar, mute: !cur });
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
                    match mgr.enumerate() {
                        Ok(sessions) => {
                            for s in sessions {
                                if s.process_name.is_empty() {
                                    continue;
                                }
                                let scalar = unsafe { s.volume.GetMasterVolume().unwrap_or(0.0) };
                                let mute = unsafe {
                                    s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false)
                                };
                                let _ = event_tx.try_send(AudioEvent::SessionVolumeSnapshot {
                                    process_name_lc: s.process_name,
                                    scalar,
                                    mute,
                                });
                            }
                        },
                        Err(e) => warn!("RefreshSessions enumerate failed: {}", e),
                    }
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
                if let Some(mgr) = session_mgr.as_ref() {
                    match mgr.enumerate() {
                        Ok(sessions) => {
                            if let Some(s) =
                                super::mapping::find_session(&sessions, &process_name_lc)
                            {
                                if let Err(e) = toggle_session_mute(s) {
                                    warn!("ToggleSessionMute({}) failed: {}", process_name_lc, e);
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

    // Unregister the callback before dropping the endpoint.
    if let (Some(ep), Some(cb)) = (endpoint.as_ref(), registered.as_ref()) {
        if let Err(e) = unsafe { ep.iface().UnregisterControlChangeNotify(cb) } {
            warn!("UnregisterControlChangeNotify failed: {:?}", e);
        }
    }
    drop(registered);
    drop(endpoint);
    drop(session_mgr);
    unsafe { CoUninitialize() };
}
