use crate::types::Settings;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::S_OK;

pub struct NotificationSoundManager {
    is_muted: Arc<AtomicBool>,
    #[allow(dead_code)]
    macos_shortcut_start: String,
    #[allow(dead_code)]
    macos_shortcut_stop: String,
}

impl NotificationSoundManager {
    pub fn new(settings: &Settings) -> Self {
        Self {
            is_muted: Arc::new(AtomicBool::new(false)),
            macos_shortcut_start: settings.macos_shortcut_start.clone(),
            macos_shortcut_stop: settings.macos_shortcut_stop.clone(),
        }
    }

    pub fn mute_system_notifications(&self) -> Result<(), String> {
        if self.is_muted.load(Ordering::SeqCst) {
            return Ok(());
        }

        #[cfg(target_os = "windows")]
        unsafe {
            Self::set_system_sounds_mute(true).map_err(|e| e.to_string())?;
        }

        #[cfg(target_os = "macos")]
        Self::run_macos_shortcut(&self.macos_shortcut_start)?;

        self.is_muted.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn restore_system_notifications(&self) -> Result<(), String> {
        if !self.is_muted.load(Ordering::SeqCst) {
            return Ok(());
        }

        #[cfg(target_os = "windows")]
        unsafe {
            Self::set_system_sounds_mute(false).map_err(|e| e.to_string())?;
        }

        #[cfg(target_os = "macos")]
        Self::run_macos_shortcut(&self.macos_shortcut_stop)?;

        self.is_muted.store(false, Ordering::SeqCst);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn run_macos_shortcut(shortcut_name: &str) -> Result<(), String> {
        use std::process::Command;

        let shortcut_name = shortcut_name.trim();
        if shortcut_name.is_empty() {
            return Err("macOS 단축어 이름이 비어 있습니다.".to_string());
        }

        let output = Command::new("/usr/bin/shortcuts")
            .args(["run", shortcut_name])
            .output()
            .map_err(|e| format!("단축어 실행 실패: {e}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                Err(format!("단축어 실행 실패: {}", output.status))
            } else {
                Err(format!("단축어 실행 실패: {stderr}"))
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn run_macos_shortcut(_shortcut_name: &str) -> Result<(), String> {
        Err("macOS에서만 단축어를 실행할 수 있습니다.".to_string())
    }

    #[cfg(target_os = "windows")]
    unsafe fn set_system_sounds_mute(mute: bool) -> Result<(), Box<dyn std::error::Error>> {
        use windows::core::Interface;
        use windows::Win32::Media::Audio::*;
        use windows::Win32::System::Com::*;

        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let device_enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let default_device = device_enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;

        let session_manager: IAudioSessionManager2 = default_device.Activate(CLSCTX_ALL, None)?;

        let session_enum = session_manager.GetSessionEnumerator()?;
        let count = session_enum.GetCount()?;

        for i in 0..count {
            if let Ok(session_control) = session_enum.GetSession(i) {
                if let Ok(session_control2) = session_control.cast::<IAudioSessionControl2>() {
                    if session_control2.IsSystemSoundsSession() == S_OK {
                        if let Ok(simple_volume) = session_control.cast::<ISimpleAudioVolume>() {
                            let _ = simple_volume.SetMute(mute, std::ptr::null());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl Drop for NotificationSoundManager {
    fn drop(&mut self) {
        let _ = self.restore_system_notifications();
    }
}
