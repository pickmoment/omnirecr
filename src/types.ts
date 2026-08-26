export interface RectRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ScreenCaptureInfo {
  image_data_url: string;
  physical_width: number;
  physical_height: number;
  scale_factor: number;
}

export type AudioFormat = 'mp3' | 'm4a' | 'wav';

export interface Settings {
  output_dir: string;
  audio_format: AudioFormat;
  audio_bitrate: number; // 128, 192, 256, 320
  audio_sample_rate: number; // 44100, 48000
  video_fps: number; // 30, 60
  system_audio_enabled: boolean;
  system_audio_volume: number; // 0.0 to 2.0
  mic_audio_enabled: boolean;
  mic_audio_volume: number; // 0.0 to 2.0
  noise_gate_enabled: boolean;
  noise_gate_threshold_db: number; // -60 to -20
  highpass_filter_enabled: boolean;
  mute_notifications: boolean;
  macos_shortcut_start: string;
  macos_shortcut_stop: string;
  auto_pause_enabled: boolean;
  auto_pause_seconds: number;
  auto_stop_enabled: boolean;
  auto_stop_seconds: number;
  custom_ffmpeg_path?: string | null;
}

export type TabType = 'screen' | 'audio' | 'subtitle' | 'converter' | 'history' | 'merger' | 'settings';

export interface RecordingStatus {
  status: 'idle' | 'recording' | 'paused' | 'stopping';
  mode: 'screen' | 'audio' | null;
  duration_secs: number;
  size_bytes: number;
  is_auto_paused: boolean;
  output_file: string | null;
  sys_vu_level: number;
  mic_vu_level: number;
}

export interface AudioVUMeterPayload {
  sys_level_db: number;
  mic_level_db: number;
  is_silent: boolean;
  duration_secs: number;
  size_bytes: number;
}

export interface HistoryItem {
  id: string;
  file_name: string;
  file_path: string;
  file_type: 'audio' | 'video' | string;
  format: string;
  size_bytes: number;
  size_formatted: string;
  duration_secs: number;
  duration_formatted: string;
  created_at: string;
  resolution?: string | null;
}

export interface MediaProbeInfo {
  path: string;
  file_name: string;
  file_type: string;
  format_name: string;
  duration_secs: number;
  size_bytes: number;
  video_codec?: string | null;
  audio_codec?: string | null;
  width?: number | null;
  height?: number | null;
  fps?: number | null;
  sample_rate?: number | null;
  channels?: number | null;
}

export interface MergeTaskPayload {
  input_files: string[];
  output_path: string;
  output_format: string;
}

export interface MergeProgressPayload {
  percent: number;
  current_time_secs: number;
  total_time_secs: number;
  is_direct_copy: boolean;
  speed: string;
  finished: boolean;
  error?: string | null;
}

export interface AudioConvertTaskPayload {
  input_files: string[];
  target_format: 'mp3' | 'm4a';
  bitrate: number;
  sample_rate?: number | null;
  channels?: number | null;
  output_dir?: string | null;
}

export interface AudioConvertProgressPayload {
  file_index: number;
  total_files: number;
  current_file_name: string;
  output_file_path: string;
  percent: number;
  overall_percent: number;
  current_time_secs: number;
  total_time_secs: number;
  speed: string;
  finished: boolean;
  error?: string | null;
}

export interface SubtitleItem {
  index: number;
  start_secs: number;
  end_secs: number;
  start_formatted: string;
  end_formatted: string;
  text: string;
}

export type SubtitleSplitMode = 'auto' | 'sentence' | 'line' | 'length';

export interface SubtitleGenerateTask {
  audio_path: string;
  script_text: string;
  split_mode: SubtitleSplitMode;
  max_chars: number;
  min_silence_duration_secs?: number;
  silence_threshold_db?: number;
  start_offset_secs?: number;
  end_margin_secs?: number;
  auto_save: boolean;
  output_dir?: string | null;
}

export interface SubtitleGenerateResult {
  subtitles: SubtitleItem[];
  srt_content: string;
  vtt_content: string;
  srt_path?: string | null;
  vtt_path?: string | null;
  total_duration: number;
  speech_segments_detected: number;
  script_lines_count: number;
}

