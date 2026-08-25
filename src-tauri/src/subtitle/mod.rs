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
            return Err(format!("오디오 파일이 존재하지 않습니다: {}", task.audio_path));
        }

        // 1. Probe total duration of audio
        let duration = Self::get_audio_duration(&audio_path, custom_ffmpeg_path.as_deref())
            .unwrap_or(0.0);

        if duration <= 0.0 {
            return Err("오디오 길이를 측정할 수 없거나 재생 시간이 0초입니다.".to_string());
        }

        // 2. Parse script lines
        let parsed_lines = Self::split_and_clean_script(
            &task.script_text,
            &task.split_mode,
            task.max_chars.max(5),
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
            script_lines_count: parsed_lines.len(),
        })
    }

    /// Split raw script into chunks according to splitting mode and max character limit
    fn split_and_clean_script(raw: &str, mode: &str, max_chars: usize) -> Vec<ParsedLine> {
        let lrc_regex = Regex::new(r"\[(\d{1,2}):(\d{2})(?:\.(\d+))?\]").unwrap();
        let srt_time_regex = Regex::new(r"(\d{2}:\d{2}:\d{2}[,\.]\d{3})\s*-->\s*(\d{2}:\d{2}:\d{2}[,\.]\d{3})").unwrap();

        let mut parsed_results = Vec::new();

        let raw_clean = raw.replace("\r\n", "\n").replace('\r', "\n");
        let raw_lines: Vec<&str> = raw_clean.lines().collect();

        let mut i = 0;
        while i < raw_lines.len() {
            let line = raw_lines[i].trim();
            if line.is_empty() {
                i += 1;
                continue;
            }

            // Check if this is an SRT block: index line followed by timestamp line
            if let Some(caps) = srt_time_regex.captures(line) {
                let st = Self::parse_time_str(&caps[1]);
                let et = Self::parse_time_str(&caps[2]);
                i += 1;
                let mut text_acc = String::new();
                while i < raw_lines.len() && !raw_lines[i].trim().is_empty() {
                    if !text_acc.is_empty() {
                        text_acc.push(' ');
                    }
                    text_acc.push_str(raw_lines[i].trim());
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
                let frac: f64 = caps.get(3).map(|m| format!("0.{}", m.as_str()).parse().unwrap_or(0.0)).unwrap_or(0.0);
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
                    let sentences = Self::split_into_sentences(line);
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
                    // "auto": split sentences first, then length
                    let sentences = Self::split_into_sentences(line);
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

    /// Split string into sentences by punctuation (. ? ! … 。)
    fn split_into_sentences(text: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();

        for (i, &ch) in chars.iter().enumerate() {
            current.push(ch);

            if ch == '.' || ch == '?' || ch == '!' || ch == '…' || ch == '。' {
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

    /// Split long text by character length while preserving word boundaries
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

    /// High-precision Voice Activity Detection by decoding audio to 16kHz mono PCM
    fn detect_speech_segments_pcm(
        path: &Path,
        total_duration: f64,
        user_thresh_db: f64,
        min_silence_secs: f64,
        custom_ffmpeg_path: Option<&str>,
    ) -> Vec<SpeechSegment> {
        let ffmpeg_path = match SettingsManager::find_ffmpeg(custom_ffmpeg_path) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        // Decode directly to 16kHz mono raw float32 PCM
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
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "pipe:1",
        ]);

        let output = match cmd.output() {
            Ok(o) if o.status.success() || !o.stdout.is_empty() => o,
            _ => return Vec::new(),
        };

        let raw_bytes = output.stdout;
        if raw_bytes.len() < 4 * 1600 {
            return Vec::new();
        }

        // Convert u8 slice to f32 samples
        let samples: Vec<f32> = raw_bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        if samples.is_empty() {
            return Vec::new();
        }

        const FRAME_SIZE: usize = 160; // 10ms per frame at 16kHz
        const FRAME_DURATION: f64 = 0.01;

        let num_frames = samples.len() / FRAME_SIZE;
        let mut frame_rms_db = Vec::with_capacity(num_frames);

        for i in 0..num_frames {
            let start = i * FRAME_SIZE;
            let end = start + FRAME_SIZE;
            let slice = &samples[start..end];

            let mut sum_sq = 0.0;
            for &s in slice {
                sum_sq += s * s;
            }
            let rms = (sum_sq / FRAME_SIZE as f32).sqrt();
            let db = if rms > 1e-6 {
                20.0 * rms.log10()
            } else {
                -90.0
            };
            frame_rms_db.push(db);
        }

        // Estimate noise floor and peak speech energy dynamically
        let mut sorted_db = frame_rms_db.clone();
        sorted_db.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let noise_floor_idx = (num_frames as f64 * 0.15) as usize;
        let speech_peak_idx = (num_frames as f64 * 0.90) as usize;

        let noise_floor = sorted_db.get(noise_floor_idx).copied().unwrap_or(-50.0);
        let speech_peak = sorted_db.get(speech_peak_idx).copied().unwrap_or(-15.0);

        // Adaptive threshold: between noise floor and peak, clamped to reasonable range
        let dynamic_threshold = (noise_floor + (speech_peak - noise_floor) * 0.35)
            .max(-55.0)
            .min(-22.0);

        // Combine with user threshold
        let effective_threshold = if user_thresh_db < -10.0 && user_thresh_db > -70.0 {
            (dynamic_threshold * 0.7) + (user_thresh_db as f32 * 0.3)
        } else {
            dynamic_threshold
        };

        let min_speech_frames = (0.06 / FRAME_DURATION).round() as usize; // at least 60ms
        let min_silence_frames = (min_silence_secs / FRAME_DURATION).round() as usize; // hold time

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
                    let speech_end_frame = frame_idx.saturating_sub(silence_counter).saturating_add(3);
                    if speech_end_frame > speech_start_frame + min_speech_frames {
                        let st = (speech_start_frame as f64 * FRAME_DURATION).max(0.0);
                        let et = (speech_end_frame as f64 * FRAME_DURATION).min(total_duration);
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
            let et = (num_frames as f64 * FRAME_DURATION).min(total_duration);
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

        // Check if all lines have explicit timestamps
        let has_explicit = lines.iter().any(|l| l.explicit_start.is_some());
        if has_explicit {
            let mut results = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let st = line.explicit_start.unwrap_or(start_offset + (i as f64 * 3.0));
                let et = line.explicit_end.unwrap_or_else(|| {
                    if i + 1 < lines.len() && lines[i + 1].explicit_start.is_some() {
                        (lines[i + 1].explicit_start.unwrap() - 0.05).max(st + 0.3)
                    } else {
                        (st + 2.5).min(total_duration)
                    }
                });

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

        let weights: Vec<f64> = lines.iter().map(|l| Self::calculate_weight(&l.text)).collect();
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
            let total_speech_time: f64 = speech_segments.iter().map(|s| (s.end - s.start).max(0.08)).sum();

            if total_speech_time > 0.3 {
                // Case A: M >= N (More speech segments than script lines)
                // Partition M segments into N contiguous groups using DP
                if m >= n {
                    // seg_dur[k] = duration of segment k
                    let seg_durs: Vec<f64> = speech_segments.iter().map(|s| (s.end - s.start).max(0.08)).collect();
                    let target_durs: Vec<f64> = weights.iter().map(|w| (w / total_weight) * total_speech_time).collect();

                    // cum_seg_dur[k] = sum_{0..k} seg_durs
                    let mut cum_seg_dur = vec![0.0; m + 1];
                    for k in 0..m {
                        cum_seg_dur[k + 1] = cum_seg_dur[k] + seg_durs[k];
                    }

                    // dp[i][j]: min cost to map first i lines (1..=n) to first j segments (1..=m)
                    // parent[i][j]: optimal split point k
                    let inf = 1e12_f64;
                    let mut dp = vec![vec![inf; m + 1]; n + 1];
                    let mut parent = vec![vec![0usize; m + 1]; n + 1];

                    dp[0][0] = 0.0;

                    for i in 1..=n {
                        let target_d = target_durs[i - 1];
                        for j in i..=m {
                            for k in (i - 1)..j {
                                if dp[i - 1][k] >= inf {
                                    continue;
                                }
                                let allocated_d = cum_seg_dur[j] - cum_seg_dur[k];
                                let diff = allocated_d - target_d;
                                let cost = dp[i - 1][k] + diff * diff;
                                if cost < dp[i][j] {
                                    dp[i][j] = cost;
                                    parent[i][j] = k;
                                }
                            }
                        }
                    }

                    // Backtrack to find boundaries
                    let mut boundaries = vec![0usize; n + 1];
                    let mut curr_j = m;
                    for i in (1..=n).rev() {
                        boundaries[i] = curr_j;
                        curr_j = parent[i][curr_j];
                    }
                    boundaries[0] = 0;

                    let mut results = Vec::with_capacity(n);
                    for i in 0..n {
                        let start_seg_idx = boundaries[i];
                        let end_seg_idx = boundaries[i + 1] - 1;

                        let st = speech_segments[start_seg_idx].start;
                        let et = speech_segments[end_seg_idx].end.max(st + 0.3);

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
                    let seg_durs: Vec<f64> = speech_segments.iter().map(|s| (s.end - s.start).max(0.08)).collect();
                    let line_target_durs: Vec<f64> = weights.iter().map(|w| (w / total_weight) * total_speech_time).collect();

                    let mut cum_line_dur = vec![0.0; n + 1];
                    for k in 0..n {
                        cum_line_dur[k + 1] = cum_line_dur[k] + line_target_durs[k];
                    }

                    let inf = 1e12_f64;
                    let mut dp = vec![vec![inf; n + 1]; m + 1];
                    let mut parent = vec![vec![0usize; n + 1]; m + 1];

                    dp[0][0] = 0.0;

                    for j in 1..=m {
                        let seg_d = seg_durs[j - 1];
                        for i in j..=n {
                            for k in (j - 1)..i {
                                if dp[j - 1][k] >= inf {
                                    continue;
                                }
                                let allocated_line_d = cum_line_dur[i] - cum_line_dur[k];
                                let diff = allocated_line_d - seg_d;
                                let cost = dp[j - 1][k] + diff * diff;
                                if cost < dp[j][i] {
                                    dp[j][i] = cost;
                                    parent[j][i] = k;
                                }
                            }
                        }
                    }

                    let mut boundaries = vec![0usize; m + 1];
                    let mut curr_i = n;
                    for j in (1..=m).rev() {
                        boundaries[j] = curr_i;
                        curr_i = parent[j][curr_i];
                    }
                    boundaries[0] = 0;

                    let mut results = Vec::with_capacity(n);
                    for j in 0..m {
                        let start_line_idx = boundaries[j];
                        let end_line_idx = boundaries[j + 1];
                        let seg = &speech_segments[j];
                        let seg_total_dur = (seg.end - seg.start).max(0.1);

                        let lines_in_seg = end_line_idx - start_line_idx;
                        let group_weight: f64 = (start_line_idx..end_line_idx).map(|idx| weights[idx]).sum();

                        let mut curr_t = seg.start;
                        for l_idx in start_line_idx..end_line_idx {
                            let w = weights[l_idx];
                            let frac = if group_weight > 0.0 { w / group_weight } else { 1.0 / lines_in_seg as f64 };
                            let item_dur = frac * seg_total_dur;
                            let item_start = curr_t;
                            let item_end = (curr_t + item_dur).max(item_start + 0.25);
                            curr_t += item_dur;

                            results.push(SubtitleItem {
                                index: results.len() + 1,
                                start_secs: item_start,
                                end_secs: item_end,
                                start_formatted: Self::format_srt_time(item_start),
                                end_formatted: Self::format_srt_time(item_end),
                                text: lines[l_idx].text.clone(),
                            });
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
        assert_eq!(SubtitleController::format_srt_time(3661.050), "01:01:01,050");
    }

    #[test]
    fn test_split_and_clean_script() {
        let script = "안녕하세요! 반갑습니다.\nOmniRec 자막 생성기입니다.";
        let lines = SubtitleController::split_and_clean_script(script, "sentence", 30);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "안녕하세요!");
        assert_eq!(lines[1].text, "반갑습니다.");
        assert_eq!(lines[2].text, "OmniRec 자막 생성기입니다.");
    }

    #[test]
    fn test_align_script_fallback() {
        let lines = vec![
            ParsedLine { text: "첫 번째 문장".to_string(), explicit_start: None, explicit_end: None },
            ParsedLine { text: "두 번째 문장".to_string(), explicit_start: None, explicit_end: None },
        ];
        let subs = SubtitleController::align_script_to_speech(&lines, &[], 10.0, 0.1, 0.2);
        assert_eq!(subs.len(), 2);
        assert!(subs[0].start_secs >= 0.1);
        assert!(subs[0].end_secs < subs[1].start_secs);
        assert!(subs[1].end_secs <= 10.0);
    }
}
