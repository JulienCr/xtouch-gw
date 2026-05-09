//! COM callback wrappers for Windows audio events.
//!
//! Implements `IAudioEndpointVolumeCallback` so external volume changes
//! (Win+Vol+/-, hardware buttons, the Windows mixer) flow back into the
//! gateway and drive the motorized fader / mute LED. The callback runs
//! on the COM (STA) thread; the only work it does is post a lightweight
//! `AudioEvent` onto the unbounded mpsc channel.

#![cfg(target_os = "windows")]

use tokio::sync::mpsc;
use tracing::trace;

use windows::core::implement;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolumeCallback_Impl;
use windows::Win32::Media::Audio::AUDIO_VOLUME_NOTIFICATION_DATA;

use super::com_thread::AudioEvent;

#[implement(windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolumeCallback)]
pub struct EndpointVolumeCallback {
    tx: mpsc::UnboundedSender<AudioEvent>,
}

impl EndpointVolumeCallback {
    pub fn new(tx: mpsc::UnboundedSender<AudioEvent>) -> Self {
        Self { tx }
    }
}

impl IAudioEndpointVolumeCallback_Impl for EndpointVolumeCallback {
    fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // SAFETY: The pointer is provided by the audio engine and is
        // guaranteed valid for the duration of this call.
        let data = unsafe { &*pnotify };
        let scalar = data.fMasterVolume.clamp(0.0, 1.0);
        let mute = data.bMuted.as_bool();
        trace!("OnNotify: scalar={:.3} mute={}", scalar, mute);
        let _ = self
            .tx
            .send(AudioEvent::MasterVolumeChanged { scalar, mute });
        Ok(())
    }
}
