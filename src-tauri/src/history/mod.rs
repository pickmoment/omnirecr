use chrono::{DateTime, Local};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use crate::merger::MergerController;
use crate::types::HistoryItem;

/// ffprobe 결과 캐시의 키.
///
/// **경로만 키로 쓰면 안 된다.** 파일명을 바꾸거나 같은 이름으로 다시 녹음하면 옛 길이·
/// 해상도가 그대로 표시되고, 사용자는 목록의 숫자를 믿을 수 없게 된다. 크기와 수정 시각을
/// 키에 넣어 파일이 조금이라도 바뀌면 자동으로 캐시 미스가 되게 한다.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ProbeKey {
    path: String,
    size_bytes: u64,
    mtime_nanos: i128,
}

/// 목록 표시에 필요한 만큼만 캐시한다.
#[derive(Clone)]
struct CachedProbe {
    duration_secs: f64,
    width: Option<u32>,
    height: Option<u32>,
}

/// 캐시 상한. 다른(예전) 출력 폴더 항목까지 무한히 쌓이는 것을 막는다.
const PROBE_CACHE_MAX: usize = 512;

static PROBE_CACHE: LazyLock<Mutex<HashMap<ProbeKey, CachedProbe>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_cache() -> MutexGuard<'static, HashMap<ProbeKey, CachedProbe>> {
    // 캐시는 순수 파생 데이터다 — 패닉으로 오염됐다고 기능을 죽일 이유가 없다.
    PROBE_CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 파일이 사라졌거나 이름이 바뀌었을 때 그 경로의 캐시 항목을 즉시 버린다.
fn invalidate_probe_cache(path: &str) {
    let mut cache = lock_cache();
    cache.retain(|k, _| k.path != path);
}

/// 수정 시각을 캐시 키로 쓸 수 있는 정수로 바꾼다(에포크 이전이면 음수).
fn mtime_nanos(metadata: &fs::Metadata) -> i128 {
    match metadata.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        },
        Err(_) => 0,
    }
}

/// 두 경로가 같은 파일 실체를 가리키는지 판정한다.
/// 대소문자 무시 파일시스템에서 "이름만 대문자로 바꾸기"와 "다른 파일 덮어쓰기"를
/// 구별하는 유일한 근거다 — 잘못 판정하면 남의 녹음을 조용히 덮어쓴다.
#[cfg(unix)]
fn is_same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// Windows 는 기본이 대소문자 무시 파일시스템이라 같은 폴더에서 이름의 대소문자만
/// 다르면 같은 파일이다(inode 개념이 없어 안정적으로 비교할 수단이 마땅치 않다).
#[cfg(windows)]
fn is_same_file(a: &Path, b: &Path) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    a.parent() == b.parent() && name(a) == name(b) && name(a).is_some()
}

/// 한 번의 `read_dir` 로 얻은 파일 정보. 같은 파일을 두 번 stat 하지 않기 위해 모아둔다.
struct ScannedFile {
    file_name: String,
    ext: String,
    size_bytes: u64,
    created_at: String,
    key: ProbeKey,
}

pub struct HistoryManager;

impl HistoryManager {
    pub fn list_files(output_dir: &str, custom_ffmpeg_path: Option<String>) -> Vec<HistoryItem> {
        let dir = Path::new(output_dir);
        if !dir.exists() || !dir.is_dir() {
            return Vec::new();
        }

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("출력 폴더를 읽을 수 없다 ({}): {}", dir.display(), e);
                return Vec::new();
            }
        };

        let mut scanned: Vec<ScannedFile> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(ext) => ext.to_lowercase(),
                None => continue,
            };
            if !["mp3", "m4a", "wav", "mp4", "mov", "mkv", "webm"].contains(&ext.as_str()) {
                continue;
            }

            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("파일 정보를 읽을 수 없다 ({}): {}", path.display(), e);
                    continue;
                }
            };

            let created_at = metadata
                .created()
                .or_else(|_| metadata.modified())
                .map(|t| {
                    let dt: DateTime<Local> = t.into();
                    dt.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_else(|_| "Unknown".to_string());

            scanned.push(ScannedFile {
                file_name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                ext,
                size_bytes: metadata.len(),
                created_at,
                key: ProbeKey {
                    path: path.to_string_lossy().to_string(),
                    size_bytes: metadata.len(),
                    mtime_nanos: mtime_nanos(&metadata),
                },
            });
        }

        let probes = Self::probe_with_cache(&scanned, custom_ffmpeg_path);

        let mut items = Vec::with_capacity(scanned.len());
        for file in &scanned {
            let is_video = ["mp4", "mov", "mkv", "webm"].contains(&file.ext.as_str());
            let probe = probes.get(&file.key.path);
            let duration_secs = probe.map(|p| p.duration_secs).unwrap_or(0.0);
            let resolution = probe.and_then(|p| match (p.width, p.height) {
                (Some(w), Some(h)) => Some(format!("{}x{}", w, h)),
                _ => None,
            });

            items.push(HistoryItem {
                id: file.key.path.clone(),
                file_name: file.file_name.clone(),
                file_path: file.key.path.clone(),
                file_type: if is_video {
                    "video".to_string()
                } else {
                    "audio".to_string()
                },
                format: file.ext.clone(),
                size_bytes: file.size_bytes,
                size_formatted: format_file_size(file.size_bytes),
                duration_secs,
                duration_formatted: format_duration(duration_secs),
                created_at: file.created_at.clone(),
                resolution,
            });
        }

        // Sort descending by creation date, then by file name
        items.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.file_name.cmp(&a.file_name))
        });
        items
    }

    /// 캐시에 없는 파일만 ffprobe 로 확인한다. 반환값은 경로 → 메타데이터.
    fn probe_with_cache(
        scanned: &[ScannedFile],
        custom_ffmpeg_path: Option<String>,
    ) -> HashMap<String, CachedProbe> {
        let mut resolved: HashMap<String, CachedProbe> = HashMap::new();
        let mut misses: Vec<String> = Vec::new();

        {
            let cache = lock_cache();
            for file in scanned {
                match cache.get(&file.key) {
                    Some(hit) => {
                        resolved.insert(file.key.path.clone(), hit.clone());
                    }
                    None => misses.push(file.key.path.clone()),
                }
            }
        }

        if !misses.is_empty() {
            match MergerController::probe_files(misses, custom_ffmpeg_path) {
                Ok(infos) => {
                    let key_by_path: HashMap<&str, &ProbeKey> = scanned
                        .iter()
                        .map(|f| (f.key.path.as_str(), &f.key))
                        .collect();
                    let mut cache = lock_cache();
                    for info in infos {
                        let cached = CachedProbe {
                            duration_secs: info.duration_secs,
                            width: info.width,
                            height: info.height,
                        };
                        if let Some(key) = key_by_path.get(info.path.as_str()) {
                            cache.insert(ProbeKey::clone(key), cached.clone());
                        }
                        resolved.insert(info.path, cached);
                    }
                }
                Err(e) => {
                    // ffprobe 가 없어도 목록 자체는 보여준다. 다만 조용히 넘기지 않는다 —
                    // 모든 길이가 00:00 으로 보이는 이유를 로그에서 찾을 수 있어야 한다.
                    log::warn!(
                        "미디어 정보 분석 실패 — 길이·해상도 없이 목록만 표시한다: {}",
                        e
                    );
                }
            }
        }

        Self::prune_cache(scanned);
        resolved
    }

    fn prune_cache(scanned: &[ScannedFile]) {
        let current_keys: HashSet<ProbeKey> = scanned.iter().map(|f| f.key.clone()).collect();
        let scanned_paths: HashSet<String> = scanned.iter().map(|f| f.key.path.clone()).collect();

        let mut cache = lock_cache();
        let over_cap = cache.len() > PROBE_CACHE_MAX;
        cache.retain(|key, _| {
            if current_keys.contains(key) {
                return true;
            }
            if scanned_paths.contains(&key.path) {
                // 같은 경로인데 크기·수정 시각이 다르다 = 파일이 바뀌었다. 옛 세대는 버린다.
                return false;
            }
            // 다른 폴더의 항목은 캐시가 상한을 넘길 때만 정리한다.
            !over_cap
        });
    }

    pub fn delete_file(path_str: &str) -> Result<(), String> {
        // 없는 파일을 성공으로 보고하면 프론트엔드는 목록에서 지우지만 실제 파일은
        // (경로 오타·이미 이동됨 등으로) 어딘가 그대로 남는다. 실패를 그대로 알린다.
        fs::remove_file(Path::new(path_str)).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                format!("파일을 찾을 수 없습니다: {}", path_str)
            }
            _ => format!("파일 삭제 실패 ({}): {}", path_str, e),
        })?;
        invalidate_probe_cache(path_str);
        Ok(())
    }

    pub fn rename_file(old_path_str: &str, new_name_str: &str) -> Result<String, String> {
        let old_path = Path::new(old_path_str);
        if !old_path.is_file() {
            return Err("원본 파일을 찾을 수 없습니다.".to_string());
        }

        let parent_dir = old_path.parent().ok_or("상위 폴더를 찾을 수 없습니다.")?;
        let old_ext = old_path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let new_name_clean = new_name_str.trim();
        if new_name_clean.is_empty() {
            return Err("새 파일명을 입력해 주세요.".to_string());
        }

        // 경로 구분자·드라이브 문자·제어문자는 전부 막는다. 하나라도 통과하면
        // "파일명 변경"이 출력 폴더 밖 임의 경로로의 이동이 된다.
        let invalid_chars = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        if new_name_clean
            .chars()
            .any(|c| invalid_chars.contains(&c) || c.is_control())
        {
            return Err("파일명에 사용할 수 없는 특수문자(\\ / : * ? \" < > |)나 제어문자가 포함되어 있습니다.".to_string());
        }
        // `.` / `..` 은 구분자가 없어도 자기 자신·상위 폴더를 가리킨다.
        // 그대로 rename 하면 폴더를 덮치는 사고가 된다.
        if new_name_clean.chars().all(|c| c == '.') {
            return Err("파일명이 올바르지 않습니다.".to_string());
        }

        // 확장자를 잃으면 히스토리 목록의 확장자 필터에서 빠져 파일이 사라진 것처럼 보이고
        // 기본 플레이어도 열지 못한다 → 원본 확장자를 반드시 유지한다.
        let target_file_name = if !old_ext.is_empty()
            && !new_name_clean
                .to_lowercase()
                .ends_with(&format!(".{}", old_ext.to_lowercase()))
        {
            format!("{}.{}", new_name_clean, old_ext)
        } else {
            new_name_clean.to_string()
        };

        let new_path = parent_dir.join(&target_file_name);

        // 방어선: 결과가 여전히 "같은 폴더의 파일 하나" 여야 한다.
        if new_path.parent() != Some(parent_dir)
            || new_path.file_name() != Some(std::ffi::OsStr::new(target_file_name.as_str()))
        {
            return Err("파일명이 올바르지 않습니다.".to_string());
        }

        if new_path == old_path {
            return Ok(old_path_str.to_string());
        }

        // 대소문자만 바꾸는 변경은 macOS/Windows 의 대소문자 무시 파일시스템에서
        // `exists()` 가 true 지만 실제 대상은 자기 자신이다. 이걸 "이미 존재"로 막으면
        // 사용자는 대문자 오타를 영원히 고칠 수 없다 → 임시 이름을 거쳐 두 단계로 바꾼다.
        // (`canonicalize` 로는 판정할 수 없다 — APFS 는 경로의 대소문자를 그대로 돌려준다.)
        let target_exists = new_path.exists();
        let same_file = target_exists && is_same_file(old_path, &new_path);

        if target_exists && !same_file {
            return Err(format!("'{}' 파일이 이미 존재합니다.", target_file_name));
        }

        if same_file {
            let staging = parent_dir.join(format!(
                ".{}.rename-{}",
                target_file_name,
                std::process::id()
            ));
            fs::rename(old_path, &staging).map_err(|e| format!("파일명 변경 실패: {}", e))?;
            if let Err(e) = fs::rename(&staging, &new_path) {
                // 실패한 채로 두면 원본이 숨은 임시 이름으로 남아 사용자는 파일을 잃은 것으로 본다.
                if let Err(revert) = fs::rename(&staging, old_path) {
                    return Err(format!(
                        "파일명 변경 실패({}) — 원래 이름 복구도 실패했다. 현재 파일: {} ({})",
                        e,
                        staging.display(),
                        revert
                    ));
                }
                return Err(format!("파일명 변경 실패: {}", e));
            }
        } else {
            fs::rename(old_path, &new_path).map_err(|e| format!("파일명 변경 실패: {}", e))?;
        }

        invalidate_probe_cache(old_path_str);
        Ok(new_path.to_string_lossy().to_string())
    }

    pub fn open_in_explorer(path_str: &str) -> Result<(), String> {
        let p = Path::new(path_str);
        if !p.exists() {
            return Err("File not found.".to_string());
        }

        #[cfg(target_os = "windows")]
        {
            // explorer 는 성공해도 0 이 아닌 코드를 돌려주는 것으로 알려져 있어
            // 종료 코드를 검사하지 않는다. 실행 자체의 실패만 전파한다.
            Command::new("explorer")
                .arg(format!("/select,\"{}\"", path_str))
                .spawn()
                .map_err(|e| format!("Failed to open Windows Explorer: {}", e))?;
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg("-R")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to reveal file in Finder: {}", e))?;
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            // 리눅스에는 "파일을 선택한 채로 열기" 표준이 없어 상위 폴더를 연다.
            // 예전 코드는 `let _ = opener::open(..)` 로 결과를 버려 파일 관리자가 아예
            // 없어도 항상 성공을 보고했다 — 사용자는 아무 일도 안 생긴 이유를 알 수 없었다.
            let target = p.parent().unwrap_or(p);
            let status = Command::new("xdg-open")
                .arg(target)
                .status()
                .map_err(|e| format!("파일 관리자를 실행할 수 없습니다: {}", e))?;
            if !status.success() {
                return Err(format!(
                    "파일 관리자를 열지 못했습니다 (xdg-open 종료 코드: {}).",
                    status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "시그널 종료".to_string())
                ));
            }
        }

        Ok(())
    }

    pub fn open_with_default_player(path_str: &str) -> Result<(), String> {
        let p = Path::new(path_str);
        if !p.exists() {
            return Err("File not found.".to_string());
        }

        #[cfg(target_os = "windows")]
        {
            Command::new("rundll32.exe")
                .arg("url.dll,FileProtocolHandler")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to open media file: {}", e))?;
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to open media file on macOS: {}", e))?;
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            Command::new("xdg-open")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to open media file: {}", e))?;
        }

        Ok(())
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    let hrs = s / 3600;
    let mins = (s % 3600) / 60;
    let secs = s % 60;

    if hrs > 0 {
        format!("{:02}:{:02}:{:02}", hrs, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "omnirec-history-test-{}-{}-{}",
            tag,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).expect("임시 폴더 생성");
        dir
    }

    #[test]
    fn rename_rejects_path_escapes_and_keeps_extension() {
        let dir = temp_dir("rename");
        let file = dir.join("Recording.mp4");
        fs::write(&file, b"data").expect("샘플 파일");
        let file_str = file.to_string_lossy().to_string();

        // 경로 구분자 · 상위 폴더 · 빈 이름은 모두 거부한다.
        for bad in ["../escape", "sub/child", "..", ".", "   ", "C:name"] {
            assert!(
                HistoryManager::rename_file(&file_str, bad).is_err(),
                "거부해야 한다: {bad}"
            );
        }
        assert!(file.exists(), "거부된 요청은 원본을 건드리지 않는다");

        // 확장자를 빼고 입력해도 원본 확장자가 유지된다.
        let renamed = HistoryManager::rename_file(&file_str, "새 이름").expect("이름 변경");
        assert!(renamed.ends_with("새 이름.mp4"), "got {renamed}");
        assert!(!file.exists());
        assert!(Path::new(&renamed).exists());

        // 이미 존재하는 이름으로는 조용히 덮어쓰지 않는다.
        let other = dir.join("다른 파일.mp4");
        fs::write(&other, b"other").expect("샘플 파일 2");
        let err = HistoryManager::rename_file(&renamed, "다른 파일").expect_err("중복 이름은 거부");
        assert!(err.contains("이미 존재"), "got {err}");
        assert_eq!(fs::read(&other).expect("보존 확인"), b"other");

        // 대소문자만 바꾸는 변경은 대소문자 무시 파일시스템(APFS/NTFS)에서도 통과해야 한다.
        let ascii = dir.join("clip.mp4");
        fs::write(&ascii, b"clip").expect("샘플 파일 3");
        let upper =
            HistoryManager::rename_file(&ascii.to_string_lossy(), "CLIP").expect("대소문자 변경");
        assert!(upper.ends_with("CLIP.mp4"), "got {upper}");
        assert_eq!(fs::read(&upper).expect("내용 보존"), b"clip");
        let names: Vec<String> = fs::read_dir(&dir)
            .expect("폴더 읽기")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "CLIP.mp4"),
            "실제 이름이 바뀌어야 한다: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains(".rename-")),
            "임시 이름이 남으면 안 된다: {names:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_reports_missing_file_instead_of_succeeding() {
        let dir = temp_dir("delete");
        let file = dir.join("Recording.m4a");
        fs::write(&file, b"data").expect("샘플 파일");
        let file_str = file.to_string_lossy().to_string();

        HistoryManager::delete_file(&file_str).expect("삭제 성공");
        let err = HistoryManager::delete_file(&file_str).expect_err("두 번째 삭제는 실패");
        assert!(err.contains("찾을 수 없습니다"), "got {err}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn probe_cache_key_changes_when_file_content_changes() {
        let dir = temp_dir("cache-key");
        let file = dir.join("Recording.mp4");
        fs::write(&file, b"data").expect("샘플 파일");
        let before = ProbeKey {
            path: file.to_string_lossy().to_string(),
            size_bytes: fs::metadata(&file).expect("메타").len(),
            mtime_nanos: mtime_nanos(&fs::metadata(&file).expect("메타")),
        };

        fs::write(&file, b"data-longer").expect("덮어쓰기");
        let after = ProbeKey {
            path: file.to_string_lossy().to_string(),
            size_bytes: fs::metadata(&file).expect("메타").len(),
            mtime_nanos: mtime_nanos(&fs::metadata(&file).expect("메타")),
        };

        assert_ne!(
            before.size_bytes, after.size_bytes,
            "크기가 키에 들어 있어야 덮어쓴 파일이 캐시 미스가 된다"
        );
        assert!(before != after);

        fs::remove_dir_all(&dir).ok();
    }
}
