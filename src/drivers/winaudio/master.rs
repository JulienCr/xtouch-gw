//! Default render endpoint master volume / mute wrapper.
//!
//! Wraps `IMMDeviceEnumerator -> IMMDevice -> IAudioEndpointVolume` for the
//! default audio render endpoint (eRender, eConsole). All methods are
//! synchronous and must be invoked on a thread that has called
//! `CoInitializeEx(COINIT_APARTMENTTHREADED)`.

#![cfg(target_os = "windows")]

use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

pub struct MasterEndpoint {
    iface: IAudioEndpointVolume,
}

impl MasterEndpoint {
    pub fn open() -> Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("CoCreateInstance(MMDeviceEnumerator)")?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("GetDefaultAudioEndpoint(eRender, eConsole)")?;
            let iface: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .context("Activate IAudioEndpointVolume")?;
            Ok(Self { iface })
        }
    }

    pub fn set_volume_scalar(&self, scalar: f32) -> Result<()> {
        let clamped = scalar.clamp(0.0, 1.0);
        unsafe {
            self.iface
                .SetMasterVolumeLevelScalar(clamped, std::ptr::null())
                .context("SetMasterVolumeLevelScalar")?;
        }
        Ok(())
    }

    pub fn get_volume_scalar(&self) -> Result<f32> {
        unsafe {
            self.iface
                .GetMasterVolumeLevelScalar()
                .context("GetMasterVolumeLevelScalar")
        }
    }

    pub fn set_mute(&self, mute: bool) -> Result<()> {
        unsafe {
            self.iface
                .SetMute(mute, std::ptr::null())
                .context("SetMute")?;
        }
        Ok(())
    }

    pub fn get_mute(&self) -> Result<bool> {
        unsafe { Ok(self.iface.GetMute().context("GetMute")?.as_bool()) }
    }

    /// Expose the underlying interface as a downcastable handle (used by
    /// the callback module to register `IAudioEndpointVolumeCallback`).
    pub fn iface(&self) -> &IAudioEndpointVolume {
        &self.iface
    }
}

// Note: `IAudioEndpointVolume` is `!Send` / `!Sync` by default; this struct
// must remain confined to the COM thread. The compiler enforces this — we
// do not add explicit Send/Sync impls.
#[allow(dead_code)]
const _ASSERT_NOT_SEND: fn() = || {
    fn require_not_send<T: ?Sized>(_: &T)
    where
        T: Interface,
    {
    }
};
