use anyhow::{Context, Result};
use std::ptr;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::core::Interface;

/// Handle to a specific process's audio session, allowing volume and mute
/// control via the Windows Audio Session API (WASAPI).
pub struct AudioSession {
    volume: ISimpleAudioVolume,
}

impl AudioSession {
    /// Search all audio sessions on the default output device for one owned
    /// by `pid`. Returns `None` if the process has no active audio session
    /// (e.g. DOSBox hasn't started producing sound yet).
    pub fn find_for_pid(pid: u32) -> Result<Option<Self>> {
        find_session_for_pid(pid)
    }

    pub fn get_volume(&self) -> Result<f32> {
        unsafe {
            self.volume
                .GetMasterVolume()
                .context("GetMasterVolume failed")
        }
    }

    pub fn set_volume(&self, level: f32) -> Result<()> {
        let clamped = level.clamp(0.0, 1.0);
        unsafe {
            self.volume
                .SetMasterVolume(clamped, ptr::null())
                .context("SetMasterVolume failed")
        }
    }

    pub fn get_mute(&self) -> Result<bool> {
        unsafe {
            self.volume
                .GetMute()
                .map(|b| b.as_bool())
                .context("GetMute failed")
        }
    }

    pub fn set_mute(&self, muted: bool) -> Result<()> {
        unsafe {
            self.volume
                .SetMute(muted, ptr::null())
                .context("SetMute failed")
        }
    }
}

/// Initialize COM for the current thread (apartment-threaded, matching egui's
/// single-thread model). Call once from `main()` before the event loop.
pub fn init_com() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("COM initialization failed")?;
    }
    Ok(())
}

fn find_session_for_pid(target_pid: u32) -> Result<Option<AudioSession>> {
    unsafe {
        // Get the default audio output device.
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("Failed to create IMMDeviceEnumerator")?;

        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .context("Failed to get default audio endpoint")?;

        // Activate the session manager on this device.
        let manager: IAudioSessionManager2 = device
            .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
            .context("Failed to activate IAudioSessionManager2")?;

        let session_list = manager
            .GetSessionEnumerator()
            .context("Failed to get session enumerator")?;

        let count = session_list
            .GetCount()
            .context("Failed to get session count")?;

        for i in 0..count {
            let control = match session_list.GetSession(i) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // QI for IAudioSessionControl2 to read the process ID.
            let control2: IAudioSessionControl2 = match control.cast() {
                Ok(c) => c,
                Err(_) => continue,
            };

            let session_pid = match control2.GetProcessId() {
                Ok(pid) => pid,
                Err(_) => continue,
            };

            if session_pid == target_pid {
                let volume: ISimpleAudioVolume = control
                    .cast()
                    .context("Audio session found but ISimpleAudioVolume query failed")?;
                return Ok(Some(AudioSession { volume }));
            }
        }

        // No session found for this PID (DOSBox may not be producing audio yet).
        Ok(None)
    }
}
