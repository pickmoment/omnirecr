use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::settings::{quarantine_corrupt_file, write_atomic};
use crate::types::{ScriptDraft, ScriptItem};

/// 한국어 낭독 평균 속도(자/초). Typecast 기본 속도(1.0x) 기준 근사값.
const KOREAN_CHARS_PER_SEC: f64 = 5.5;

/// 대본 저장소의 read-modify-write 전체를 직렬화하는 프로세스 전역 잠금.
///
/// 커맨드들은 각자 `load_raw` → 변형 → 저장 을 하는데, Tauri 커맨드는 서로 다른 스레드에서
/// 동시에 실행된다. 잠금이 없으면 "대본 저장"과 "녹음 결과 연결"이 겹칠 때 나중에 쓴 쪽이
/// 먼저 쓴 쪽의 변경을 통째로 지운다(lost update). 되돌리면 일괄 TTS 녹음처럼 저장이
/// 연달아 일어나는 흐름에서 대본이 조용히 사라진다.
///
/// 잠금은 재진입 불가다 — 잠금을 잡는 함수(공개 API)는 다른 공개 API 를 호출하지 않는다.
static STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock_store() -> MutexGuard<'static, ()> {
    // 보호 대상은 `()` 이고 실제 상태는 디스크에 있다. 패닉 한 번(poison)으로 저장소를
    // 영구히 못 쓰게 만들 이유가 없으므로 오염된 잠금은 그대로 복구해 쓴다.
    STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 대본 낭독 예상 시간(초).
/// 공백/줄바꿈은 제외하고 세되, 문장부호마다 쉼(0.35초)을 더한다.
pub fn estimate_reading_secs(content: &str) -> f64 {
    let spoken_chars = content.chars().filter(|c| !c.is_whitespace()).count() as f64;
    let pause_marks = content
        .chars()
        .filter(|c| matches!(c, '.' | '!' | '?' | ',' | '…'))
        .count() as f64;
    spoken_chars / KOREAN_CHARS_PER_SEC + pause_marks * 0.35
}

pub struct ScriptManager;

impl ScriptManager {
    pub fn store_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let omni_dir = home.join(".omnirec");
        if !omni_dir.exists() {
            if let Err(e) = fs::create_dir_all(&omni_dir) {
                log::error!("대본 폴더 생성 실패 ({}): {}", omni_dir.display(), e);
            }
        }
        omni_dir.join("scripts.json")
    }

    pub fn list() -> Vec<ScriptItem> {
        let _guard = lock_store();
        // 커맨드 시그니처가 `Vec` 이라 에러를 올릴 수 없다. 대신 **읽기 실패로 목록이
        // 비어 보이더라도 파일은 절대 지우거나 덮어쓰지 않는다**(load_raw_from 이 손상본을
        // 보존만 한다). 예전 구현은 여기서 빈 목록으로 접고 다음 저장이 그 빈 상태를
        // 확정해 라이브러리를 통째로 날렸다.
        match Self::load_raw_from(&Self::store_path()) {
            Ok(mut items) => {
                // 최근 수정 순 정렬
                items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                items
            }
            Err(e) => {
                log::error!("대본 목록을 읽을 수 없다: {}", e);
                Vec::new()
            }
        }
    }

    /// 저장소를 읽는다. 파일이 없으면 빈 목록, 해석 불가면 원본을 보존하고 에러.
    /// 잠금은 호출자가 잡는다.
    fn load_raw_from(path: &Path) -> Result<Vec<ScriptItem>, String> {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(format!(
                    "대본 파일을 읽을 수 없습니다 ({}): {}",
                    path.display(),
                    e
                ))
            }
        };

        match serde_json::from_str::<Vec<ScriptItem>>(&content) {
            Ok(items) => Ok(items),
            Err(parse_err) => {
                // 손상본은 지우지 않고 옮겨 보존한다 — 사용자가 손으로 복구할 수 있어야 한다.
                let preserved = match quarantine_corrupt_file(path) {
                    Ok(saved) => format!("원본은 {} 로 보존했다", saved.display()),
                    Err(rename_err) => {
                        format!("원본 보존까지 실패해 파일을 그대로 남겼다: {}", rename_err)
                    }
                };
                Err(format!(
                    "대본 파일을 해석할 수 없습니다: {} ({})",
                    parse_err, preserved
                ))
            }
        }
    }

    /// 저장소를 원자적으로 교체한다. 잠금은 호출자가 잡는다.
    fn persist_to(path: &Path, items: &[ScriptItem]) -> Result<(), String> {
        let json = serde_json::to_string_pretty(items)
            .map_err(|e| format!("대본 목록 직렬화 실패: {}", e))?;
        write_atomic(path, &json)
    }

    fn now_string() -> String {
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    }

    fn new_id() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("scr_{:x}", nanos)
    }

    /// 통계값(글자 수 / 줄 수 / 예상 낭독 시간) 재계산.
    fn apply_stats(item: &mut ScriptItem) {
        let content = &item.content;
        item.char_count = content.chars().count();
        item.line_count = content.lines().filter(|l| !l.trim().is_empty()).count();
        item.estimated_secs = estimate_reading_secs(content);
    }

    /// 아직 쓰이지 않은 첫 복제 제목을 고른다: `(복사본)`, `(복사본 2)`, `(복사본 3)` …
    ///
    /// `(복사본)` 고정 접미어면 같은 대본을 두 번 복제할 때 제목이 완전히 겹친다.
    /// 제목이 곧 TTS 녹음 파일명이라, 겹치면 두 대본의 녹음이 같은 파일을 서로 덮어쓴다.
    fn unique_copy_title(items: &[ScriptItem], base: &str) -> String {
        let base = base.trim();
        let mut candidate = format!("{} (복사본)", base);
        let mut nth = 2;
        while items.iter().any(|s| s.title.trim() == candidate) {
            candidate = format!("{} (복사본 {})", base, nth);
            nth += 1;
        }
        candidate
    }

    /// 초안을 목록에 반영한다(신규 추가 또는 기존 갱신). 잠금은 호출자가 잡는다.
    fn apply_draft(items: &mut Vec<ScriptItem>, draft: ScriptDraft) -> Result<ScriptItem, String> {
        let title = if draft.title.trim().is_empty() {
            // 제목이 비면 본문 첫 줄에서 자동 생성
            draft
                .content
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| {
                    let t: String = l.trim().chars().take(30).collect();
                    t
                })
                .unwrap_or_else(|| "제목 없는 대본".to_string())
        } else {
            draft.title.trim().to_string()
        };

        let tags: Vec<String> = draft
            .tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();

        let now = Self::now_string();

        match draft.id.filter(|id| !id.trim().is_empty()) {
            Some(id) => {
                let existing = items
                    .iter_mut()
                    .find(|s| s.id == id)
                    .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?;
                existing.title = title;
                existing.content = draft.content;
                existing.tags = tags;
                existing.memo = draft.memo;
                existing.updated_at = now;
                Self::apply_stats(existing);
                Ok(existing.clone())
            }
            None => {
                let mut item = ScriptItem {
                    id: Self::new_id(),
                    title,
                    content: draft.content,
                    tags,
                    memo: draft.memo,
                    created_at: now.clone(),
                    updated_at: now,
                    char_count: 0,
                    line_count: 0,
                    estimated_secs: 0.0,
                    last_recorded_path: None,
                    last_recorded_at: None,
                    record_count: 0,
                };
                Self::apply_stats(&mut item);
                items.push(item.clone());
                Ok(item)
            }
        }
    }

    pub fn upsert(draft: ScriptDraft) -> Result<ScriptItem, String> {
        let _guard = lock_store();
        let path = Self::store_path();
        let mut items = Self::load_raw_from(&path)?;
        let result = Self::apply_draft(&mut items, draft)?;
        Self::persist_to(&path, &items)?;
        Ok(result)
    }

    pub fn delete(id: &str) -> Result<(), String> {
        let _guard = lock_store();
        let path = Self::store_path();
        let mut items = Self::load_raw_from(&path)?;
        let before = items.len();
        items.retain(|s| s.id != id);
        if items.len() == before {
            return Err(format!("대본을 찾을 수 없습니다: {}", id));
        }
        Self::persist_to(&path, &items)
    }

    pub fn duplicate(id: &str) -> Result<ScriptItem, String> {
        let _guard = lock_store();
        let path = Self::store_path();
        let mut items = Self::load_raw_from(&path)?;
        let source = items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?
            .clone();
        let title = Self::unique_copy_title(&items, &source.title);

        let created = Self::apply_draft(
            &mut items,
            ScriptDraft {
                id: None,
                title,
                content: source.content,
                tags: source.tags,
                memo: source.memo,
            },
        )?;
        Self::persist_to(&path, &items)?;
        Ok(created)
    }

    /// 녹음 결과 파일을 대본에 연결한다.
    pub fn attach_recording(id: &str, recorded_path: &str) -> Result<ScriptItem, String> {
        let _guard = lock_store();
        let path = Self::store_path();
        let mut items = Self::load_raw_from(&path)?;
        let item = items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?;

        item.last_recorded_path = Some(recorded_path.to_string());
        item.last_recorded_at = Some(Self::now_string());
        item.record_count = item.record_count.saturating_add(1);
        let updated = item.clone();

        Self::persist_to(&path, &items)?;
        Ok(updated)
    }

    /// 텍스트 파일(.txt / .md / .srt 등)을 읽어 새 대본으로 등록한다.
    pub fn import_from_file(path: &str) -> Result<ScriptItem, String> {
        // 잠금을 잡기 전에 외부 파일을 읽는다 — 잠금은 재진입 불가이고 `upsert` 가 잡는다.
        let content = crate::subtitle::SubtitleController::read_script_file(path)?;
        let file_stem = PathBuf::from(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "가져온 대본".to_string());

        Self::upsert(ScriptDraft {
            id: None,
            title: file_stem,
            content,
            tags: vec!["가져오기".to_string()],
            memo: format!("원본 파일: {}", path),
        })
    }

    pub fn export_to_file(id: &str, path: &str) -> Result<(), String> {
        let content = {
            let _guard = lock_store();
            let items = Self::load_raw_from(&Self::store_path())?;
            items
                .iter()
                .find(|s| s.id == id)
                .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?
                .content
                .clone()
        };
        // 사용자가 고른 경로도 원자적으로 쓴다 — 기존 파일을 절단만 하고 죽으면
        // 내보내기 대상이 빈 파일이 된다.
        write_atomic(Path::new(path), &content).map_err(|e| format!("대본 내보내기 실패: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, title: &str) -> ScriptItem {
        ScriptItem {
            id: id.to_string(),
            title: title.to_string(),
            content: "본문".to_string(),
            tags: Vec::new(),
            memo: String::new(),
            created_at: "2026-01-01 00:00:00".to_string(),
            updated_at: "2026-01-01 00:00:00".to_string(),
            char_count: 2,
            line_count: 1,
            estimated_secs: 0.0,
            last_recorded_path: None,
            last_recorded_at: None,
            record_count: 0,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "omnirec-script-test-{}-{}-{}",
            tag,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).expect("임시 폴더 생성");
        dir
    }

    #[test]
    fn estimates_reading_time_from_spoken_characters_only() {
        // 공백 2자를 뺀 12자(마침표 포함) / 5.5 = 2.1818초, 문장부호(.) 1개 → +0.35초
        let secs = estimate_reading_secs("안녕하세요 오늘은 좋은날.");
        assert!((secs - (12.0 / 5.5 + 0.35)).abs() < 1e-6, "got {secs}");
    }

    #[test]
    fn empty_script_takes_no_time() {
        assert_eq!(estimate_reading_secs(""), 0.0);
        assert_eq!(estimate_reading_secs("   \n\n  "), 0.0);
    }

    #[test]
    fn duplicate_titles_never_collide() {
        let mut items = vec![sample("a", "회사 소개")];

        let first = ScriptManager::unique_copy_title(&items, "회사 소개");
        assert_eq!(first, "회사 소개 (복사본)");
        items.push(sample("b", &first));

        let second = ScriptManager::unique_copy_title(&items, "회사 소개");
        assert_eq!(second, "회사 소개 (복사본 2)");
        items.push(sample("c", &second));

        let third = ScriptManager::unique_copy_title(&items, "회사 소개");
        assert_eq!(third, "회사 소개 (복사본 3)");

        // 복제의 복제도 겹치지 않는다.
        let nested = ScriptManager::unique_copy_title(&items, "회사 소개 (복사본)");
        assert_eq!(nested, "회사 소개 (복사본) (복사본)");
    }

    #[test]
    fn corrupt_store_is_preserved_and_reported() {
        let dir = temp_dir("corrupt");
        let path = dir.join("scripts.json");
        let broken = "[{\"id\": \"a\",,,";
        fs::write(&path, broken).expect("손상 저장소 기록");

        let err = ScriptManager::load_raw_from(&path).expect_err("에러를 올려야 한다");
        assert!(err.contains("해석할 수 없습니다"), "got {err}");
        assert!(!path.exists(), "손상본은 옮겨져 보존된다");

        let preserved: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("폴더 읽기")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains("scripts.json.bad-"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(preserved.len(), 1);
        assert_eq!(
            fs::read_to_string(&preserved[0]).expect("보존본 읽기"),
            broken
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_store_reads_as_empty_and_round_trips_atomically() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("scripts.json");

        assert!(ScriptManager::load_raw_from(&path)
            .expect("빈 목록")
            .is_empty());

        let mut items = Vec::new();
        ScriptManager::apply_draft(
            &mut items,
            ScriptDraft {
                id: None,
                title: "  제목  ".to_string(),
                content: "한 줄".to_string(),
                tags: vec!["  ".to_string(), " 태그 ".to_string()],
                memo: String::new(),
            },
        )
        .expect("초안 반영");
        ScriptManager::persist_to(&path, &items).expect("원자적 저장");

        let reloaded = ScriptManager::load_raw_from(&path).expect("재로드");
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].title, "제목");
        assert_eq!(reloaded[0].tags, vec!["태그".to_string()]);
        let leftovers = fs::read_dir(&dir)
            .expect("폴더 읽기")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);

        fs::remove_dir_all(&dir).ok();
    }
}
