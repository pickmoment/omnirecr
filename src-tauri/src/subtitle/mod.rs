use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::settings::SettingsManager;
use crate::types::{SubtitleGenerateResult, SubtitleGenerateTask, SubtitleItem};

pub struct SubtitleController;

#[derive(Debug, Clone)]
struct SpeechSegment {
    start: f64,
    end: f64,
}

#[derive(Debug, Clone)]
struct ParsedLine {
    text: String,
    explicit_start: Option<f64>,
    explicit_end: Option<f64>,
}

impl SubtitleController {
    pub fn new() -> Self {
        Self
    }

    /// Read text file content with utf-8 encoding fallback
    pub fn read_script_file(path: &str) -> Result<String, String> {
        let bytes = fs::read(path).map_err(|e| format!("Failed to read script file: {}", e))?;

        if let Ok(s) = String::from_utf8(bytes.clone()) {
            return Ok(s);
        }

        let s = String::from_utf8_lossy(&bytes).to_string();
        Ok(s)
    }

    /// Save subtitle string content to a file
    pub fn save_subtitle_file(path: &str, content: &str) -> Result<(), String> {
        let p = Path::new(path);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                // 폴더 생성 실패를 삼키면 바로 아래 write 가 엉뚱한 OS 에러로 죽는다 → 원인을 그대로 올린다
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "자막 저장 폴더를 만들 수 없습니다({}): {}",
                        parent.display(),
                        e
                    )
                })?;
            }
        }
        fs::write(path, content).map_err(|e| format!("자막 파일 저장 실패({}): {}", path, e))
    }

    /// Main entry point: generate subtitles from audio file and script text
    pub fn generate(
        task: SubtitleGenerateTask,
        custom_ffmpeg_path: Option<String>,
    ) -> Result<SubtitleGenerateResult, String> {
        let audio_path = PathBuf::from(&task.audio_path);
        if !audio_path.exists() {
            return Err(format!(
                "오디오 파일이 존재하지 않습니다: {}",
                task.audio_path
            ));
        }

        // 1. Probe total duration of audio
        let duration =
            Self::get_audio_duration(&audio_path, custom_ffmpeg_path.as_deref()).unwrap_or(0.0);

        if duration <= 0.0 {
            return Err("오디오 길이를 측정할 수 없거나 재생 시간이 0초입니다.".to_string());
        }

        // 2. Parse script lines
        let parsed_lines = Self::split_and_clean_script(
            &task.script_text,
            &task.split_mode,
            task.max_chars.max(5),
            task.split_on_comma,
        );

        if parsed_lines.is_empty() {
            return Err("대본에서 유효한 문장을 찾을 수 없습니다.".to_string());
        }

        // 3. High-precision VAD (Voice Activity Detection) via 16kHz PCM analysis
        let user_thresh = task.silence_threshold_db.unwrap_or(-35.0);
        let user_min_silence = task.min_silence_duration_secs.unwrap_or(0.25).max(0.05);

        let speech_segments = Self::detect_speech_segments_pcm(
            &audio_path,
            duration,
            user_thresh,
            user_min_silence,
            custom_ffmpeg_path.as_deref(),
        );

        let speech_segments_detected = speech_segments.len();

        // 4. Align script lines to detected voice activity
        let start_offset = task.start_offset_secs.unwrap_or(0.1).max(0.0);
        let end_margin = task.end_margin_secs.unwrap_or(0.2).max(0.0);

        let subtitles = Self::align_script_to_speech(
            &parsed_lines,
            &speech_segments,
            duration,
            start_offset,
            end_margin,
        );

        // 5. Generate SRT and VTT content
        let srt_content = Self::build_srt_string(&subtitles);
        let vtt_content = Self::build_vtt_string(&subtitles);

        // 6. Handle auto saving if requested
        let mut srt_path_saved = None;
        let mut vtt_path_saved = None;

        if task.auto_save {
            let stem = audio_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "subtitles".to_string());

            let target_dir = if let Some(dir) = task.output_dir.filter(|d| !d.is_empty()) {
                PathBuf::from(dir)
            } else if let Some(parent) = audio_path.parent() {
                parent.to_path_buf()
            } else {
                PathBuf::from(".")
            };

            // 저장 실패를 삼키면 "저장됨"으로 표시된 채 파일이 없다 → 어느 포맷이 왜 실패했는지 담아 올린다
            fs::create_dir_all(&target_dir).map_err(|e| {
                format!(
                    "자막 저장 폴더를 만들 수 없습니다({}): {}",
                    target_dir.display(),
                    e
                )
            })?;

            let srt_file = target_dir.join(format!("{}.srt", stem));
            let vtt_file = target_dir.join(format!("{}.vtt", stem));

            fs::write(&srt_file, &srt_content)
                .map_err(|e| format!("SRT 자막 저장 실패({}): {}", srt_file.display(), e))?;
            srt_path_saved = Some(srt_file.to_string_lossy().to_string());

            fs::write(&vtt_file, &vtt_content)
                .map_err(|e| format!("VTT 자막 저장 실패({}): {}", vtt_file.display(), e))?;
            vtt_path_saved = Some(vtt_file.to_string_lossy().to_string());
        }

        Ok(SubtitleGenerateResult {
            subtitles,
            srt_content,
            vtt_content,
            srt_path: srt_path_saved,
            vtt_path: vtt_path_saved,
            total_duration: duration,
            speech_segments_detected,
            script_lines_count: parsed_lines.len(),
        })
    }

    /// Split raw script into chunks according to splitting mode and max character limit
    fn split_and_clean_script(
        raw: &str,
        mode: &str,
        max_chars: usize,
        split_on_comma: bool,
    ) -> Vec<ParsedLine> {
        let lrc_regex = Regex::new(r"\[(\d{1,2}):(\d{2})(?:\.(\d+))?\]").unwrap();
        let srt_time_regex =
            Regex::new(r"(\d{2}:\d{2}:\d{2}[,\.]\d{3})\s*-->\s*(\d{2}:\d{2}:\d{2}[,\.]\d{3})")
                .unwrap();

        let mut parsed_results = Vec::new();

        let raw_clean = raw.replace("\r\n", "\n").replace('\r', "\n");
        let raw_lines: Vec<&str> = raw_clean.lines().collect();

        // SRT 블록 헤더(숫자 인덱스 줄) 판정: 바로 다음 줄이 타임스탬프면 인덱스 줄로 본다.
        // 이 줄을 소비하지 않으면 "1", "2" 같은 숫자가 그대로 낭독 자막이 되어 화면에 뜬다.
        let is_srt_index = |idx: usize| -> bool {
            let cur = raw_lines.get(idx).map(|l| l.trim()).unwrap_or("");
            if cur.is_empty() || !cur.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            raw_lines
                .get(idx + 1)
                .map(|next| srt_time_regex.is_match(next.trim()))
                .unwrap_or(false)
        };

        let mut i = 0;
        while i < raw_lines.len() {
            let line = raw_lines[i].trim();
            if line.is_empty() {
                i += 1;
                continue;
            }

            if is_srt_index(i) {
                i += 1;
                continue;
            }

            // SRT 블록: 타임스탬프 줄 + 뒤따르는 본문 줄들
            if let Some(caps) = srt_time_regex.captures(line) {
                let st = Self::parse_time_str(&caps[1]);
                let et = Self::parse_time_str(&caps[2]);
                i += 1;
                let mut text_acc = String::new();
                while i < raw_lines.len() {
                    let cur = raw_lines[i].trim();
                    // 빈 줄 · 다음 블록의 인덱스 · 다음 타임스탬프에서 멈춘다
                    // (빈 줄 없이 블록이 이어지는 SRT 도 있어서 세 조건을 모두 본다)
                    if cur.is_empty() || is_srt_index(i) || srt_time_regex.is_match(cur) {
                        break;
                    }
                    if !text_acc.is_empty() {
                        text_acc.push(' ');
                    }
                    text_acc.push_str(cur);
                    i += 1;
                }
                if !text_acc.is_empty() {
                    parsed_results.push(ParsedLine {
                        text: text_acc,
                        explicit_start: st,
                        explicit_end: et,
                    });
                }
                continue;
            }

            // Check LRC timestamp [mm:ss.xx]
            if let Some(caps) = lrc_regex.captures(line) {
                let mins: f64 = caps[1].parse().unwrap_or(0.0);
                let secs: f64 = caps[2].parse().unwrap_or(0.0);
                let frac: f64 = caps
                    .get(3)
                    .map(|m| format!("0.{}", m.as_str()).parse().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let explicit_start = Some(mins * 60.0 + secs + frac);
                let no_ts = lrc_regex.replace_all(line, "").trim().to_string();
                if !no_ts.is_empty() {
                    parsed_results.push(ParsedLine {
                        text: no_ts,
                        explicit_start,
                        explicit_end: None,
                    });
                }
                i += 1;
                continue;
            }

            // Normal text line
            match mode {
                "line" => {
                    parsed_results.push(ParsedLine {
                        text: line.to_string(),
                        explicit_start: None,
                        explicit_end: None,
                    });
                }
                "sentence" => {
                    let sentences = Self::split_into_sentences(line, split_on_comma);
                    for s in sentences {
                        if !s.is_empty() {
                            parsed_results.push(ParsedLine {
                                text: s,
                                explicit_start: None,
                                explicit_end: None,
                            });
                        }
                    }
                }
                "length" => {
                    let chunks = Self::split_by_char_length(line, max_chars);
                    for c in chunks {
                        if !c.is_empty() {
                            parsed_results.push(ParsedLine {
                                text: c,
                                explicit_start: None,
                                explicit_end: None,
                            });
                        }
                    }
                }
                _ => {
                    // "auto": split sentences first, then length (comma-splitting is a "sentence" mode-only option)
                    let sentences = Self::split_into_sentences(line, false);
                    for s in sentences {
                        if s.chars().count() > max_chars {
                            let chunks = Self::split_by_char_length(&s, max_chars);
                            for c in chunks {
                                if !c.is_empty() {
                                    parsed_results.push(ParsedLine {
                                        text: c,
                                        explicit_start: None,
                                        explicit_end: None,
                                    });
                                }
                            }
                        } else if !s.is_empty() {
                            parsed_results.push(ParsedLine {
                                text: s,
                                explicit_start: None,
                                explicit_end: None,
                            });
                        }
                    }
                }
            }

            i += 1;
        }

        parsed_results
    }

    /// Split string into sentences by punctuation (. ? ! … 。 ~), optionally also on commas (, 、 ，)
    fn split_into_sentences(text: &str, split_on_comma: bool) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();

        let mut i = 0;
        while i < len {
            let ch = chars[i];
            current.push(ch);

            let is_comma = ch == ',' || ch == '、' || ch == '，';
            let is_punct = ch == '.'
                || ch == '?'
                || ch == '!'
                || ch == '…'
                || ch == '。'
                || ch == '~'
                || (split_on_comma && is_comma);
            if is_punct {
                // Check if decimal number like 3.14, or thousands separator like 1,000
                let is_decimal = (ch == '.' || is_comma)
                    && i > 0
                    && chars[i - 1].is_ascii_digit()
                    && i + 1 < len
                    && chars[i + 1].is_ascii_digit();
                if !is_decimal {
                    // Include any trailing quotes or brackets
                    let mut next_idx = i + 1;
                    while next_idx < len
                        && (chars[next_idx] == '"'
                            || chars[next_idx] == '\''
                            || chars[next_idx] == '”'
                            || chars[next_idx] == '’'
                            || chars[next_idx] == '」'
                            || chars[next_idx] == '』'
                            || chars[next_idx] == ')'
                            || chars[next_idx] == ']')
                    {
                        current.push(chars[next_idx]);
                        i = next_idx;
                        next_idx += 1;
                    }

                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        result.push(trimmed);
                    }
                    current.clear();
                }
            }

            i += 1;
        }

        let remaining = current.trim().to_string();
        if !remaining.is_empty() {
            result.push(remaining);
        }

        result
    }

    /// Split long text by character length while preserving word boundaries.
    /// 공백이 없는 한국어/CJK 문장은 단어 경계가 없어 공백 분할만으로는 max_chars 를 그냥 넘긴다
    /// (자막 한 줄이 화면을 뚫고 나가는 사고) → 토큰 자체가 제한을 넘으면 char 경계로 강제 분할한다.
    /// 바이트 슬라이싱은 멀티바이트 문자를 쪼개 패닉/글자 깨짐을 만들므로 절대 쓰지 않는다.
    fn split_by_char_length(text: &str, max_chars: usize) -> Vec<String> {
        let max_chars = max_chars.max(1);
        if text.chars().count() <= max_chars {
            return vec![text.to_string()];
        }

        let mut chunks: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut current_len = 0usize; // current 의 char 수(바이트 길이가 아니다)

        for word in text.split_whitespace() {
            let word_len = word.chars().count();

            if word_len > max_chars {
                // 단어 하나가 제한보다 길다 → 여기서 char 로 쪼개지 않으면 제한이 무의미해진다
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                    current_len = 0;
                }
                let mut piece = String::new();
                let mut piece_len = 0usize;
                for ch in word.chars() {
                    piece.push(ch);
                    piece_len += 1;
                    if piece_len == max_chars {
                        chunks.push(std::mem::take(&mut piece));
                        piece_len = 0;
                    }
                }
                if piece_len > 0 {
                    // 남은 꼬리는 다음 단어와 이어붙일 수 있게 current 로 넘긴다
                    current = piece;
                    current_len = piece_len;
                }
                continue;
            }

            let potential_len = if current.is_empty() {
                word_len
            } else {
                current_len + 1 + word_len
            };

            if potential_len > max_chars && !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current.push_str(word);
                current_len = word_len;
            } else {
                if !current.is_empty() {
                    current.push(' ');
                    current_len += 1;
                }
                current.push_str(word);
                current_len += word_len;
            }
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    fn parse_time_str(s: &str) -> Option<f64> {
        let clean = s.replace(',', ".");
        let parts: Vec<&str> = clean.split(':').collect();
        if parts.len() == 3 {
            let h: f64 = parts[0].parse().ok()?;
            let m: f64 = parts[1].parse().ok()?;
            let sec: f64 = parts[2].parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + sec)
        } else {
            None
        }
    }

    /// Probe duration of audio/video using ffprobe or ffmpeg
    fn get_audio_duration(path: &Path, custom_ffmpeg_path: Option<&str>) -> Option<f64> {
        if let Ok(ffprobe_path) = SettingsManager::find_ffprobe(custom_ffmpeg_path) {
            let mut cmd = Command::new(ffprobe_path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            cmd.args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                path.to_str().unwrap_or_default(),
            ]);

            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    let out_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if let Ok(dur) = out_str.parse::<f64>() {
                        if dur > 0.0 {
                            return Some(dur);
                        }
                    }
                }
            }
        }

        if let Ok(ffmpeg_path) = SettingsManager::find_ffmpeg(custom_ffmpeg_path) {
            let mut cmd = Command::new(ffmpeg_path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            cmd.args(["-i", path.to_str().unwrap_or_default()]);

            if let Ok(output) = cmd.output() {
                let err_str = String::from_utf8_lossy(&output.stderr);
                let re = Regex::new(r"Duration:\s*(\d{2}):(\d{2}):(\d{2}\.?\d*)").unwrap();
                if let Some(caps) = re.captures(&err_str) {
                    let h: f64 = caps[1].parse().unwrap_or(0.0);
                    let m: f64 = caps[2].parse().unwrap_or(0.0);
                    let s: f64 = caps[3].parse().unwrap_or(0.0);
                    let total = h * 3600.0 + m * 60.0 + s;
                    if total > 0.0 {
                        return Some(total);
                    }
                }
            }
        }

        None
    }

    /// FFmpeg 로 16kHz 모노 f32le PCM 을 뽑아 raw 바이트로 돌려준다.
    /// f32 배열 변환은 필요한 호출자만 하도록 분리해 두었다(프론트엔드로 보낼 때는
    /// Float32Array 로 그대로 매핑되므로 중간 Vec<f32> 사본이 순수 낭비다).
    fn run_ffmpeg_pcm_16k(
        path: &Path,
        custom_ffmpeg_path: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let ffmpeg_path = SettingsManager::find_ffmpeg(custom_ffmpeg_path)?;

        let mut cmd = Command::new(ffmpeg_path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        cmd.args([
            "-hide_banner",
            "-nostdin",
            "-vn",
            "-i",
            path.to_str().unwrap_or_default(),
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "pipe:1",
        ]);

        let output = cmd
            .output()
            .map_err(|e| format!("FFmpeg 실행 실패: {}", e))?;

        // 예전에는 stdout 이 비어있지 않으면 종료 코드를 무시했다 → 디코딩 중간에 죽어 잘린
        // PCM 을 정상으로 받아 자막 타임라인이 조용히 어긋났다. 종료 코드를 반드시 본다.
        if !output.status.success() {
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            return Err(format!(
                "FFmpeg 오디오 PCM 추출 실패(종료 코드 {}): {}",
                code,
                Self::tail_of_stderr(&output.stderr)
            ));
        }

        let mut raw_bytes = output.stdout;
        if raw_bytes.len() < 4 * 160 {
            return Err("오디오 데이터가 너무 짧거나 비어 있습니다.".to_string());
        }

        // f32 경계로 정렬해 둔다. 프론트엔드가 이 버퍼를 Float32Array 로 바로 감싸므로
        // 길이가 4의 배수가 아니면 RangeError 로 터진다.
        let remainder = raw_bytes.len() % 4;
        if remainder != 0 {
            let keep = raw_bytes.len() - remainder;
            raw_bytes.truncate(keep);
        }

        Ok(raw_bytes)
    }

    /// FFmpeg stderr 의 뒤쪽만 잘라 에러 메시지에 담는다(앞부분은 스트림 정보라 원인이 뒤에 있다)
    fn tail_of_stderr(stderr: &[u8]) -> String {
        const MAX_CHARS: usize = 600;
        let text = String::from_utf8_lossy(stderr);
        let trimmed = text.trim();
        let count = trimmed.chars().count();
        if count <= MAX_CHARS {
            return trimmed.to_string();
        }
        let skip = count - MAX_CHARS;
        format!("...{}", trimmed.chars().skip(skip).collect::<String>())
    }

    /// 프론트엔드 전송용: 16kHz 모노 f32le PCM raw 바이트(중간 Vec<f32> 없음)
    pub fn extract_pcm_16k_bytes(
        path: &Path,
        custom_ffmpeg_path: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        Self::run_ffmpeg_pcm_16k(path, custom_ffmpeg_path)
    }

    /// Rust VAD 내부용: 16kHz 모노 Float32 샘플
    pub fn extract_pcm_16k(
        path: &Path,
        custom_ffmpeg_path: Option<&str>,
    ) -> Result<Vec<f32>, String> {
        let raw_bytes = Self::run_ffmpeg_pcm_16k(path, custom_ffmpeg_path)?;

        Ok(raw_bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    /// High-precision Voice Activity Detection by decoding audio to 16kHz mono PCM
    fn detect_speech_segments_pcm(
        path: &Path,
        total_duration: f64,
        user_thresh_db: f64,
        min_silence_secs: f64,
        custom_ffmpeg_path: Option<&str>,
    ) -> Vec<SpeechSegment> {
        let samples = match Self::extract_pcm_16k(path, custom_ffmpeg_path) {
            Ok(s) => s,
            Err(e) => {
                // VAD 가 없어도 비례 배분으로 자막은 만든다. 다만 원인을 완전히 삼키면
                // "왜 타이밍이 이상한지"를 추적할 수 없으므로 로그에는 반드시 남긴다.
                eprintln!("자막 VAD PCM 추출 실패, 비례 배분으로 대체합니다: {}", e);
                return Vec::new();
            }
        };

        Self::detect_speech_segments_from_samples(
            &samples,
            total_duration,
            user_thresh_db,
            min_silence_secs,
        )
    }

    /// NaN/무한대를 기본값으로 눌러준다.
    /// NaN 은 max/min/clamp 를 그대로 통과하고 모든 비교를 false 로 만들어
    /// "발화 구간 0개"처럼 조용히 잘못된 결과를 만든다.
    fn finite_or(value: f32, default: f32) -> f32 {
        if value.is_finite() {
            value
        } else {
            default
        }
    }

    /// 16kHz 모노 PCM 샘플에서 발화 구간을 찾는다(FFmpeg·파일 I/O 없는 순수 함수)
    fn detect_speech_segments_from_samples(
        samples: &[f32],
        total_duration: f64,
        user_thresh_db: f64,
        min_silence_secs: f64,
    ) -> Vec<SpeechSegment> {
        const SAMPLE_RATE: f64 = 16_000.0;
        const FRAME_SIZE: usize = 160; // 10ms per frame at 16kHz
        const FRAME_DURATION: f64 = 0.01;

        if samples.is_empty() {
            return Vec::new();
        }

        // 오디오 끝은 프레임 개수가 아니라 실제 샘플 수에서 유도한다.
        // samples.len() / FRAME_SIZE 는 내림이라 마지막 최대 9.94ms 가 사라지고,
        // 그만큼 끝 자막이 잘려 마지막 낭독이 자막 없이 지나간다.
        let audio_dur = samples.len() as f64 / SAMPLE_RATE;
        let timeline_end = if total_duration > 0.0 {
            audio_dur.min(total_duration)
        } else {
            audio_dur
        };

        // 마지막 부분 프레임도 0 패딩해서 분석한다(합을 FRAME_SIZE 로 나누는 것이 0 패딩과 같다)
        let num_frames = samples.len().div_ceil(FRAME_SIZE);
        let mut frame_rms_db: Vec<f32> = Vec::with_capacity(num_frames);

        for i in 0..num_frames {
            let start = i * FRAME_SIZE;
            let end = (start + FRAME_SIZE).min(samples.len());

            let mut sum_sq = 0.0_f32;
            for &s in &samples[start..end] {
                if s.is_finite() {
                    sum_sq += s * s;
                }
            }
            let rms = (sum_sq / FRAME_SIZE as f32).sqrt();
            let db = if rms > 1e-6 {
                20.0 * rms.log10()
            } else {
                -90.0
            };
            frame_rms_db.push(Self::finite_or(db, -90.0));
        }

        // Estimate noise floor and peak speech energy dynamically
        let mut sorted_db = frame_rms_db.clone();
        // partial_cmp().unwrap() 은 NaN 하나로 패닉한다 → total_cmp 로 정렬한다
        sorted_db.sort_by(|a, b| a.total_cmp(b));

        let noise_floor_idx = (num_frames as f64 * 0.15) as usize;
        let speech_peak_idx = (num_frames as f64 * 0.90) as usize;

        let noise_floor = Self::finite_or(
            sorted_db.get(noise_floor_idx).copied().unwrap_or(-50.0),
            -50.0,
        );
        let speech_peak = Self::finite_or(
            sorted_db.get(speech_peak_idx).copied().unwrap_or(-15.0),
            -15.0,
        );

        // Adaptive threshold: between noise floor and peak, clamped to reasonable range.
        // clamp 만으로는 NaN 이 그대로 통과하므로 finite_or 로 한 번 더 막는다.
        let dynamic_threshold = Self::finite_or(
            (noise_floor + (speech_peak - noise_floor) * 0.35).clamp(-55.0, -22.0),
            -35.0,
        );

        // Combine with user threshold
        let mixed_threshold =
            if user_thresh_db.is_finite() && user_thresh_db < -10.0 && user_thresh_db > -70.0 {
                (dynamic_threshold * 0.7) + (user_thresh_db as f32 * 0.3)
            } else {
                dynamic_threshold
            };
        let effective_threshold = Self::finite_or(mixed_threshold, -35.0);

        // NaN 이 들어오면 `as usize` 가 0 이 되어 첫 무음 프레임에서 구간을 끊어버린다
        let min_silence_secs = if min_silence_secs.is_finite() {
            min_silence_secs.max(0.05)
        } else {
            0.25
        };
        let min_speech_frames = (0.06 / FRAME_DURATION).round() as usize; // at least 60ms
        let min_silence_frames = ((min_silence_secs / FRAME_DURATION).round() as usize).max(1); // hold time

        let mut segments: Vec<SpeechSegment> = Vec::new();
        let mut in_speech = false;
        let mut speech_start_frame = 0;
        let mut silence_counter = 0;

        for (frame_idx, &db) in frame_rms_db.iter().enumerate() {
            let is_active = db >= effective_threshold;

            if is_active {
                if !in_speech {
                    in_speech = true;
                    // Pre-roll onset by 40ms to catch soft consonants
                    speech_start_frame = frame_idx.saturating_sub(4);
                }
                silence_counter = 0;
            } else if in_speech {
                silence_counter += 1;
                if silence_counter >= min_silence_frames {
                    let speech_end_frame =
                        frame_idx.saturating_sub(silence_counter).saturating_add(3);
                    if speech_end_frame > speech_start_frame + min_speech_frames {
                        let st = (speech_start_frame as f64 * FRAME_DURATION).max(0.0);
                        let et = (speech_end_frame as f64 * FRAME_DURATION).min(timeline_end);
                        if et > st + 0.08 {
                            segments.push(SpeechSegment { start: st, end: et });
                        }
                    }
                    in_speech = false;
                    silence_counter = 0;
                }
            }
        }

        // Handle trailing speech at the end of the file
        if in_speech {
            let st = (speech_start_frame as f64 * FRAME_DURATION).max(0.0);
            // 프레임 인덱스가 아니라 실제 샘플 길이에서 끝을 잡는다(마지막 부분 프레임 보존)
            let et = timeline_end;
            if et > st + 0.08 {
                segments.push(SpeechSegment { start: st, end: et });
            }
        }

        // Merge adjacent segments that are closer than 180ms
        let mut merged: Vec<SpeechSegment> = Vec::new();
        for seg in segments {
            if let Some(last) = merged.last_mut() {
                if seg.start <= last.end + 0.18 {
                    last.end = seg.end.max(last.end);
                    continue;
                }
            }
            merged.push(seg);
        }

        merged
    }

    /// Calculate phonetic / reading weight of a line
    fn calculate_weight(text: &str) -> f64 {
        let mut weight: f64 = 0.0;
        for c in text.chars() {
            if c.is_alphanumeric() {
                if (c >= '\u{AC00}' && c <= '\u{D7A3}') || (c >= '\u{4E00}' && c <= '\u{9FFF}') {
                    weight += 1.2;
                } else {
                    weight += 0.85;
                }
            } else if c == ',' || c == '.' || c == '!' || c == '?' {
                weight += 0.5;
            } else if !c.is_whitespace() {
                weight += 0.2;
            }
        }
        weight.max(1.0_f64)
    }

    /// 구간 분할 DP: 누적합 `cum`(길이 L+1, 단조 증가) 이 나타내는 L 개 원소를
    /// `targets.len()` 개의 연속 그룹으로 나눈다. 그룹 g 의 비용은
    /// (그룹 합 - targets[g-1])^2 이고, 총비용이 최소인 경계를 돌려준다.
    /// 반환값은 길이 G+1 경계 배열이며 boundaries[0] = 0, boundaries[G] = L, 항상 단조 증가한다.
    ///
    /// ── 왜 분할정복 최적화가 최적해를 보존하는가 ──
    /// 비용 w(k, j) = f(cum[j] - cum[k]), f(x) = (x - t)^2 는 사각부등식(QI)
    ///     w(a,c) + w(b,d) <= w(a,d) + w(b,c)   (a <= b <= c <= d)
    /// 를 만족한다. u = cum[c]-cum[a]-t, v = cum[d]-cum[b]-t, p = cum[d]-cum[a]-t,
    /// q = cum[c]-cum[b]-t 로 두면 u+v = p+q 이고, cum 이 단조라 p >= max(u,v) 이며
    /// q <= min(u,v) 다. 합이 같을 때 더 벌어진 쌍의 제곱합이 크므로(x^2 볼록)
    /// u^2+v^2 <= p^2+q^2, 즉 QI 가 성립한다. QI 가 성립하고 동점일 때 가장 작은 k 를
    /// 고르면 최적 분할점 opt(j) 는 j 에 대해 단조 증가하므로, 분할정복으로 k 범위를
    /// 좁혀도 전수 탐색과 동일한 최소값·동일한 분할점을 얻는다.
    /// 따라서 이것은 후보를 임의로 잘라내는 근사(밴드 제한)가 아니라 최적해 보존 최적화다.
    /// 복잡도: O(L^2 * G) -> O(G * L log L).
    fn partition_dp(cum: &[f64], targets: &[f64]) -> Vec<usize> {
        let total = cum.len().saturating_sub(1);
        let groups = targets.len();

        if groups == 0 || total == 0 {
            return vec![0; groups + 1];
        }
        if groups >= total {
            // 그룹 수가 원소 수 이상이면 1:1 배분뿐이다(호출자는 groups < total 로만 부른다)
            return (0..=groups).map(|g| g.min(total)).collect();
        }

        let mut prev = vec![f64::INFINITY; total + 1];
        let mut curr = vec![f64::INFINITY; total + 1];
        prev[0] = 0.0;

        // parent[g][j]: 그룹 g 까지 원소 j 개를 소비했을 때의 최적 직전 경계.
        // 원소 수가 u32 를 넘는 상황은 물리적으로 불가능하므로 메모리를 아껴 u32 로 둔다.
        let mut parent: Vec<Vec<u32>> = vec![vec![0u32; total + 1]; groups + 1];

        for g in 1..=groups {
            let target = targets[g - 1];
            for v in curr.iter_mut() {
                *v = f64::INFINITY;
            }

            // g 번째 그룹은 최소 1개를 먹으므로 j >= g.
            // prev[k] 가 유한한 k 범위는 g == 1 이면 {0}, 그 밖에는 [g-1, total] 이다.
            // 무한 비용을 탐색 범위에 넣으면 위 단조성 논증(유한 비용 가정)이 깨지므로
            // 반드시 유한 구간으로만 제한한다.
            let k_lo = g - 1;
            let k_hi = if g == 1 { 0 } else { total };

            Self::dc_partition_row(
                &prev,
                &mut curr,
                &mut parent[g],
                cum,
                target,
                g,
                total,
                k_lo,
                k_hi,
            );

            std::mem::swap(&mut prev, &mut curr);
        }

        let mut boundaries = vec![0usize; groups + 1];
        let mut j = total;
        for g in (1..=groups).rev() {
            boundaries[g] = j;
            j = parent[g][j] as usize;
        }
        boundaries[0] = 0;

        boundaries
    }

    /// partition_dp 의 한 행을 분할정복으로 채운다.
    /// j_mid 의 최적 분할점을 구한 뒤, 좌측은 [k_lo, best_k], 우측은 [best_k, k_hi] 로
    /// 범위를 좁힌다(opt 의 단조성 덕분에 최적해를 잃지 않는다).
    #[allow(clippy::too_many_arguments)]
    fn dc_partition_row(
        prev: &[f64],
        curr: &mut [f64],
        parent: &mut [u32],
        cum: &[f64],
        target: f64,
        j_lo: usize,
        j_hi: usize,
        k_lo: usize,
        k_hi: usize,
    ) {
        if j_lo > j_hi {
            return;
        }

        let j_mid = j_lo + (j_hi - j_lo) / 2;
        // 그룹은 최소 1개를 먹으므로 k < j_mid
        let scan_hi = k_hi.min(j_mid.saturating_sub(1));

        let mut best = f64::INFINITY;
        let mut best_k = k_lo;
        for k in k_lo..=scan_hi {
            let diff = (cum[j_mid] - cum[k]) - target;
            let cost = prev[k] + diff * diff;
            // 동점에서는 먼저 만난(가장 작은) k 를 유지한다 — 단조성 논증의 전제
            if cost < best {
                best = cost;
                best_k = k;
            }
        }

        curr[j_mid] = best;
        parent[j_mid] = best_k as u32;

        if j_mid > j_lo {
            Self::dc_partition_row(
                prev,
                curr,
                parent,
                cum,
                target,
                j_lo,
                j_mid - 1,
                k_lo,
                best_k,
            );
        }
        if j_mid < j_hi {
            Self::dc_partition_row(
                prev,
                curr,
                parent,
                cum,
                target,
                j_mid + 1,
                j_hi,
                best_k,
                k_hi,
            );
        }
    }

    /// 구간 [start, end] 을 가중치 비례로 나눈 (시작, 끝) 목록을 만든다.
    /// 항상 단조 증가하고 서로 겹치지 않으며 end 를 넘지 않는다.
    ///
    /// 예전 구현은 250ms 최소 길이를 "끝"에만 적용하고 다음 줄 시작에는 명목 시간을 써서,
    /// 짧은 줄이 다음 줄의 시작을 추월했다(자막 두 개가 동시에 뜨는 사고).
    /// 그래서 최소 슬롯으로 경계를 함께 밀고(앞→뒤), 구간 끝을 고정한 뒤 다시 당긴다(뒤→앞).
    /// min_slot <= span/count 로 잡았기 때문에 두 패스가 항상 동시에 만족 가능하다:
    /// 앞 패스 후 b[t] >= start + t*min_slot 이고 b[count] = start + span >= start + count*min_slot,
    /// 뒤 패스는 b'[t] = min_{u>=t}(b[u] - (u-t)*min_slot) 이므로 b'[t] >= start + t*min_slot 이
    /// 유지되며 모든 간격이 min_slot 이상으로 남는다.
    fn distribute_lines_in_span(start: f64, end: f64, weights: &[f64]) -> Vec<(f64, f64)> {
        let count = weights.len();
        if count == 0 {
            return Vec::new();
        }

        let span = (end - start).max(1e-3);
        let total_w: f64 = weights.iter().filter(|w| w.is_finite() && **w > 0.0).sum();

        // 1) 명목 경계: 가중치 비례
        let mut bounds = Vec::with_capacity(count + 1);
        bounds.push(start);
        let mut acc = 0.0_f64;
        for (idx, w) in weights.iter().enumerate() {
            if total_w > 0.0 && w.is_finite() && *w > 0.0 {
                acc += *w;
            }
            let frac = if total_w > 0.0 {
                acc / total_w
            } else {
                (idx + 1) as f64 / count as f64
            };
            bounds.push(start + span * frac.clamp(0.0, 1.0));
        }

        // 최소 슬롯: 250ms 를 원칙으로 하되 구간이 짧으면 균등 분할값까지 낮춘다
        let min_slot = (span / count as f64).min(0.25);

        // 2) 앞 → 뒤: 최소 슬롯 확보(경계를 함께 밀기)
        for t in 1..=count {
            let pushed = bounds[t - 1] + min_slot;
            if bounds[t] < pushed {
                bounds[t] = pushed;
            }
        }

        // 3) 구간 끝을 고정하고 뒤 → 앞으로 당겨 end 를 절대 넘지 않게 한다
        bounds[count] = start + span;
        for t in (1..count).rev() {
            let pulled = bounds[t + 1] - min_slot;
            if bounds[t] > pulled {
                bounds[t] = pulled;
            }
        }

        (0..count).map(|t| (bounds[t], bounds[t + 1])).collect()
    }

    /// 명시적 시작 시각이 빠진 줄을 이웃 사이에 단조 보간한다.
    /// 예전에는 빠진 줄에 start_offset + i*3.0 을 넣어 앞 줄보다 이른 시각이 나왔고,
    /// 플레이어에서 자막이 뒤로 점프했다. 결과는 항상 엄격히 증가한다.
    fn interpolate_explicit_starts(
        lines: &[ParsedLine],
        total_duration: f64,
        start_offset: f64,
    ) -> Vec<f64> {
        let n = lines.len();
        let mut known: Vec<Option<f64>> = Vec::with_capacity(n);
        let mut prev_known = f64::NEG_INFINITY;
        for line in lines {
            match line.explicit_start {
                Some(v) if v.is_finite() => {
                    // 입력 SRT/LRC 가 뒤섞여 있어도 재생 순서가 깨지지 않게 단조로 정리한다
                    let fixed = v.max(prev_known);
                    prev_known = fixed;
                    known.push(Some(fixed));
                }
                _ => known.push(None),
            }
        }

        let mut filled: Vec<f64> = Vec::with_capacity(n);
        let mut i = 0;
        while i < n {
            if let Some(v) = known[i] {
                filled.push(v);
                i += 1;
                continue;
            }

            // [i, run_end) 이 타임스탬프 없는 구간
            let run_end = (i..n).find(|&k| known[k].is_some()).unwrap_or(n);
            let gap = run_end - i;
            let right = match known.get(run_end).and_then(|v| *v) {
                Some(v) => v,
                None => total_duration.max(0.0),
            };

            if i == 0 {
                // 선행 구간: start_offset 부터 첫 명시 시각 앞까지 균등 배치
                let left = start_offset.min(right);
                let step = (right - left) / gap as f64;
                for t in 0..gap {
                    filled.push(left + step * t as f64);
                }
            } else {
                let left = filled[i - 1];
                let step = (right - left) / (gap + 1) as f64;
                for t in 0..gap {
                    filled.push(left + step * (t + 1) as f64);
                }
            }

            i = run_end;
        }

        // 엄격 증가 강제(같은 타임스탬프가 연속으로 들어오거나 뒤 구간이 좁을 때 방어)
        const EPS: f64 = 0.001;
        if let Some(first) = filled.first_mut() {
            if !first.is_finite() {
                *first = start_offset.max(0.0);
            }
        }
        for idx in 1..n {
            if !filled[idx].is_finite() || filled[idx] <= filled[idx - 1] {
                filled[idx] = filled[idx - 1] + EPS;
            }
        }

        filled
    }

    /// Global Optimal DP Alignment of Script Lines into Precise Speech Timelines with Zero Cumulative Drift
    fn align_script_to_speech(
        lines: &[ParsedLine],
        speech_segments: &[SpeechSegment],
        total_duration: f64,
        start_offset: f64,
        end_margin: f64,
    ) -> Vec<SubtitleItem> {
        let num_lines = lines.len();
        if num_lines == 0 {
            return Vec::new();
        }

        // 명시적 타임스탬프(SRT/LRC)가 하나라도 있으면 그 값을 신뢰하고,
        // 빠진 줄은 이웃 사이에 단조 보간한다.
        if lines.iter().any(|l| l.explicit_start.is_some()) {
            let starts = Self::interpolate_explicit_starts(lines, total_duration, start_offset);

            let mut results = Vec::with_capacity(num_lines);
            for (i, line) in lines.iter().enumerate() {
                let st = starts[i];
                let next_start = starts.get(i + 1).copied();

                let mut et = match line.explicit_end {
                    Some(e) => e,
                    None => match next_start {
                        Some(ns) => (ns - 0.05).max(st + 0.3),
                        None => (st + 2.5).min(total_duration.max(st + 0.3)),
                    },
                };

                // 다음 줄 시작을 절대 넘지 않게 자른다(넘으면 자막 두 개가 동시에 뜬다)
                if let Some(ns) = next_start {
                    et = et.min(ns);
                }
                // NaN 이나 역전된 종료 시각 방어(is_finite 로 걸러야 NaN 이 통과하지 않는다)
                if !et.is_finite() || et <= st {
                    et = match next_start {
                        Some(ns) => st + (ns - st) * 0.9,
                        None => st + 0.3,
                    };
                }

                results.push(SubtitleItem {
                    index: i + 1,
                    start_secs: st,
                    end_secs: et,
                    start_formatted: Self::format_srt_time(st),
                    end_formatted: Self::format_srt_time(et),
                    text: line.text.clone(),
                });
            }
            return results;
        }

        let weights: Vec<f64> = lines
            .iter()
            .map(|l| Self::calculate_weight(&l.text))
            .collect();
        let total_weight: f64 = weights.iter().sum();

        // If valid VAD speech segments were detected, use Dynamic Programming partition
        if !speech_segments.is_empty() {
            let m = speech_segments.len();
            let n = num_lines;

            // Direct 1-to-1 match if count is identical
            if n == m {
                let mut results = Vec::with_capacity(n);
                for (i, seg) in speech_segments.iter().enumerate() {
                    let st = seg.start;
                    let et = (seg.end).max(st + 0.3);
                    results.push(SubtitleItem {
                        index: i + 1,
                        start_secs: st,
                        end_secs: et,
                        start_formatted: Self::format_srt_time(st),
                        end_formatted: Self::format_srt_time(et),
                        text: lines[i].text.clone(),
                    });
                }
                return results;
            }

            // Total speech active time
            let total_speech_time: f64 = speech_segments
                .iter()
                .map(|s| (s.end - s.start).max(0.08))
                .sum();

            if total_speech_time > 0.3 {
                // Case A: M >= N (More speech segments than script lines)
                // Partition M segments into N contiguous groups using DP
                if m >= n {
                    // seg_dur[k] = duration of segment k
                    let seg_durs: Vec<f64> = speech_segments
                        .iter()
                        .map(|s| (s.end - s.start).max(0.08))
                        .collect();
                    let target_durs: Vec<f64> = weights
                        .iter()
                        .map(|w| (w / total_weight) * total_speech_time)
                        .collect();

                    // cum_seg_dur[k] = sum_{0..k} seg_durs
                    let mut cum_seg_dur = vec![0.0; m + 1];
                    for k in 0..m {
                        cum_seg_dur[k + 1] = cum_seg_dur[k] + seg_durs[k];
                    }

                    // 예전에는 셀마다 이전 분할점 전체를 훑어 실제 O(n·m^2) 였고, 세그먼트가
                    // 수백 개만 넘어도 자막 생성이 멈춘 것처럼 보였다. partition_dp 는 같은
                    // 최적해를 O(n·m log m) 으로 구한다(근거는 partition_dp 주석).
                    let boundaries = Self::partition_dp(&cum_seg_dur, &target_durs);

                    let mut results = Vec::with_capacity(n);
                    for i in 0..n {
                        let start_seg_idx = boundaries[i];
                        let end_seg_idx = boundaries[i + 1] - 1;

                        let st = speech_segments[start_seg_idx].start;
                        let seg_end = speech_segments[end_seg_idx].end;
                        // 최소 길이 300ms 를 억지로 주면 다음 자막 시작을 추월한다 → 경계에서 자른다
                        let et = if i + 1 < n {
                            let next_start = speech_segments[boundaries[i + 1]].start;
                            seg_end.max(st + 0.3).min(next_start)
                        } else {
                            seg_end.max(st + 0.3).min(total_duration.max(seg_end))
                        };

                        results.push(SubtitleItem {
                            index: i + 1,
                            start_secs: st,
                            end_secs: et,
                            start_formatted: Self::format_srt_time(st),
                            end_formatted: Self::format_srt_time(et),
                            text: lines[i].text.clone(),
                        });
                    }

                    return results;
                }

                // Case B: M < N (Fewer speech segments than script lines - rapid continuous speech)
                // Partition N script lines into M groups using DP
                if m < n {
                    let seg_durs: Vec<f64> = speech_segments
                        .iter()
                        .map(|s| (s.end - s.start).max(0.08))
                        .collect();
                    let line_target_durs: Vec<f64> = weights
                        .iter()
                        .map(|w| (w / total_weight) * total_speech_time)
                        .collect();

                    let mut cum_line_dur = vec![0.0; n + 1];
                    for k in 0..n {
                        cum_line_dur[k + 1] = cum_line_dur[k] + line_target_durs[k];
                    }

                    // 대본 n 줄을 세그먼트 m 개에 연속 그룹으로 배분한다(O(m·n log n))
                    let boundaries = Self::partition_dp(&cum_line_dur, &seg_durs);

                    let mut results = Vec::with_capacity(n);
                    let mut prev_end = f64::NEG_INFINITY;
                    for j in 0..m {
                        let start_line_idx = boundaries[j];
                        let end_line_idx = boundaries[j + 1];
                        let seg = &speech_segments[j];

                        // 세그먼트 경계를 단조 증가로 고정한다(뒤 세그먼트가 앞 자막 끝을 추월하지 않게)
                        let span_start = seg.start.max(prev_end);
                        let span_end = seg.end.max(span_start + 0.05);

                        let slots = Self::distribute_lines_in_span(
                            span_start,
                            span_end,
                            &weights[start_line_idx..end_line_idx],
                        );

                        for (offset, (item_start, item_end)) in slots.into_iter().enumerate() {
                            results.push(SubtitleItem {
                                index: results.len() + 1,
                                start_secs: item_start,
                                end_secs: item_end,
                                start_formatted: Self::format_srt_time(item_start),
                                end_formatted: Self::format_srt_time(item_end),
                                text: lines[start_line_idx + offset].text.clone(),
                            });
                            prev_end = item_end;
                        }
                    }

                    return results;
                }
            }
        }

        // Fallback: Proportional allocation across duration
        let effective_start = start_offset;
        let effective_end = (total_duration - end_margin).max(effective_start + 0.5);
        let available_duration = effective_end - effective_start;
        let pause_between = (0.08_f64).min(available_duration / (num_lines as f64 * 4.0));
        let active_duration = (available_duration - (num_lines as f64 * pause_between)).max(0.5);

        let mut results = Vec::with_capacity(num_lines);
        let mut current_start = effective_start;

        for (i, line) in lines.iter().enumerate() {
            let weight = weights[i];
            let item_dur = (weight / total_weight) * active_duration;
            let item_start = current_start;
            let item_end = (item_start + item_dur).max(item_start + 0.3);

            results.push(SubtitleItem {
                index: i + 1,
                start_secs: item_start,
                end_secs: item_end,
                start_formatted: Self::format_srt_time(item_start),
                end_formatted: Self::format_srt_time(item_end),
                text: line.text.clone(),
            });

            current_start = item_end + pause_between;
        }

        results
    }

    /// Format seconds to SRT format `HH:MM:SS,mmm`
    pub fn format_srt_time(secs: f64) -> String {
        let total_millis = (secs.max(0.0) * 1000.0).round() as u64;
        let ms = total_millis % 1000;
        let s = (total_millis / 1000) % 60;
        let m = (total_millis / (1000 * 60)) % 60;
        let h = total_millis / (1000 * 60 * 60);

        format!("{:02}:{:02}:{:02},{:03}", h, m, s, ms)
    }

    /// Format seconds to WebVTT format `HH:MM:SS.mmm`
    pub fn format_vtt_time(secs: f64) -> String {
        let total_millis = (secs.max(0.0) * 1000.0).round() as u64;
        let ms = total_millis % 1000;
        let s = (total_millis / 1000) % 60;
        let m = (total_millis / (1000 * 60)) % 60;
        let h = total_millis / (1000 * 60 * 60);

        format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
    }

    /// Build standard SRT subtitle string
    pub fn build_srt_string(items: &[SubtitleItem]) -> String {
        let mut srt = String::new();
        for item in items {
            srt.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                item.index,
                Self::format_srt_time(item.start_secs),
                Self::format_srt_time(item.end_secs),
                item.text
            ));
        }
        srt
    }

    /// Build standard WebVTT subtitle string
    pub fn build_vtt_string(items: &[SubtitleItem]) -> String {
        let mut vtt = String::from("WEBVTT\n\n");
        for item in items {
            vtt.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                item.index,
                Self::format_vtt_time(item.start_secs),
                Self::format_vtt_time(item.end_secs),
                item.text
            ));
        }
        vtt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_time() {
        assert_eq!(SubtitleController::format_srt_time(1.234), "00:00:01,234");
        assert_eq!(SubtitleController::format_vtt_time(1.234), "00:00:01.234");
        assert_eq!(
            SubtitleController::format_srt_time(3661.050),
            "01:01:01,050"
        );
    }

    #[test]
    fn test_split_and_clean_script() {
        let script = "안녕하세요! 반갑습니다.\nOmniRec 자막 생성기입니다.";
        let lines = SubtitleController::split_and_clean_script(script, "sentence", 30, false);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "안녕하세요!");
        assert_eq!(lines[1].text, "반갑습니다.");
        assert_eq!(lines[2].text, "OmniRec 자막 생성기입니다.");
    }

    #[test]
    fn test_align_script_fallback() {
        let lines = vec![
            ParsedLine {
                text: "첫 번째 문장".to_string(),
                explicit_start: None,
                explicit_end: None,
            },
            ParsedLine {
                text: "두 번째 문장".to_string(),
                explicit_start: None,
                explicit_end: None,
            },
        ];
        let subs = SubtitleController::align_script_to_speech(&lines, &[], 10.0, 0.1, 0.2);
        assert_eq!(subs.len(), 2);
        assert!(subs[0].start_secs >= 0.1);
        assert!(subs[0].end_secs < subs[1].start_secs);
        assert!(subs[1].end_secs <= 10.0);
    }

    #[test]
    fn test_srt_script_drops_index_lines() {
        // SRT 를 대본으로 넣으면 숫자 인덱스 줄이 낭독 자막으로 새어 들어갔다
        let script = "1\n00:00:01,000 --> 00:00:02,500\n첫 번째 자막\n\n2\n00:00:03,000 --> 00:00:04,000\n두 번째 자막\n";
        let lines = SubtitleController::split_and_clean_script(script, "auto", 30, false);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "첫 번째 자막");
        assert_eq!(lines[1].text, "두 번째 자막");
        assert_eq!(lines[0].explicit_start, Some(1.0));
        assert_eq!(lines[0].explicit_end, Some(2.5));
        assert_eq!(lines[1].explicit_start, Some(3.0));
        assert_eq!(lines[1].explicit_end, Some(4.0));
    }

    #[test]
    fn test_explicit_timestamps_interpolate_missing() {
        // 타임스탬프가 빠진 줄은 이웃 사이에 보간되어 단조성이 유지되어야 한다
        let lines = vec![
            ParsedLine {
                text: "가".to_string(),
                explicit_start: Some(1.0),
                explicit_end: None,
            },
            ParsedLine {
                text: "나".to_string(),
                explicit_start: None,
                explicit_end: None,
            },
            ParsedLine {
                text: "다".to_string(),
                explicit_start: Some(5.0),
                explicit_end: None,
            },
        ];
        let subs = SubtitleController::align_script_to_speech(&lines, &[], 20.0, 0.1, 0.2);

        assert_eq!(subs.len(), 3);
        assert!(subs[1].start_secs > 1.0 && subs[1].start_secs < 5.0);
        for i in 0..subs.len() {
            assert!(subs[i].start_secs < subs[i].end_secs, "빈 자막 #{}", i);
            if i + 1 < subs.len() {
                assert!(
                    subs[i].start_secs < subs[i + 1].start_secs,
                    "비단조 시작 #{}",
                    i
                );
                assert!(
                    subs[i].end_secs <= subs[i + 1].start_secs + 1e-9,
                    "자막 겹침 #{}",
                    i
                );
            }
        }
    }

    #[test]
    fn test_split_by_char_length_respects_cjk_limit() {
        // 공백이 없는 한국어 문장도 max_chars 를 지켜야 한다
        let text = "가나다라마바사아자차카타파하가나다라마바사"; // 21자, 공백 없음
        let chunks = SubtitleController::split_by_char_length(text, 8);

        assert!(chunks.len() >= 3, "chunks = {:?}", chunks);
        for c in &chunks {
            assert!(c.chars().count() <= 8, "제한 초과: {}", c);
        }
        assert_eq!(chunks.concat(), text);

        // 파이프라인(길이 모드)에서도 동일하게 지켜지는지 확인
        let lines = SubtitleController::split_and_clean_script(text, "length", 8, false);
        assert!(!lines.is_empty());
        for l in &lines {
            assert!(l.text.chars().count() <= 8, "제한 초과: {}", l.text);
        }
    }

    #[test]
    fn test_align_fewer_segments_than_lines_is_monotonic() {
        // 세그먼트(2) < 대본 줄(7): 250ms 최소 길이 강제 때문에 자막이 서로 겹쳤다
        let lines: Vec<ParsedLine> = (0..7)
            .map(|i| ParsedLine {
                text: format!("문장 {}", i),
                explicit_start: None,
                explicit_end: None,
            })
            .collect();
        let segs = vec![
            SpeechSegment {
                start: 0.5,
                end: 1.0,
            },
            SpeechSegment {
                start: 1.5,
                end: 2.2,
            },
        ];

        let subs = SubtitleController::align_script_to_speech(&lines, &segs, 3.0, 0.1, 0.2);

        assert_eq!(subs.len(), 7);
        for (i, s) in subs.iter().enumerate() {
            assert!(s.start_secs < s.end_secs, "빈 자막 #{}", i);
            assert!(s.start_secs >= 0.5 - 1e-9, "구간 앞으로 벗어남 #{}", i);
            assert!(
                s.end_secs <= 2.2 + 1e-9,
                "구간 뒤로 벗어남 #{}: {}",
                i,
                s.end_secs
            );
            if i + 1 < subs.len() {
                assert!(
                    s.end_secs <= subs[i + 1].start_secs + 1e-9,
                    "자막 겹침 #{}: {} > {}",
                    i,
                    s.end_secs,
                    subs[i + 1].start_secs
                );
            }
        }
    }

    #[test]
    fn test_vad_includes_trailing_partial_frame() {
        // 16080 샘플 = 1.005초. 160 샘플 프레임으로는 마지막 0.5 프레임이 남는다.
        let samples = vec![0.5f32; 16_080];
        let segs =
            SubtitleController::detect_speech_segments_from_samples(&samples, 2.0, -35.0, 0.25);

        assert_eq!(segs.len(), 1);
        let end = segs[0].end;
        // 예전 구현은 프레임 수 내림(100프레임 = 1.000초)으로 끝을 잘랐다
        assert!(end > 1.0, "마지막 부분 프레임이 버려졌다: {}", end);
        assert!((end - 16_080.0 / 16_000.0).abs() < 1e-9, "end = {}", end);
    }

    #[test]
    fn test_vad_survives_nan_samples() {
        // NaN 이 섞여도 정렬 패닉·임계값 NaN 전파가 없어야 한다
        let mut samples = vec![0.0f32; 1_600];
        samples.resize(9_600, 0.4f32);
        samples[10] = f32::NAN;
        samples[2_000] = f32::NAN;

        let segs =
            SubtitleController::detect_speech_segments_from_samples(&samples, 0.6, -35.0, f64::NAN);
        for s in &segs {
            assert!(s.start.is_finite() && s.end.is_finite());
            assert!(s.start < s.end);
        }
    }

    fn partition_cost(cum: &[f64], targets: &[f64], boundaries: &[usize]) -> f64 {
        let mut cost = 0.0;
        for g in 0..targets.len() {
            let d = cum[boundaries[g + 1]] - cum[boundaries[g]] - targets[g];
            cost += d * d;
        }
        cost
    }

    /// 최적성 비교 기준: 원래 구현과 동일한 전수 DP O(L^2 * G)
    fn brute_force_partition(cum: &[f64], targets: &[f64]) -> Vec<usize> {
        let total = cum.len() - 1;
        let groups = targets.len();
        let mut dp = vec![vec![f64::INFINITY; total + 1]; groups + 1];
        let mut parent = vec![vec![0usize; total + 1]; groups + 1];
        dp[0][0] = 0.0;

        for g in 1..=groups {
            for j in g..=total {
                for k in (g - 1)..j {
                    if dp[g - 1][k].is_infinite() {
                        continue;
                    }
                    let d = cum[j] - cum[k] - targets[g - 1];
                    let cost = dp[g - 1][k] + d * d;
                    if cost < dp[g][j] {
                        dp[g][j] = cost;
                        parent[g][j] = k;
                    }
                }
            }
        }

        let mut boundaries = vec![0usize; groups + 1];
        let mut j = total;
        for g in (1..=groups).rev() {
            boundaries[g] = j;
            j = parent[g][j];
        }
        boundaries[0] = 0;
        boundaries
    }

    fn lcg(state: &mut u64) -> f64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*state >> 33) as f64) / ((1u64 << 31) as f64)
    }

    #[test]
    fn test_partition_dp_matches_brute_force() {
        // 분할정복 최적화가 전수 탐색과 동일한 최적해를 주는지 확인(결정적 LCG 데이터)
        let mut seed = 12_345u64;
        for (total, groups) in [
            (1usize, 1usize),
            (5, 1),
            (5, 5),
            (9, 3),
            (12, 4),
            (20, 7),
            (31, 11),
            (40, 39),
        ] {
            let mut cum = vec![0.0; total + 1];
            for k in 0..total {
                cum[k + 1] = cum[k] + 0.08 + lcg(&mut seed) * 2.0;
            }
            let targets: Vec<f64> = (0..groups).map(|_| 0.1 + lcg(&mut seed) * 3.0).collect();

            let fast = SubtitleController::partition_dp(&cum, &targets);
            let brute = brute_force_partition(&cum, &targets);

            assert_eq!(fast.len(), groups + 1);
            assert_eq!(fast[0], 0);
            assert_eq!(fast[groups], total);
            for g in 1..=groups {
                assert!(fast[g] > fast[g - 1], "비단조 경계: {:?}", fast);
            }

            let cost_fast = partition_cost(&cum, &targets, &fast);
            let cost_brute = partition_cost(&cum, &targets, &brute);
            assert!(
                (cost_fast - cost_brute).abs() < 1e-9,
                "total={} groups={} fast={} brute={}",
                total,
                groups,
                cost_fast,
                cost_brute
            );
        }
    }

    #[test]
    fn test_distribute_lines_in_span_never_overlaps() {
        // 짧은 구간에 많은 줄이 들어가도 단조·비겹침·구간 내 유지
        let weights = vec![9.0, 1.0, 1.0, 1.0, 5.0, 1.0];
        let slots = SubtitleController::distribute_lines_in_span(0.5, 1.0, &weights);

        assert_eq!(slots.len(), weights.len());
        for (i, (s, e)) in slots.iter().enumerate() {
            assert!(s < e, "빈 슬롯 #{}: {} .. {}", i, s, e);
            assert!(
                *s >= 0.5 - 1e-9 && *e <= 1.0 + 1e-9,
                "구간 이탈 #{}: {} .. {}",
                i,
                s,
                e
            );
            if i + 1 < slots.len() {
                assert!(*e <= slots[i + 1].0 + 1e-9, "겹침 #{}", i);
            }
        }
        assert!((slots[weights.len() - 1].1 - 1.0).abs() < 1e-9);
    }
}
