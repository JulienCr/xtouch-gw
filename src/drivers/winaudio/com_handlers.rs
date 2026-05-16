//! Per-command handlers for the COM thread's main loop.
//!
//! Extracted from `com_thread.rs` so [`super::com_thread::run_com_loop`]
//! stays under the project's 500-line file budget and the match arms
//! become 3-5 lines (CLAUDE.md function-size limits). See #37.
//!
//! Every helper takes `Option<&...>` for its endpoint/manager so the
//! caller can pass the borrowed `Option` straight from the loop without
//! re-checking. Callbacks emit `AudioEvent`s through the shared
//! `event_tx`.
//!
//! Module is `#[cfg(target_os = "windows")]`-gated at its parent declaration
//! in `mod.rs`.

use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use windows::Win32::Media::Audio::IAudioSessionEvents;

use super::com_thread::{AudioCmd, AudioEvent, SessionReg, SESSION_CACHE_TTL};
use super::master::MasterEndpoint;
use super::session::{set_session_volume, toggle_session_mute, SessionInfo, SessionManager};
use super::session_events::SessionEventsCallback;

/// Return a reference to a cached session matching `process_name_lc`,
/// re-enumerating once if the cache is missing/expired or the name
/// isn't found (handles the race where a transient session disappeared
/// after the last refresh). The returned reference borrows from
/// `sessions_cache`, which is updated in place on a refresh. Returns
/// `None` if no matching session exists after the refresh.
fn cached_session<'a>(
    mgr: &SessionManager,
    sessions_cache: &'a mut Option<(Instant, Vec<SessionInfo>)>,
    process_name_lc: &str,
) -> Option<&'a SessionInfo> {
    let needs_refresh = match sessions_cache.as_ref() {
        None => true,
        Some((ts, sessions)) => {
            ts.elapsed() > SESSION_CACHE_TTL
                || super::mapping::find_session(sessions, process_name_lc).is_none()
        },
    };
    if needs_refresh {
        match mgr.enumerate() {
            Ok(sessions) => {
                *sessions_cache = Some((Instant::now(), sessions));
            },
            Err(e) => {
                warn!("session enumerate failed (cache refresh): {}", e);
                return None;
            },
        }
    }
    let sessions = &sessions_cache.as_ref()?.1;
    super::mapping::find_session(sessions, process_name_lc)
}

/// Apply `scalar` to the cached session matching `process_name_lc`,
/// refreshing the cache on miss/TTL expiry. See #35.
pub(super) fn handle_set_session_scalar(
    mgr: &SessionManager,
    sessions_cache: &mut Option<(Instant, Vec<SessionInfo>)>,
    process_name_lc: &str,
    scalar: f32,
) {
    let Some(s) = cached_session(mgr, sessions_cache, process_name_lc) else {
        trace!(
            "No active session for '{}'; ignoring volume command",
            process_name_lc
        );
        return;
    };
    if let Err(e) = set_session_volume(s, scalar) {
        warn!(
            "SetSessionVolume({}, {}) failed: {}",
            process_name_lc, scalar, e
        );
    }
}

/// Toggle mute on the cached session matching `process_name_lc` and
/// emit the resulting state. Sessions have no callback equivalent to
/// `IAudioEndpointVolumeCallback`, so the mute LED would never refresh
/// unless we push a snapshot here ourselves. See #35.
pub(super) fn handle_toggle_session_mute(
    mgr: &SessionManager,
    sessions_cache: &mut Option<(Instant, Vec<SessionInfo>)>,
    process_name_lc: &str,
    event_tx: &mpsc::Sender<AudioEvent>,
) {
    let Some(s) = cached_session(mgr, sessions_cache, process_name_lc) else {
        return;
    };
    match toggle_session_mute(s) {
        Ok(()) => {
            let scalar = unsafe { s.volume.GetMasterVolume().unwrap_or(0.0) };
            let mute = unsafe { s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false) };
            let _ = event_tx.try_send(AudioEvent::SessionVolumeSnapshot {
                process_name_lc: process_name_lc.to_string(),
                scalar,
                mute,
            });
        },
        Err(e) => warn!("ToggleSessionMute({}) failed: {}", process_name_lc, e),
    }
}

/// Apply `scalar` to the master endpoint. When the OS volume-change
/// callback is wired (normal path) the callback fans out the resulting
/// state; otherwise we emit a synthetic `MasterVolumeChanged` so the
/// X-Touch stays in sync. See #41.
pub(super) fn handle_master_scalar(
    endpoint: Option<&MasterEndpoint>,
    scalar: f32,
    callback_active: bool,
    event_tx: &mpsc::Sender<AudioEvent>,
) {
    let Some(ep) = endpoint else {
        return;
    };
    if let Err(e) = ep.set_volume_scalar(scalar) {
        warn!("set_volume_scalar({}) failed: {}", scalar, e);
        return;
    }
    if !callback_active {
        let mute = ep.get_mute().unwrap_or(false);
        let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute });
    }
}

/// Toggle master mute on the endpoint. Same callback-vs-synthetic
/// emit logic as [`handle_master_scalar`]. See #41.
pub(super) fn handle_master_mute(
    endpoint: Option<&MasterEndpoint>,
    callback_active: bool,
    event_tx: &mpsc::Sender<AudioEvent>,
) {
    let Some(ep) = endpoint else {
        return;
    };
    let cur = ep.get_mute().unwrap_or(false);
    if let Err(e) = ep.set_mute(!cur) {
        warn!("set_mute failed: {}", e);
        return;
    }
    if !callback_active {
        let scalar = ep.get_volume_scalar().unwrap_or(0.0);
        let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute: !cur });
    }
}

/// Read the current master state and emit it as a `MasterVolumeChanged`
/// event. Used to re-sync the X-Touch on page activation (the OS
/// callback only fires on actual *changes*).
pub(super) fn handle_refresh_master(
    endpoint: Option<&MasterEndpoint>,
    event_tx: &mpsc::Sender<AudioEvent>,
) {
    let Some(ep) = endpoint else {
        return;
    };
    if let (Ok(scalar), Ok(mute)) = (ep.get_volume_scalar(), ep.get_mute()) {
        let _ = event_tx.try_send(AudioEvent::MasterVolumeChanged { scalar, mute });
    }
}

/// Re-enumerate active sessions, register per-session events on any new
/// ones, and refresh the cache used by hot-path `Set*` / `Toggle*` arms.
pub(super) fn handle_refresh_sessions(
    mgr: Option<&SessionManager>,
    event_tx: &mpsc::Sender<AudioEvent>,
    cmd_tx: &mpsc::Sender<AudioCmd>,
    session_regs: &mut HashMap<u32, SessionReg>,
    sessions_cache: &mut Option<(Instant, Vec<SessionInfo>)>,
) {
    let Some(mgr) = mgr else {
        return;
    };
    let sessions = register_session_events_for_all(mgr, event_tx, cmd_tx, session_regs);
    *sessions_cache = Some((Instant::now(), sessions));
}

/// Enumerate sessions, populate the cache, and return their lowercase
/// process names. Used by the `EnumerateSessions` oneshot reply.
pub(super) fn handle_enumerate_sessions(
    mgr: Option<&SessionManager>,
    sessions_cache: &mut Option<(Instant, Vec<SessionInfo>)>,
) -> Vec<String> {
    mgr.and_then(|m| m.enumerate().ok())
        .map(|sessions| {
            let names = sessions
                .iter()
                .map(|s| s.process_name.clone())
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>();
            *sessions_cache = Some((Instant::now(), sessions));
            names
        })
        .unwrap_or_default()
}

/// Enumerate all sessions, register an `IAudioSessionEvents` callback on
/// each new one, push a `SessionVolumeSnapshot` so the consumer sees
/// every session's current state immediately, then emit a single
/// `ActiveSessionsChanged` summary at the end. Re-running this is safe:
/// already-registered sessions are skipped (looked up by PID), and PIDs
/// that have disappeared since the previous pass are unregistered to
/// prevent the `session_regs` map from growing unbounded.
///
/// Returns the freshly enumerated `SessionInfo`s so callers can populate
/// the cache used by hot-path `Set*` / `Toggle*` arms (#35).
pub(super) fn register_session_events_for_all(
    mgr: &SessionManager,
    event_tx: &mpsc::Sender<AudioEvent>,
    cmd_tx: &mpsc::Sender<AudioCmd>,
    session_regs: &mut HashMap<u32, SessionReg>,
) -> Vec<SessionInfo> {
    let sessions = match mgr.enumerate() {
        Ok(s) => s,
        Err(e) => {
            warn!("session enumerate failed: {}", e);
            return Vec::new();
        },
    };

    let mut active_names: Vec<String> = Vec::with_capacity(sessions.len());
    let mut active_pids: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(sessions.len());

    for s in &sessions {
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

        register_session_events_for_one(s, event_tx, cmd_tx, session_regs);
    }

    purge_stale_session_regs(session_regs, &active_pids);

    // Atomic "active set" marker — the consumer uses this to swap its
    // active-session view in one go.
    let _ = event_tx.try_send(AudioEvent::ActiveSessionsChanged {
        names_lc: active_names,
    });

    sessions
}

/// Register an `IAudioSessionEvents` callback on `s` if no callback is
/// already registered for its PID. No-op on re-register.
fn register_session_events_for_one(
    s: &SessionInfo,
    event_tx: &mpsc::Sender<AudioEvent>,
    cmd_tx: &mpsc::Sender<AudioCmd>,
    session_regs: &mut HashMap<u32, SessionReg>,
) {
    if session_regs.contains_key(&s.pid) {
        return;
    }
    let cb_impl =
        SessionEventsCallback::new(event_tx.clone(), cmd_tx.clone(), s.process_name.clone());
    let events: IAudioSessionEvents = cb_impl.into();
    // Cloning a COM interface bumps the refcount; the registration
    // map then holds an independent reference to the same control
    // object that we also keep in the cache.
    match unsafe { s.control.RegisterAudioSessionNotification(&events) } {
        Ok(()) => {
            debug!(
                "RegisterAudioSessionNotification(pid={}, {}) succeeded",
                s.pid, s.process_name
            );
            session_regs.insert(
                s.pid,
                SessionReg {
                    control: s.control.clone(),
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

/// Unregister callbacks for PIDs that have disappeared since the last
/// pass. Prevents the map from growing unboundedly across long-lived
/// runs where many short-lived audio sources come and go.
fn purge_stale_session_regs(
    session_regs: &mut HashMap<u32, SessionReg>,
    active_pids: &std::collections::HashSet<u32>,
) {
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
}
