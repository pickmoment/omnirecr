use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RectRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionScreenInfo {
    pub physical_width: u32,
    pub physical_height: u32,
    pub scale_factor: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub output_dir: String,
    pub audio_format: String, // "mp3" | "m4a"
    pub audio_bitrate: u32,   // 128, 192, 256, 320
    pub audio_sample_rate: u32, // 44100, 48000
    pub video_fps: u32,       // 30, 60
    pub system_audio_enabled: bool,
    pub system_audio_volume: f32, // 0.0 to 2.0 (0% to 200%)
    pub mic_audio_enabled: bool,
    pub mic_audio_volume: f32,    // 0.0 to 2.0 (0% to 200%)
    pub noise_gate_enabled: bool,
    pub noise_gate_threshold_db: f32, // e.g. -45.0 dB
    pub highpass_filter_enabled: bool, // 80Hz Low-cut filter
    pub mute_notifications: bool,     // Auto mute windows notifications during recording
    #[serde(default = "default_macos_shortcut_start")]
    pub macos_shortcut_start: String,
    #[serde(default = "default_macos_shortcut_stop")]
    pub macos_shortcut_stop: String,
    pub auto_pause_enabled: bool,
    pub auto_pause_seconds: f32,      // default 1.0s
    pub auto_stop_enabled: bool,
    pub auto_stop_seconds: f32,       // default 5.0s
    pub custom_ffmpeg_path: Option<String>,
    #[serde(default = "default_subtitle_generation_workflow")]
    pub subtitle_generation_workflow: String, // "with-script" | "ai-only"
    #[serde(default = "default_subtitle_sync_engine")]
    pub subtitle_sync_engine: String, // "ai-whisper" | "vad"
    #[serde(default = "default_subtitle_whisper_model")]
    pub subtitle_whisper_model: String,
    #[serde(default = "default_subtitle_whisper_language")]
    pub subtitle_whisper_language: String,
    #[serde(default = "default_subtitle_split_mode")]
    pub subtitle_split_mode: String, // "auto" | "sentence" | "line" | "length"
    #[serde(default = "default_subtitle_max_chars")]
    pub subtitle_max_chars: u32,
    #[serde(default = "default_subtitle_silence_threshold_db")]
    pub subtitle_silence_threshold_db: f32,
    #[serde(default = "default_subtitle_min_silence_duration")]
    pub subtitle_min_silence_duration: f32,
    #[serde(default = "default_subtitle_start_offset_secs")]
    pub subtitle_start_offset_secs: f32,
    #[serde(default = "default_subtitle_auto_save")]
    pub subtitle_auto_save: bool,
    #[serde(default = "default_subtitle_auto_scroll")]
    pub subtitle_auto_scroll: bool,
    #[serde(default)]
    pub subtitle_ripple_edit: bool,
    #[serde(default)]
    pub subtitle_split_on_comma: bool,
}

fn default_macos_shortcut_start() -> String {
    "OmniRec 녹화 시작".to_string()
}

fn default_macos_shortcut_stop() -> String {
    "OmniRec 녹화 종료".to_string()
}

fn default_subtitle_generation_workflow() -> String {
    "with-script".to_string()
}

fn default_subtitle_sync_engine() -> String {
    "ai-whisper".to_string()
}

fn default_subtitle_whisper_model() -> String {
    "Xenova/whisper-base".to_string()
}

fn default_subtitle_whisper_language() -> String {
    "korean".to_string()
}

fn default_subtitle_split_mode() -> String {
    "auto".to_string()
}

fn default_subtitle_max_chars() -> u32 {
    28
}

fn default_subtitle_silence_threshold_db() -> f32 {
    -35.0
}

fn default_subtitle_min_silence_duration() -> f32 {
    0.25
}

fn default_subtitle_start_offset_secs() -> f32 {
    0.1
}

fn default_subtitle_auto_save() -> bool {
    true
}

fn default_subtitle_auto_scroll() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        let default_output = dirs::video_dir()
            .or_else(|| dirs::document_dir())
            .or_else(|| dirs::home_dir())
            .unwrap_or_else(|| PathBuf::from("."))
            .join("OmniRec");

        Self {
            output_dir: default_output.to_string_lossy().to_string(),
            audio_format: "m4a".to_string(),
            audio_bitrate: 256,
            audio_sample_rate: 48000,
            video_fps: 60,
            system_audio_enabled: true,
            system_audio_volume: 1.0,
            mic_audio_enabled: true,
            mic_audio_volume: 1.0,
            noise_gate_enabled: true,
            noise_gate_threshold_db: -45.0,
            highpass_filter_enabled: true,
            mute_notifications: true,
            macos_shortcut_start: default_macos_shortcut_start(),
            macos_shortcut_stop: default_macos_shortcut_stop(),
            auto_pause_enabled: false,
            auto_pause_seconds: 1.0,
            auto_stop_enabled: false,
            auto_stop_seconds: 5.0,
            custom_ffmpeg_path: None,
            subtitle_generation_workflow: default_subtitle_generation_workflow(),
            subtitle_sync_engine: default_subtitle_sync_engine(),
            subtitle_whisper_model: default_subtitle_whisper_model(),
            subtitle_whisper_language: default_subtitle_whisper_language(),
            subtitle_split_mode: default_subtitle_split_mode(),
            subtitle_max_chars: default_subtitle_max_chars(),
            subtitle_silence_threshold_db: default_subtitle_silence_threshold_db(),
            subtitle_min_silence_duration: default_subtitle_min_silence_duration(),
            subtitle_start_offset_secs: default_subtitle_start_offset_secs(),
            subtitle_auto_save: default_subtitle_auto_save(),
            subtitle_auto_scroll: default_subtitle_auto_scroll(),
            subtitle_ripple_edit: false,
            subtitle_split_on_comma: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingMode {
    Screen,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingStateStatus {
    Idle,
    Recording,
    Paused,
    Stopping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingStatus {
    pub status: RecordingStateStatus,
    pub mode: Option<RecordingMode>,
    pub duration_secs: f64,
    pub size_bytes: u64,
    pub is_auto_paused: bool,
    pub output_file: Option<String>,
    pub sys_vu_level: f32, // -60.0 to 0.0 dB
    pub mic_vu_level: f32, // -60.0 to 0.0 dB
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioVUMeterPayload {
    pub sys_level_db: f32,
    pub mic_level_db: f32,
    pub is_silent: bool,
    pub duration_secs: f64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub file_name: String,
    pub file_path: String,
    pub file_type: String, // "audio" | "video"
    pub format: String,    // "mp3", "m4a", "mp4", etc.
    pub size_bytes: u64,
    pub size_formatted: String,
    pub duration_secs: f64,
    pub duration_formatted: String,
    pub created_at: String,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProbeInfo {
    pub path: String,
    pub file_name: String,
    pub file_type: String, // "audio" | "video"
    pub format_name: String,
    pub duration_secs: f64,
    pub size_bytes: u64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeTaskPayload {
    pub input_files: Vec<String>,
    pub output_path: String,
    pub output_format: String, // "mp4", "mp3", "m4a"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeProgressPayload {
    pub percent: f32,
    pub current_time_secs: f64,
    pub total_time_secs: f64,
    pub is_direct_copy: bool,
    pub speed: String,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConvertTaskPayload {
    pub input_files: Vec<String>,
    pub target_format: String, // "mp3" | "m4a"
    pub bitrate: u32,          // e.g. 128, 192, 256, 320
    pub sample_rate: Option<u32>, // e.g. 44100, 48000
    pub channels: Option<u32>,    // e.g. 1, 2
    pub output_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConvertProgressPayload {
    pub file_index: usize,
    pub total_files: usize,
    pub current_file_name: String,
    pub output_file_path: String,
    pub percent: f32,
    pub overall_percent: f32,
    pub current_time_secs: f64,
    pub total_time_secs: f64,
    pub speed: String,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleItem {
    pub index: usize,
    pub start_secs: f64,
    pub end_secs: f64,
    pub start_formatted: String,
    pub end_formatted: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleGenerateTask {
    pub audio_path: String,
    pub script_text: String,
    pub split_mode: String, // "auto" | "line" | "sentence" | "length"
    pub max_chars: usize,
    #[serde(default)]
    pub split_on_comma: bool,
    pub min_silence_duration_secs: Option<f64>,
    pub silence_threshold_db: Option<f64>,
    pub start_offset_secs: Option<f64>,
    pub end_margin_secs: Option<f64>,
    pub auto_save: bool,
    pub output_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleGenerateResult {
    pub subtitles: Vec<SubtitleItem>,
    pub srt_content: String,
    pub vtt_content: String,
    pub srt_path: Option<String>,
    pub vtt_path: Option<String>,
    pub total_duration: f64,
    pub speech_segments_detected: usize,
    pub script_lines_count: usize,
}


