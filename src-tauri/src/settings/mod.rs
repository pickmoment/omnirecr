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
            // macOS Homebrew & MacPorts paths
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/opt/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
            // Windows common paths
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
                let probe = parent.join("ffprobe");
                let probe_exe = parent.join("ffprobe.exe");
                if probe.is_file() {
                    return Ok(probe);
                } else if probe_exe.is_file() {
                    return Ok(probe_exe);
                }
            }
        }

        if let Ok(output) = Command::new("ffprobe").arg("-version").output() {
            if output.status.success() {
                return Ok(PathBuf::from("ffprobe"));
            }
        }

        let candidates = [
            // macOS Homebrew & MacPorts paths
            "/opt/homebrew/bin/ffprobe",
            "/usr/local/bin/ffprobe",
            "/opt/local/bin/ffprobe",
            "/usr/bin/ffprobe",
            // Windows common paths
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

    /// Typecast 자동화용 Chrome 실행 파일을 찾는다.
    /// 사용자 지정 경로가 있으면 우선하고, 없으면 OS별 기본 설치 위치를 순서대로 확인한다.
    /// 시스템 PATH 는 뒤진다 — 사용자의 기본(개인 로그인 세션이 든) Chrome 프로필과
    /// 혼동되지 않도록 실행 파일 자체를 특정하는 쪽을 우선한다.
    pub fn find_chrome(custom_path: Option<&str>) -> Result<PathBuf, String> {
        if let Some(path_str) = custom_path {
            let trimmed = path_str.trim();
            if !trimmed.is_empty() {
                let path = PathBuf::from(trimmed);
                if path.is_file() {
                    return Ok(path);
                }
                return Err(format!(
                    "지정한 Chrome 경로를 찾을 수 없습니다: {}",
                    trimmed
                ));
            }
        }

        #[cfg(target_os = "macos")]
        let candidates: Vec<PathBuf> = {
            let mut list = vec![PathBuf::from(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            )];
            if let Some(home) = dirs::home_dir() {
                list.push(
                    home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
                );
            }
            list
        };

        #[cfg(target_os = "windows")]
        let candidates: Vec<PathBuf> = {
            let mut list = vec![];
            for env_var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
                if let Ok(base) = std::env::var(env_var) {
                    list.push(
                        PathBuf::from(base)
                            .join("Google")
                            .join("Chrome")
                            .join("Application")
                            .join("chrome.exe"),
                    );
                }
            }
            list
        };

        #[cfg(all(unix, not(target_os = "macos")))]
        let candidates: Vec<PathBuf> = vec![
            PathBuf::from("/usr/bin/google-chrome-stable"),
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/usr/bin/chromium-browser"),
            PathBuf::from("/usr/bin/chromium"),
            PathBuf::from("/snap/bin/chromium"),
        ];

        for candidate in &candidates {
            if candidate.is_file() {
                return Ok(candidate.clone());
            }
        }

        // 시스템 PATH 에 등록된 실행 파일 이름들을 마지막으로 시도한다.
        for name in ["google-chrome-stable", "google-chrome", "chromium-browser", "chromium"] {
            if let Ok(output) = Command::new(name).arg("--version").output() {
                if output.status.success() {
                    return Ok(PathBuf::from(name));
                }
            }
        }

        Err(
            "Google Chrome 을 찾을 수 없습니다. Chrome 을 설치하거나 설정에서 실행 파일 경로를 지정하세요."
                .to_string(),
        )
    }

    /// Typecast 자동화 전용 Chrome 프로필 디렉터리.
    /// 사용자의 개인 Chrome 프로필과 절대 공유하지 않는다 — 로그인 세션이 이 안에서만 유지된다.
    pub fn typecast_chrome_profile_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = home.join(".omnirec").join("typecast-chrome-profile");
        let _ = fs::create_dir_all(&dir);
        dir
    }
}
