//! Audio session enumeration and per-app volume control.
//!
//! Wraps `IAudioSessionManager2` / `IAudioSessionEnumerator` and resolves
//! sessions to their owning process via `IAudioSessionControl2::GetProcessId`
//! plus `QueryFullProcessImageNameW`. Volume changes use `ISimpleAudioVolume`
//! (which every session control instance can be cast to).
//!
//! Module is `#[cfg(target_os = "windows")]`-gated at its parent declaration
//! in `mod.rs`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{debug, trace};
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub pid: u32,
    /// Lowercase executable name (e.g. "discord.exe"). Empty if the
    /// owning process couldn't be opened (e.g. system session, race).
    pub process_name: String,
    /// Live `ISimpleAudioVolume` interface. Confined to the COM thread.
    pub volume: ISimpleAudioVolume,
    /// Live session-control interface, kept so we can register
    /// `IAudioSessionEvents` for live volume-change notifications.
    /// Confined to the COM thread.
    pub control: IAudioSessionControl2,
}

pub struct SessionManager {
    pub manager: IAudioSessionManager2,
}

impl SessionManager {
    pub fn open() -> Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("CoCreateInstance(MMDeviceEnumerator)")?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("GetDefaultAudioEndpoint(eRender, eConsole)")?;
            let manager: IAudioSessionManager2 = device
                .Activate(CLSCTX_ALL, None)
                .context("Activate IAudioSessionManager2")?;
            Ok(Self { manager })
        }
    }

    /// Enumerate all currently active sessions on the default render endpoint.
    /// Skips system sessions (process_id == 0) and sessions whose owning
    /// process can no longer be opened.
    ///
    /// No per-PID name cache: Windows recycles PIDs immediately after
    /// process death, so any cache keyed solely on PID can return the
    /// previous owner's name. The hot path is already protected by the
    /// 2 s `sessions_cache` in `com_thread.rs`, which holds whole
    /// `SessionInfo`s — this function only runs on (re)enumerate, where
    /// re-resolving each PID's image name is cheap enough.
    pub fn enumerate(&self) -> Result<Vec<SessionInfo>> {
        let mut out = Vec::new();
        unsafe {
            let enumerator = self
                .manager
                .GetSessionEnumerator()
                .context("GetSessionEnumerator")?;
            let count = enumerator.GetCount().context("session GetCount")?;
            for i in 0..count {
                let control = match enumerator.GetSession(i) {
                    Ok(c) => c,
                    Err(e) => {
                        trace!("GetSession({}) failed: {:?}", i, e);
                        continue;
                    },
                };
                let control2: IAudioSessionControl2 = match control.cast() {
                    Ok(c) => c,
                    Err(e) => {
                        trace!("cast to IAudioSessionControl2 failed: {:?}", e);
                        continue;
                    },
                };
                let pid = control2.GetProcessId().unwrap_or(0);
                if pid == 0 {
                    continue;
                }
                let volume: ISimpleAudioVolume = match control2.cast() {
                    Ok(v) => v,
                    Err(e) => {
                        trace!("cast to ISimpleAudioVolume failed: {:?}", e);
                        continue;
                    },
                };
                let process_name = process_image_name(pid)
                    .map(|p| {
                        p.file_name()
                            .map(|s| s.to_string_lossy().to_lowercase())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                out.push(SessionInfo {
                    pid,
                    process_name,
                    volume,
                    control: control2,
                });
            }
        }
        Ok(out)
    }
}

fn process_image_name(pid: u32) -> Result<PathBuf> {
    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid)
            .with_context(|| format!("OpenProcess({pid})"))?;
        if handle.is_invalid() {
            anyhow::bail!("OpenProcess({pid}) returned invalid handle");
        }
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        result.with_context(|| format!("QueryFullProcessImageNameW({pid})"))?;
        let name = String::from_utf16_lossy(&buffer[..size as usize]);
        Ok(PathBuf::from(name))
    }
}

/// Set scalar volume (0.0..=1.0) on a session interface.
pub fn set_session_volume(session: &SessionInfo, scalar: f32) -> Result<()> {
    unsafe {
        session
            .volume
            .SetMasterVolume(scalar.clamp(0.0, 1.0), std::ptr::null())
            .context("ISimpleAudioVolume::SetMasterVolume")?;
    }
    Ok(())
}

/// Toggle mute on a session.
pub fn toggle_session_mute(session: &SessionInfo) -> Result<()> {
    unsafe {
        let cur = session.volume.GetMute().context("GetMute")?;
        session
            .volume
            .SetMute(!cur.as_bool(), std::ptr::null())
            .context("SetMute")?;
        debug!(
            "Toggled mute for pid={} ({}) -> {}",
            session.pid,
            session.process_name,
            !cur.as_bool()
        );
    }
    Ok(())
}
