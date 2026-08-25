use crate::types::Settings;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct SettingsManager;

impl SettingsManager {
    pub fn get_config_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let omni_dir = home.join(".omnirec");
        if !omni_dir.exists() {
            let _ = fs::create_dir_all(&omni_dir);
        }
        omni_dir.join("settings.json")
    }

    pub fn load() -> Settings {
        let path = Self::get_config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str::<Settings>(&content) {
                    let _ = fs::create_dir_all(&settings.output_dir);
                    return settings;
                }
            }
        }

        let default_settings = Settings::default();
        let _ = Self::save(&default_settings);
        let _ = fs::create_dir_all(&default_settings.output_dir);
        default_settings
    }

    pub fn save(settings: &Settings) -> Result<(), String> {
        let path = Self::get_config_path();
        let json = serde_json::to_string_pretty(settings)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        fs::write(&path, json).map_err(|e| format!("Failed to write settings file: {}", e))?;
        let _ = fs::create_dir_all(&settings.output_dir);
        Ok(())
    }

    pub fn find_ffmpeg(custom_path: Option<&str>) -> Result<PathBuf, String> {
        if let Some(path_str) = custom_path {
            let path = PathBuf::from(path_str);
            if path.is_file() {
                return Ok(path);
            }
        }

        // 1. Check if ffmpeg is in PATH
        if let Ok(output) = Command::new("ffmpeg").arg("-version").output() {
            if output.status.success() {
                return Ok(PathBuf::from("ffmpeg"));
            }
        }

        // 2. Common Windows paths
        let candidates = [
            r"C:\Program Files\DownloadHelper CoApp\ffmpeg.exe",
            r"C:\Program Files\ffmpeg\bin\ffmpeg.exe",
            r"C:\ffmpeg\bin\ffmpeg.exe",
            r"C:\tools\ffmpeg\bin\ffmpeg.exe",
            r"C:\ProgramData\chocolatey\bin\ffmpeg.exe",
        ];

        for &candidate in &candidates {
            let p = Path::new(candidate);
            if p.is_file() {
                return Ok(p.to_path_buf());
            }
        }

        // 3. User local appdata / scoop / winget
        if let Some(local_app_data) = dirs::data_local_dir() {
            let winget_path = local_app_data.join("Microsoft").join("WinGet").join("Links").join("ffmpeg.exe");
            if winget_path.is_file() {
                return Ok(winget_path);
            }
        }

        if let Some(home) = dirs::home_dir() {
            let scoop_path = home.join("scoop").join("apps").join("ffmpeg").join("current").join("bin").join("ffmpeg.exe");
            if scoop_path.is_file() {
                return Ok(scoop_path);
            }
            let scoop_shims = home.join("scoop").join("shims").join("ffmpeg.exe");
            if scoop_shims.is_file() {
                return Ok(scoop_shims);
            }
        }

        Err("FFmpeg executable not found. Please install FFmpeg or set custom path in Settings.".to_string())
    }

    pub fn find_ffprobe(custom_ffmpeg_path: Option<&str>) -> Result<PathBuf, String> {
        if let Some(path_str) = custom_ffmpeg_path {
            let ffmpeg_path = PathBuf::from(path_str);
            if let Some(parent) = ffmpeg_path.parent() {
                let probe = parent.join("ffprobe.exe");
                if probe.is_file() {
                    return Ok(probe);
                }
            }
        }

        if let Ok(output) = Command::new("ffprobe").arg("-version").output() {
            if output.status.success() {
                return Ok(PathBuf::from("ffprobe"));
            }
        }

        let candidates = [
            r"C:\Program Files\DownloadHelper CoApp\ffprobe.exe",
            r"C:\Program Files\ffmpeg\bin\ffprobe.exe",
            r"C:\ffmpeg\bin\ffprobe.exe",
            r"C:\tools\ffmpeg\bin\ffprobe.exe",
            r"C:\ProgramData\chocolatey\bin\ffprobe.exe",
        ];

        for &candidate in &candidates {
            let p = Path::new(candidate);
            if p.is_file() {
                return Ok(p.to_path_buf());
            }
        }

        if let Some(local_app_data) = dirs::data_local_dir() {
            let winget_path = local_app_data.join("Microsoft").join("WinGet").join("Links").join("ffprobe.exe");
            if winget_path.is_file() {
                return Ok(winget_path);
            }
        }

        Err("FFprobe executable not found.".to_string())
    }
}
