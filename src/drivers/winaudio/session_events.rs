//! Per-session COM callbacks: `IAudioSessionEvents` for live volume/mute
//! change notifications and session-end detection, and
//! `IAudioSessionNotification` for catching sessions that appear after
//! driver startup (e.g. an app that starts producing audio later).
//!
//! Both callbacks fire on a WASAPI-internal thread, so the bodies must
//! be non-blocking. They communicate back to the COM thread by pushing
//! events / commands through `mpsc` channels (`try_send`, drop on full).
//!
//! Module is `#[cfg(target_os = "windows")]`-gated at its parent declaration
//! in `mod.rs`.

use tokio::sync::mpsc;
use tracing::trace;

use windows::core::{implement, GUID};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::{
    AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateExpired,
    IAudioSessionControl, IAudioSessionEvents_Impl, IAudioSessionNotification_Impl,
};

use super::com_thread::{AudioCmd, AudioEvent};

/// Listener attached to one `IAudioSessionControl` to relay volume/mute
/// changes back to the gateway as `SessionVolumeSnapshot` events, and
/// to request a session refresh whenever the session ends (process
/// exits, endpoint disconnects).
#[implement(windows::Win32::Media::Audio::IAudioSessionEvents)]
pub struct SessionEventsCallback {
    event_tx: mpsc::Sender<AudioEvent>,
    cmd_tx: mpsc::Sender<AudioCmd>,
    process_name_lc: String,
}

impl SessionEventsCallback {
    pub fn new(
        event_tx: mpsc::Sender<AudioEvent>,
        cmd_tx: mpsc::Sender<AudioCmd>,
        process_name_lc: String,
    ) -> Self {
        Self {
            event_tx,
            cmd_tx,
            process_name_lc,
        }
    }
}

#[allow(non_snake_case)]
impl IAudioSessionEvents_Impl for SessionEventsCallback {
    fn OnDisplayNameChanged(
        &self,
        _new_display_name: &windows::core::PCWSTR,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnIconPathChanged(
        &self,
        _new_icon_path: &windows::core::PCWSTR,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnSimpleVolumeChanged(
        &self,
        new_volume: f32,
        new_mute: BOOL,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        let scalar = new_volume.clamp(0.0, 1.0);
        let mute = new_mute.as_bool();
        trace!(
            "SessionEvents OnSimpleVolumeChanged: {} v={:.3} m={}",
            self.process_name_lc,
            scalar,
            mute
        );
        let _ = self.event_tx.try_send(AudioEvent::SessionVolumeSnapshot {
            process_name_lc: self.process_name_lc.clone(),
            scalar,
            mute,
        });
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _channel_count: u32,
        _new_channel_volume_array: *const f32,
        _changed_channel: u32,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(
        &self,
        _new_grouping_param: *const GUID,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnStateChanged(&self, state: AudioSessionState) -> windows::core::Result<()> {
        // `Expired` is the reliable signal that the owning process has
        // exited. `OnSessionDisconnected` only fires for endpoint-level
        // disconnects (device removed), not for plain app exits — so
        // without this hook, closing Discord would never blank its LCD.
        if state == AudioSessionStateExpired {
            trace!(
                "SessionEvents OnStateChanged(Expired): {}",
                self.process_name_lc
            );
            let _ = self.cmd_tx.try_send(AudioCmd::RefreshSessions);
        }
        Ok(())
    }

    fn OnSessionDisconnected(
        &self,
        _disconnect_reason: AudioSessionDisconnectReason,
    ) -> windows::core::Result<()> {
        trace!(
            "SessionEvents OnSessionDisconnected: {}",
            self.process_name_lc
        );
        let _ = self.cmd_tx.try_send(AudioCmd::RefreshSessions);
        Ok(())
    }
}

/// Listener attached to the session manager. When a new session is
/// created on the default render endpoint, request a session refresh
/// so the COM thread can register `IAudioSessionEvents` on it.
#[implement(windows::Win32::Media::Audio::IAudioSessionNotification)]
pub struct NewSessionCallback {
    cmd_tx: mpsc::Sender<AudioCmd>,
}

impl NewSessionCallback {
    pub fn new(cmd_tx: mpsc::Sender<AudioCmd>) -> Self {
        Self { cmd_tx }
    }
}

#[allow(non_snake_case)]
impl IAudioSessionNotification_Impl for NewSessionCallback {
    fn OnSessionCreated(
        &self,
        _new_session: Option<&IAudioSessionControl>,
    ) -> windows::core::Result<()> {
        trace!("IAudioSessionNotification: new session created");
        let _ = self.cmd_tx.try_send(AudioCmd::RefreshSessions);
        Ok(())
    }
}
