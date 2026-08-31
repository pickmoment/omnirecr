use crate::types::Settings;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// 선두의 독립 `~` 를 홈 디렉터리로 확장한다.
/// Rust 는 셸이 아니라서 틸데를 확장하지 않는다 — 설정에 `~/Videos/OmniRec` 를 적으면
/// 작업 디렉터리 아래에 `~` 라는 이름의 폴더가 생기고 녹음 파일이 거기로 사라진다.
/// `~user`(다른 사용자 홈) 형태는 OS마다 규칙이 달라 일부러 건드리지 않는다.
pub(crate) fn expand_tilde(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed != "~" && !trimmed.starts_with("~/") && !trimmed.starts_with("~\\") {
        return trimmed.to_string();
    }
    let Some(home) = dirs::home_dir() else {
        return trimmed.to_string();
    };
    if trimmed == "~" {
        return home.to_string_lossy().to_string();
    }
    home.join(&trimmed[2..]).to_string_lossy().to_string()
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 파일을 원자적으로 교체한다: 같은 디렉터리의 임시 파일에 쓰고 `sync_all()` 한 뒤 rename.
/// `fs::write` 는 대상 파일을 **먼저 0바이트로 자르기** 때문에, 쓰는 중 프로세스가 죽으면
/// 설정/대본이 절반만 남아 파싱 불가가 된다(= 사용자 데이터 소실).
/// rename 은 같은 파일시스템 안에서 원자적이므로 "옛 내용" 아니면 "새 내용" 하나만 남는다.
/// 되돌리면 크래시·전원 차단 한 번에 대본 라이브러리가 통째로 날아간다.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&dir).map_err(|e| format!("폴더 생성 실패 ({}): {}", dir.display(), e))?;

    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "store.json".to_string());
    // 임시 파일은 반드시 같은 디렉터리에 둔다 — 다른 파일시스템(예: /tmp)으로 가면
    // rename 이 EXDEV 로 실패하고 원자성도 잃는다.
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        base,
        std::process::id(),
        unique_suffix()
    ));

    let written = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        // fsync 없이 rename 하면 메타데이터만 먼저 반영되어 빈 파일이 남을 수 있다.
        file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = written {
        cleanup_temp(&tmp);
        return Err(format!("임시 파일 쓰기 실패 ({}): {}", tmp.display(), e));
    }

    if let Err(e) = fs::rename(&tmp, path) {
        cleanup_temp(&tmp);
        return Err(format!(
            "파일 교체 실패 ({} -> {}): {}",
            tmp.display(),
            path.display(),
            e
        ));
    }

    Ok(())
}

fn cleanup_temp(tmp: &Path) {
    if let Err(e) = fs::remove_file(tmp) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("임시 파일 정리 실패 ({}): {}", tmp.display(), e);
        }
    }
}

/// 해석할 수 없는 파일을 `<이름>.bad-<UTC타임스탬프>` 로 옮겨 보존한다.
/// 지우거나 덮어쓰지 않는 것이 핵심 — 사용자가 손으로 복구할 수 있어야 한다.
pub(crate) fn quarantine_corrupt_file(path: &Path) -> Result<PathBuf, String> {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "store.json".to_string());
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    let mut target = path.with_file_name(format!("{}.bad-{}", base, stamp));
    let mut nth = 2;
    // 같은 초에 두 번 손상되어도 이전 보존본을 덮어쓰지 않는다(rename 은 조용히 덮어쓴다).
    while target.exists() {
        target = path.with_file_name(format!("{}.bad-{}-{}", base, stamp, nth));
        nth += 1;
    }

    fs::rename(path, &target).map_err(|e| {
        format!(
            "손상 파일 보존 실패 ({} -> {}): {}",
            path.display(),
            target.display(),
            e
        )
    })?;
    Ok(target)
}

pub struct SettingsManager;

impl SettingsManager {
    pub fn get_config_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let omni_dir = home.join(".omnirec");
        if !omni_dir.exists() {
            if let Err(e) = fs::create_dir_all(&omni_dir) {
                log::error!("설정 폴더 생성 실패 ({}): {}", omni_dir.display(), e);
            }
        }
        omni_dir.join("settings.json")
    }

    pub fn load() -> Settings {
        let settings = Self::load_from(&Self::get_config_path());
        if let Err(e) = fs::create_dir_all(&settings.output_dir) {
            log::warn!("출력 폴더 생성 실패 ({}): {}", settings.output_dir, e);
        }
        settings
    }

    /// 실제 구현. 경로를 인자로 받아 테스트에서 임시 디렉터리를 쓸 수 있게 한다.
    ///
    /// 지키는 규칙: **파일이 없을 때만** 기본값을 디스크에 쓴다.
    /// 예전 구현은 읽기/파싱 실패를 모두 "기본값"으로 접고 그 기본값을 곧바로 유일한
    /// 사본 위에 덮어썼다 — JSON 오타 한 글자, 일시적 권한 오류 한 번에 사용자의 모든
    /// 설정이 영구히 사라졌다. 되돌리면 그 사고가 그대로 돌아온다.
    ///
    /// 계약 C3: `Settings` 는 컨테이너 수준 `#[serde(default)]` 를 가지므로 필드가 빠져도
    /// 그 필드만 기본값이 되고 전체 파싱이 실패하지 않는다 → 필드 추가/삭제 릴리스에서
    /// 사용자 설정이 초기화되지 않는다.
    fn load_from(path: &Path) -> Settings {
        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<Settings>(&content) {
                Ok(mut settings) => {
                    Self::normalize_paths(&mut settings);
                    settings
                }
                Err(parse_err) => {
                    match quarantine_corrupt_file(path) {
                        Ok(saved) => log::error!(
                            "설정 파일을 해석할 수 없어 {} 로 보존하고 이번 실행만 기본값을 쓴다: {}",
                            saved.display(),
                            parse_err
                        ),
                        Err(rename_err) => log::error!(
                            "설정 파일 해석 실패({}) — 원본 보존까지 실패해 파일을 그대로 남긴다: {}",
                            parse_err,
                            rename_err
                        ),
                    }
                    // 사용자가 실제로 저장할 때까지 새 settings.json 을 만들지 않는다.
                    Self::defaults_without_writing()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 첫 실행. 이때만 기본값을 파일로 남긴다.
                let defaults = Self::defaults_without_writing();
                if let Err(save_err) = Self::save_to(path, &defaults) {
                    log::error!(
                        "최초 설정 파일 생성 실패 ({}): {}",
                        path.display(),
                        save_err
                    );
                }
                defaults
            }
            Err(e) => {
                // 권한 오류 등. 파일은 남아 있으니 **절대** 덮어쓰지 않는다.
                log::error!(
                    "설정 파일을 읽을 수 없어 이번 실행만 기본값을 쓴다 ({}): {}",
                    path.display(),
                    e
                );
                Self::defaults_without_writing()
            }
        }
    }

    fn defaults_without_writing() -> Settings {
        let mut settings = Settings::default();
        Self::normalize_paths(&mut settings);
        settings
    }

    pub fn save(settings: &Settings) -> Result<(), String> {
        Self::save_to(&Self::get_config_path(), settings)?;
        let output_dir = expand_tilde(&settings.output_dir);
        if let Err(e) = fs::create_dir_all(&output_dir) {
            log::warn!("출력 폴더 생성 실패 ({}): {}", output_dir, e);
        }
        Ok(())
    }

    fn save_to(path: &Path, settings: &Settings) -> Result<(), String> {
        // 디스크에도 확장된 절대 경로를 남긴다 — 저장된 값과 실제 사용되는 값이 달라지면
        // "설정에는 이 폴더인데 파일은 다른 데 있다" 류의 재현 불가 버그가 된다.
        let mut normalized = settings.clone();
        Self::normalize_paths(&mut normalized);
        let json = serde_json::to_string_pretty(&normalized)
            .map_err(|e| format!("설정 직렬화 실패: {}", e))?;
        write_atomic(path, &json)
    }

    /// 경로 성격의 설정값을 `SettingsManager` 경계에서 한 번에 정규화한다.
    /// 소비하는 쪽(recorder/screen/audio, converter, tts …)이 각자 틸데를 신경 쓰지 않아도 되게.
    fn normalize_paths(settings: &mut Settings) {
        settings.output_dir = expand_tilde(&settings.output_dir);
        settings.custom_ffmpeg_path = normalize_optional_path(settings.custom_ffmpeg_path.take());
        settings.custom_chrome_path = normalize_optional_path(settings.custom_chrome_path.take());
    }

    pub fn find_ffmpeg(custom_path: Option<&str>) -> Result<PathBuf, String> {
        // 프론트엔드가 커맨드 인자로 직접 넘긴 값도 여기로 들어온다(설정 화면의
        // "FFmpeg 경로 확인"). 그래서 로드 경계뿐 아니라 이 진입점에서도 틸데를 확장한다.
        if let Some(expanded) = normalize_optional_path(custom_path.map(|p| p.to_string())) {
            let path = PathBuf::from(&expanded);
            if path.is_file() {
                return Ok(path);
            }
            // 지정 경로가 잘못됐는데 시스템 FFmpeg 로 조용히 넘어가면 사용자는
            // 자기 경로가 쓰이고 있다고 착각한다 → 최소한 로그로 남긴다.
            log::warn!(
                "지정한 FFmpeg 경로를 쓸 수 없어 자동 탐색으로 넘어간다: {}",
                expanded
            );
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
            let winget_path = local_app_data
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("ffmpeg.exe");
            if winget_path.is_file() {
                return Ok(winget_path);
            }
        }

        if let Some(home) = dirs::home_dir() {
            let scoop_path = home
                .join("scoop")
                .join("apps")
                .join("ffmpeg")
                .join("current")
                .join("bin")
                .join("ffmpeg.exe");
            if scoop_path.is_file() {
                return Ok(scoop_path);
            }
            let scoop_shims = home.join("scoop").join("shims").join("ffmpeg.exe");
            if scoop_shims.is_file() {
                return Ok(scoop_shims);
            }
        }

        Err(
            "FFmpeg executable not found. Please install FFmpeg or set custom path in Settings."
                .to_string(),
        )
    }

    pub fn find_ffprobe(custom_ffmpeg_path: Option<&str>) -> Result<PathBuf, String> {
        if let Some(path_str) = normalize_optional_path(custom_ffmpeg_path.map(|p| p.to_string())) {
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
            let winget_path = local_app_data
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("ffprobe.exe");
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
        if let Some(expanded) = normalize_optional_path(custom_path.map(|p| p.to_string())) {
            let path = PathBuf::from(&expanded);
            if path.is_file() {
                return Ok(path);
            }
            return Err(format!(
                "지정한 Chrome 경로를 찾을 수 없습니다: {}",
                expanded
            ));
        }

        #[cfg(target_os = "macos")]
        let candidates: Vec<PathBuf> = {
            let mut list = vec![PathBuf::from(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            )];
            if let Some(home) = dirs::home_dir() {
                list.push(home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
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
        for name in [
            "google-chrome-stable",
            "google-chrome",
            "chromium-browser",
            "chromium",
        ] {
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
        if let Err(e) = fs::create_dir_all(&dir) {
            log::error!("Chrome 프로필 폴더 생성 실패 ({}): {}", dir.display(), e);
        }
        dir
    }
}

/// 빈 문자열은 `None` 으로 접는다 — `Some("")` 은 "사용자 지정 경로 있음"으로 오해되어
/// 탐색 로직을 헛돌게 한다.
fn normalize_optional_path(raw: Option<String>) -> Option<String> {
    raw.map(|p| expand_tilde(&p)).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omnirec-settings-test-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&dir).expect("임시 폴더 생성");
        dir
    }

    #[test]
    fn expands_leading_tilde_only() {
        let home = dirs::home_dir().expect("홈 디렉터리");
        assert_eq!(
            expand_tilde("~/Videos/OmniRec"),
            home.join("Videos/OmniRec").to_string_lossy().to_string()
        );
        assert_eq!(expand_tilde("~"), home.to_string_lossy().to_string());
        // 공백은 다듬고, 중간의 물결표나 `~user` 형태는 그대로 둔다.
        assert_eq!(expand_tilde("  /tmp/omnirec  "), "/tmp/omnirec");
        assert_eq!(expand_tilde("/tmp/~/x"), "/tmp/~/x");
        assert_eq!(expand_tilde("~someone/x"), "~someone/x");
        assert_eq!(expand_tilde(""), "");
    }

    #[test]
    fn normalizes_tilde_in_settings_paths_at_the_manager_boundary() {
        let home = dirs::home_dir().expect("홈 디렉터리");
        let mut settings = Settings::default();
        settings.output_dir = "~/Videos/OmniRec".to_string();
        settings.custom_ffmpeg_path = Some("~/bin/ffmpeg".to_string());
        settings.custom_chrome_path = Some("   ".to_string());

        SettingsManager::normalize_paths(&mut settings);

        assert_eq!(
            settings.output_dir,
            home.join("Videos/OmniRec").to_string_lossy().to_string()
        );
        assert_eq!(
            settings.custom_ffmpeg_path.as_deref(),
            Some(
                home.join("bin/ffmpeg")
                    .to_string_lossy()
                    .to_string()
                    .as_str()
            )
        );
        assert_eq!(settings.custom_chrome_path, None);
    }

    #[test]
    fn missing_fields_keep_the_rest_of_the_file() {
        // 계약 C3: 컨테이너 수준 `#[serde(default)]` 덕분에 필드가 빠져도 그 필드만 기본값이 된다.
        let dir = temp_dir("partial");
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{"output_dir":"/tmp/omnirec-partial","audio_bitrate":320}"#,
        )
        .expect("부분 설정 기록");

        let loaded = SettingsManager::load_from(&path);

        assert_eq!(loaded.output_dir, "/tmp/omnirec-partial");
        assert_eq!(loaded.audio_bitrate, 320);
        // 명시되지 않은 값은 기본값을 따른다.
        assert_eq!(loaded.audio_format, Settings::default().audio_format);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_settings_are_preserved_and_never_overwritten() {
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.json");
        let broken = r#"{"output_dir": "/tmp/omnirec-broken",,,"#;
        fs::write(&path, broken).expect("손상 설정 기록");

        let loaded = SettingsManager::load_from(&path);

        // 기본값을 돌려주되 파일은 새로 만들지 않는다.
        assert_eq!(loaded.audio_format, Settings::default().audio_format);
        assert!(
            !path.exists(),
            "손상 파일은 옮겨져야 하고 같은 자리에 새 파일이 생기면 안 된다"
        );

        let preserved: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("폴더 읽기")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains("settings.json.bad-"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(preserved.len(), 1, "보존본이 정확히 하나 있어야 한다");
        assert_eq!(
            fs::read_to_string(&preserved[0]).expect("보존본 읽기"),
            broken,
            "보존본은 원본 내용을 그대로 유지해야 한다"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn first_run_creates_the_file_and_round_trips() {
        let dir = temp_dir("first-run");
        let path = dir.join("settings.json");

        let created = SettingsManager::load_from(&path);
        assert!(path.exists(), "파일이 없을 때만 기본값을 새로 쓴다");

        let mut changed = created.clone();
        changed.audio_bitrate = 192;
        SettingsManager::save_to(&path, &changed).expect("원자적 저장");

        let reloaded = SettingsManager::load_from(&path);
        assert_eq!(reloaded.audio_bitrate, 192);
        // 임시 파일이 남아 있으면 원자 교체가 새는 것이다.
        let leftovers = fs::read_dir(&dir)
            .expect("폴더 읽기")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);

        fs::remove_dir_all(&dir).ok();
    }
}
