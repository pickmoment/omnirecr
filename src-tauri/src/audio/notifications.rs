use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::Win32::Foundation::S_OK;

pub struct NotificationSoundManager {
    is_muted: Arc<AtomicBool>,
}

impl NotificationSoundManager {
    pub fn new() -> Self {
        Self {
            is_muted: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(target_os = "windows")]
    pub fn mute_system_notifications(&self) {
        if self.is_muted.load(Ordering::SeqCst) {
            return;
        }

        unsafe {
            let _ = Self::set_system_sounds_mute(true);
        }
        self.is_muted.store(true, Ordering::SeqCst);
    }

    #[cfg(target_os = "windows")]
    pub fn restore_system_notifications(&self) {
        if !self.is_muted.load(Ordering::SeqCst) {
            return;
        }

        unsafe {
            let _ = Self::set_system_sounds_mute(false);
        }
        self.is_muted.store(false, Ordering::SeqCst);
    }

    #[cfg(not(target_os = "windows"))]
    pub fn mute_system_notifications(&self) {}

    #[cfg(not(target_os = "windows"))]
    pub fn restore_system_notifications(&self) {}

    #[cfg(target_os = "windows")]
    unsafe fn set_system_sounds_mute(mute: bool) -> Result<(), Box<dyn std::error::Error>> {
        use windows::core::Interface;
        use windows::Win32::Media::Audio::*;
        use windows::Win32::System::Com::*;

        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let device_enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let default_device =
            device_enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;

        let session_manager: IAudioSessionManager2 =
            default_device.Activate(CLSCTX_ALL, None)?;

        let session_enum = session_manager.GetSessionEnumerator()?;
        let count = session_enum.GetCount()?;

        for i in 0..count {
            if let Ok(session_control) = session_enum.GetSession(i) {
                if let Ok(session_control2) = session_control.cast::<IAudioSessionControl2>() {
                    let is_system_sound = session_control2.IsSystemSoundsSession() == S_OK;
                    
                    if is_system_sound {
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
        self.restore_system_notifications();
    }
}
