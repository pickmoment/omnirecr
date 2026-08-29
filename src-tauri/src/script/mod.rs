use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::{ScriptDraft, ScriptItem};

/// 한국어 낭독 평균 속도(자/초). Typecast 기본 속도(1.0x) 기준 근사값.
const KOREAN_CHARS_PER_SEC: f64 = 5.5;

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
            let _ = fs::create_dir_all(&omni_dir);
        }
        omni_dir.join("scripts.json")
    }

    pub fn list() -> Vec<ScriptItem> {
        let mut items = Self::load_raw();
        // 최근 수정 순 정렬
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        items
    }

    fn load_raw() -> Vec<ScriptItem> {
        let path = Self::store_path();
        if !path.exists() {
            return Vec::new();
        }
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<Vec<ScriptItem>>(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    fn persist(items: &[ScriptItem]) -> Result<(), String> {
        let path = Self::store_path();
        let json = serde_json::to_string_pretty(items)
            .map_err(|e| format!("대본 목록 직렬화 실패: {}", e))?;
        fs::write(&path, json).map_err(|e| format!("대본 파일 저장 실패: {}", e))
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

    pub fn upsert(draft: ScriptDraft) -> Result<ScriptItem, String> {
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

        let mut items = Self::load_raw();
        let now = Self::now_string();

        let result = match draft.id.filter(|id| !id.trim().is_empty()) {
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
                existing.clone()
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
                item
            }
        };

        Self::persist(&items)?;
        Ok(result)
    }

    pub fn delete(id: &str) -> Result<(), String> {
        let mut items = Self::load_raw();
        let before = items.len();
        items.retain(|s| s.id != id);
        if items.len() == before {
            return Err(format!("대본을 찾을 수 없습니다: {}", id));
        }
        Self::persist(&items)
    }

    pub fn duplicate(id: &str) -> Result<ScriptItem, String> {
        let items = Self::load_raw();
        let source = items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?;

        Self::upsert(ScriptDraft {
            id: None,
            title: format!("{} (복사본)", source.title),
            content: source.content.clone(),
            tags: source.tags.clone(),
            memo: source.memo.clone(),
        })
    }

    /// 녹음 결과 파일을 대본에 연결한다.
    pub fn attach_recording(id: &str, recorded_path: &str) -> Result<ScriptItem, String> {
        let mut items = Self::load_raw();
        let item = items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?;

        item.last_recorded_path = Some(recorded_path.to_string());
        item.last_recorded_at = Some(Self::now_string());
        item.record_count = item.record_count.saturating_add(1);
        let updated = item.clone();

        Self::persist(&items)?;
        Ok(updated)
    }

    /// 텍스트 파일(.txt / .md / .srt 등)을 읽어 새 대본으로 등록한다.
    pub fn import_from_file(path: &str) -> Result<ScriptItem, String> {
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
        let items = Self::load_raw();
        let item = items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("대본을 찾을 수 없습니다: {}", id))?;
        fs::write(path, &item.content).map_err(|e| format!("대본 내보내기 실패: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
