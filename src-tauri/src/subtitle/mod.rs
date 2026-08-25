use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use regex::Regex;

use crate::settings::SettingsManager;
use crate::types::{SubtitleGenerateResult, SubtitleGenerateTask, SubtitleItem};

pub struct SubtitleController;

#[derive(Debug, Clone)]
struct SpeechSegment {
    start: f64,
    end: f64,
}

impl SubtitleController {
    pub fn new() -> Self {
        Self
    }

    /// Read text file content with utf-8 encoding fallback
    pub fn read_script_file(path: &str) -> Result<String, String> {
        let bytes = fs::read(path).map_err(|e| format!("Failed to read script file: {}", e))?;
        
        // Try UTF-8 first
        if let Ok(s) = String::from_utf8(bytes.clone()) {
            return Ok(s);
        }

        // Try lossy utf-8
        let s = String::from_utf8_lossy(&bytes).to_string();
        Ok(s)
    }

    /// Save subtitle string content to a file
    pub fn save_subtitle_file(path: &str, content: &str) -> Result<(), String> {
        let p = Path::new(path);
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, content).map_err(|e| format!("Failed to save subtitle file: {}", e))
    }

    /// Main entry point: generate subtitles from audio file and script text
    pub fn generate(
        task: SubtitleGenerateTask,
        custom_ffmpeg_path: Option<String>,
    ) -> Result<SubtitleGenerateResult, String> {
        let audio_path = PathBuf::from(&task.audio_path);
        if !audio_path.exists() {
            return Err(format!("Audio file does not exist: {}", task.audio_path));
        }

        // 1. Probe total duration of audio
        let duration = Self::get_audio_duration(&audio_path, custom_ffmpeg_path.as_deref())
            .unwrap_or(0.0);

        if duration <= 0.0 {
            return Err("Failed to get audio duration or audio duration is 0 seconds.".to_string());
        }

        // 2. Parse script lines
        let script_lines = Self::split_and_clean_script(
            &task.script_text,
            &task.split_mode,
            task.max_chars.max(5),
        );

        if script_lines.is_empty() {
            return Err("No valid text lines found in script.".to_string());
        }

        // 3. Detect silence / speech segments via FFmpeg
        let min_silence = task.min_silence_duration_secs.unwrap_or(0.25).max(0.05);
        let silence_thresh = task.silence_threshold_db.unwrap_or(-35.0);
        let speech_segments = Self::detect_speech_segments(
            &audio_path,
            duration,
            silence_thresh,
            min_silence,
            custom_ffmpeg_path.as_deref(),
        );

        let speech_segments_detected = speech_segments.len();

        // 4. Align script lines to audio timeline
        let start_offset = task.start_offset_secs.unwrap_or(0.1).max(0.0);
        let end_margin = task.end_margin_secs.unwrap_or(0.2).max(0.0);

        let subtitles = Self::align_script_to_speech(
            &script_lines,
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

            let _ = fs::create_dir_all(&target_dir);

            let srt_file = target_dir.join(format!("{}.srt", stem));
            let vtt_file = target_dir.join(format!("{}.vtt", stem));

            if fs::write(&srt_file, &srt_content).is_ok() {
                srt_path_saved = Some(srt_file.to_string_lossy().to_string());
            }
            if fs::write(&vtt_file, &vtt_content).is_ok() {
                vtt_path_saved = Some(vtt_file.to_string_lossy().to_string());
            }
        }

        Ok(SubtitleGenerateResult {
            subtitles,
            srt_content,
            vtt_content,
            srt_path: srt_path_saved,
            vtt_path: vtt_path_saved,
            total_duration: duration,
            speech_segments_detected,
            script_lines_count: script_lines.len(),
        })
    }

    /// Split raw script into chunks according to splitting mode and max character limit
    fn split_and_clean_script(raw: &str, mode: &str, max_chars: usize) -> Vec<String> {
        // First check if script already has timestamp headers like [00:01.00]
        let timestamp_regex = Regex::new(r"\[\d{1,2}:\d{2}(?:\.\d+)?\]").unwrap();
        let mut cleaned_lines = Vec::new();

        let raw_clean = raw.replace("\r\n", "\n").replace('\r', "\n");
        let raw_lines: Vec<&str> = raw_clean.lines().collect();

        for line in raw_lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Remove timestamps if present in raw text lines
            let no_ts = timestamp_regex.replace_all(trimmed, "").trim().to_string();
            if no_ts.is_empty() {
                continue;
            }

            match mode {
                "line" => {
                    cleaned_lines.push(no_ts);
                }
                "sentence" => {
                    let sentences = Self::split_into_sentences(&no_ts);
                    for s in sentences {
                        if !s.is_empty() {
                            cleaned_lines.push(s);
                        }
                    }
                }
                "length" => {
                    let chunks = Self::split_by_char_length(&no_ts, max_chars);
                    for c in chunks {
                        if !c.is_empty() {
                            cleaned_lines.push(c);
                        }
                    }
                }
                _ => {
                    // "auto": sentence split + length check
                    let sentences = Self::split_into_sentences(&no_ts);
                    for s in sentences {
                        if s.chars().count() > max_chars {
                            let chunks = Self::split_by_char_length(&s, max_chars);
                            for c in chunks {
                                if !c.is_empty() {
                                    cleaned_lines.push(c);
                                }
                            }
                        } else if !s.is_empty() {
                            cleaned_lines.push(s);
                        }
                    }
                }
            }
        }

        cleaned_lines
    }

    /// Split string into sentences by punctuation (. ? ! \n)
    fn split_into_sentences(text: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();

        for (i, &ch) in chars.iter().enumerate() {
            current.push(ch);

            if ch == '.' || ch == '?' || ch == '!' || ch == '…' || ch == '。' {
                // Peek next char to avoid breaking decimals like 3.14 or abbreviations
                let next_is_space_or_end = if i + 1 < len {
                    chars[i + 1].is_whitespace()
                } else {
                    true
                };

                if next_is_space_or_end {
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        result.push(trimmed);
                    }
                    current.clear();
                }
            }
        }

        let remaining = current.trim().to_string();
        if !remaining.is_empty() {
            result.push(remaining);
        }

        result
    }

    /// Split long text by character length while preserving word boundaries (spaces/punctuation)
    fn split_by_char_length(text: &str, max_chars: usize) -> Vec<String> {
        if text.chars().count() <= max_chars {
            return vec![text.to_string()];
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return vec![];
        }

        let mut chunks = Vec::new();
        let mut current_chunk = String::new();

        for word in words {
            let potential_len = if current_chunk.is_empty() {
                word.chars().count()
            } else {
                current_chunk.chars().count() + 1 + word.chars().count()
            };

            if potential_len > max_chars && !current_chunk.is_empty() {
                chunks.push(current_chunk.clone());
                current_chunk = word.to_string();
            } else {
                if !current_chunk.is_empty() {
                    current_chunk.push(' ');
                }
                current_chunk.push_str(word);
            }
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        chunks
    }

    /// Probe duration of audio/video using ffprobe or ffmpeg
    fn get_audio_duration(path: &Path, custom_ffmpeg_path: Option<&str>) -> Option<f64> {
        if let Ok(ffprobe_path) = SettingsManager::find_ffprobe(custom_ffmpeg_path) {
            let mut cmd = Command::new(ffprobe_path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
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

        // Fallback: use ffmpeg -i path
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

    /// Detect speech intervals using ffmpeg silencedetect filter
    fn detect_speech_segments(
        path: &Path,
        total_duration: f64,
        silence_thresh_db: f64,
        min_silence_secs: f64,
        custom_ffmpeg_path: Option<&str>,
    ) -> Vec<SpeechSegment> {
        let mut segments = Vec::new();
        let ffmpeg_path = match SettingsManager::find_ffmpeg(custom_ffmpeg_path) {
            Ok(p) => p,
            Err(_) => return segments,
        };

        let filter_arg = format!(
            "silencedetect=noise={}dB:d={}",
            silence_thresh_db, min_silence_secs
        );

        let mut cmd = Command::new(ffmpeg_path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        cmd.args([
            "-vn",
            "-i",
            path.to_str().unwrap_or_default(),
            "-af",
            &filter_arg,
            "-f",
            "null",
            "-",
        ]);

        let output = match cmd.output() {
            Ok(o) => o,
            Err(_) => return segments,
        };

        let stderr_str = String::from_utf8_lossy(&output.stderr);

        // Parse silence_start: X and silence_end: Y
        let re_silence_start = Regex::new(r"silence_start:\s*(\d+(?:\.\d+)?)").unwrap();
        let re_silence_end = Regex::new(r"silence_end:\s*(\d+(?:\.\d+)?)").unwrap();

        let mut silences: Vec<(f64, f64)> = Vec::new();
        let mut current_silence_start: Option<f64> = None;

        for line in stderr_str.lines() {
            if let Some(caps) = re_silence_start.captures(line) {
                if let Ok(st) = caps[1].parse::<f64>() {
                    current_silence_start = Some(st);
                }
            }
            if let Some(caps) = re_silence_end.captures(line) {
                if let Ok(end) = caps[1].parse::<f64>() {
                    let start = current_silence_start.unwrap_or(0.0);
                    silences.push((start, end));
                    current_silence_start = None;
                }
            }
        }

        // Convert silences into speech segments
        let mut last_speech_start = 0.0;
        for (silence_start, silence_end) in silences {
            if silence_start > last_speech_start + 0.15 {
                segments.push(SpeechSegment {
                    start: last_speech_start,
                    end: silence_start,
                });
            }
            last_speech_start = silence_end;
        }

        if last_speech_start < total_duration - 0.15 {
            segments.push(SpeechSegment {
                start: last_speech_start,
                end: total_duration,
            });
        }

        segments
    }

    /// Calculate phonetic / reading weight of a line
    fn calculate_weight(text: &str) -> f64 {
        let mut weight: f64 = 0.0;
        for c in text.chars() {
            if c.is_alphanumeric() {
                // Hangul or CJK characters take slightly more reading time
                if (c >= '\u{AC00}' && c <= '\u{D7A3}') || (c >= '\u{4E00}' && c <= '\u{9FFF}') {
                    weight += 1.2;
                } else {
                    weight += 0.8;
                }
            } else if c == ',' || c == '.' || c == '!' || c == '?' {
                weight += 0.6;
            } else {
                weight += 0.2;
            }
        }
        weight.max(1.0_f64)
    }

    /// Intelligent alignment of script lines into speech intervals
    fn align_script_to_speech(
        lines: &[String],
        speech_segments: &[SpeechSegment],
        total_duration: f64,
        start_offset: f64,
        end_margin: f64,
    ) -> Vec<SubtitleItem> {
        let num_lines = lines.len();
        if num_lines == 0 {
            return Vec::new();
        }

        let weights: Vec<f64> = lines.iter().map(|l| Self::calculate_weight(l)).collect();
        let total_weight: f64 = weights.iter().sum();

        let effective_start = start_offset;
        let effective_end = (total_duration - end_margin).max(effective_start + 0.5);

        let mut results = Vec::with_capacity(num_lines);

        // Case A: We have valid detected speech segments
        if !speech_segments.is_empty() && speech_segments.len() >= (num_lines / 3).max(1) {
            // Allocate lines to speech segments based on cumulative weights

            // Compute total speech duration
            let total_speech_time: f64 = speech_segments
                .iter()
                .map(|s| (s.end - s.start).max(0.1))
                .sum();

            if total_speech_time > 0.5 {
                let mut line_idx = 0;
                let mut current_line_weight_progress = 0.0;

                for seg in speech_segments {
                    let seg_duration = (seg.end - seg.start).max(0.1);
                    let seg_weight_quota = (seg_duration / total_speech_time) * total_weight;

                    let mut seg_lines: Vec<(usize, f64)> = Vec::new();
                    let mut accumulated_in_seg = 0.0;

                    while line_idx < num_lines {
                        let line_w = weights[line_idx];
                        let needed = line_w - current_line_weight_progress;

                        if accumulated_in_seg + needed <= seg_weight_quota * 1.15
                            || seg_lines.is_empty()
                            || line_idx == num_lines - 1
                        {
                            seg_lines.push((line_idx, needed));
                            accumulated_in_seg += needed;
                            line_idx += 1;
                            current_line_weight_progress = 0.0;
                            if accumulated_in_seg >= seg_weight_quota {
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    if !seg_lines.is_empty() {
                        let seg_total_w: f64 = seg_lines.iter().map(|(_, w)| *w).sum();
                        let mut curr_t = seg.start;

                        for (idx, w) in seg_lines {
                            let item_dur = (w / seg_total_w) * seg_duration;
                            let item_start = curr_t;
                            let item_end = (curr_t + item_dur - 0.05).max(item_start + 0.2);
                            curr_t += item_dur;

                            results.push(SubtitleItem {
                                index: results.len() + 1,
                                start_secs: item_start,
                                end_secs: item_end,
                                start_formatted: Self::format_srt_time(item_start),
                                end_formatted: Self::format_srt_time(item_end),
                                text: lines[idx].clone(),
                            });
                        }
                    }
                }

                // If any remaining lines not placed, evenly allocate in the end
                while line_idx < num_lines {
                    let last_end = results
                        .last()
                        .map(|r| r.end_secs + 0.08)
                        .unwrap_or(effective_start);
                    let end_t = (last_end + 1.5).min(total_duration);
                    results.push(SubtitleItem {
                        index: results.len() + 1,
                        start_secs: last_end,
                        end_secs: end_t,
                        start_formatted: Self::format_srt_time(last_end),
                        end_formatted: Self::format_srt_time(end_t),
                        text: lines[line_idx].clone(),
                    });
                    line_idx += 1;
                }

                return results;
            }
        }

        // Case B: Fallback proportional weight allocation across duration
        let available_duration = effective_end - effective_start;
        let pause_between = (0.1_f64).min(available_duration / (num_lines as f64 * 3.0));
        let active_duration = (available_duration - (num_lines as f64 * pause_between)).max(0.5);

        let mut current_start = effective_start;

        for (i, line) in lines.iter().enumerate() {
            let weight = weights[i];
            let item_dur = (weight / total_weight) * active_duration;
            let item_start = current_start;
            let item_end = item_start + item_dur.max(0.3);

            results.push(SubtitleItem {
                index: i + 1,
                start_secs: item_start,
                end_secs: item_end,
                start_formatted: Self::format_srt_time(item_start),
                end_formatted: Self::format_srt_time(item_end),
                text: line.clone(),
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
        assert_eq!(SubtitleController::format_srt_time(3661.050), "01:01:01,050");
    }

    #[test]
    fn test_split_and_clean_script() {
        let script = "안녕하세요! 반갑습니다.\nOmniRec 자막 생성기입니다.";
        let lines = SubtitleController::split_and_clean_script(script, "sentence", 30);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "안녕하세요!");
        assert_eq!(lines[1], "반갑습니다.");
        assert_eq!(lines[2], "OmniRec 자막 생성기입니다.");
    }

    #[test]
    fn test_align_script_fallback() {
        let lines = vec!["첫 번째 문장".to_string(), "두 번째 문장".to_string()];
        let subs = SubtitleController::align_script_to_speech(&lines, &[], 10.0, 0.1, 0.2);
        assert_eq!(subs.len(), 2);
        assert!(subs[0].start_secs >= 0.1);
        assert!(subs[0].end_secs < subs[1].start_secs);
        assert!(subs[1].end_secs <= 10.0);
    }
}

