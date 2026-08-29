export interface RectRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SelectionScreenInfo {
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
  /**
   * OmniRec 자신이 내는 소리(앱 내 Typecast 웹뷰의 TTS 재생 등)를 시스템 오디오에 포함할지.
   * macOS 전용. 꺼두면 스피커로는 들리는데 녹음 파일은 무음이 된다.
   */
  system_audio_include_own_app: boolean;
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
  subtitle_generation_workflow: 'with-script' | 'ai-only';
  subtitle_sync_engine: 'ai-whisper' | 'vad';
  subtitle_whisper_model: 'Xenova/whisper-tiny' | 'Xenova/whisper-base' | 'Xenova/whisper-small';
  subtitle_whisper_language: string;
  subtitle_split_mode: SubtitleSplitMode;
  subtitle_max_chars: number;
  subtitle_silence_threshold_db: number;
  subtitle_min_silence_duration: number;
  subtitle_start_offset_secs: number;
  subtitle_auto_save: boolean;
  subtitle_auto_scroll: boolean;
  subtitle_ripple_edit: boolean;
  subtitle_split_on_comma: boolean;
  typecast_editor_url: string;
  typecast_signin_url: string;
  /** 표시용 계정 이메일. 비밀번호는 저장하지 않으며 세션은 브라우저 쿠키로 유지된다. */
  typecast_account_email?: string | null;
  typecast_session_saved: boolean;
  typecast_last_login_at?: string | null;
  /** 자동화용 사용자 지정 CSS 선택자 (비우면 내장 휴리스틱) */
  typecast_editor_selector: string;
  typecast_play_selector: string;
  tts_countdown_secs: number;
  tts_mic_enabled: boolean;
  /** 낭독이 끝났다고 판정할 무음 길이(초). 자동 · 수동 TTS 녹음이 함께 쓴다. */
  tts_auto_stop_seconds: number;
  /** 낭독 소리로 판정할 시스템 오디오 레벨(dB) */
  tts_speech_threshold_db: number;
  /** 재생 시작(소리 감지) 최대 대기 시간(초) */
  tts_start_timeout_secs: number;
  /** 일괄 처리에서 대본 사이 간격(초) */
  tts_gap_secs: number;
  /** 실패한 대본이 있어도 계속 진행할지 */
  tts_batch_continue_on_error: boolean;
}

/**
 * 상단 탭. 위젯 단위가 아니라 작업 흐름 단위로 묶는다.
 * - record : 오디오 녹음 · 화면 녹화
 * - script : 대본 관리 → TTS 자동/수동 녹음
 * - subtitle : 자막 편집기 · 대본 일괄 생성
 * - files  : 히스토리 · 병합 · 변환
 */
export type TabType = 'record' | 'script' | 'subtitle' | 'files' | 'settings';

export type RecordView = 'audio' | 'screen';
export type FilesView = 'history' | 'merger' | 'converter';

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
  split_on_comma?: boolean;
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


// ─────────────────────────────────────────────────────────────
// 대본 관리 & Typecast TTS
// ─────────────────────────────────────────────────────────────

export interface ScriptItem {
  id: string;
  title: string;
  content: string;
  tags: string[];
  memo: string;
  created_at: string;
  updated_at: string;
  char_count: number;
  line_count: number;
  /** 한국어 낭독 평균 속도 기준 예상 낭독 시간(초) */
  estimated_secs: number;
  last_recorded_path?: string | null;
  last_recorded_at?: string | null;
  record_count: number;
}

export interface ScriptDraft {
  id?: string | null;
  title: string;
  content: string;
  tags: string[];
  memo: string;
}

export interface TypecastBrowserState {
  is_open: boolean;
  current_url?: string | null;
  looks_signed_in: boolean;
  account_email?: string | null;
  last_login_at?: string | null;
}

export interface TypecastNavigationPayload {
  url: string;
  looks_signed_in: boolean;
}

export interface TypecastPopupPayload {
  url: string;
}

/** 페이지 자동화 단계 보고 */
export interface TypecastStepPayload {
  name: string;
  detail: string;
}

/** Typecast 창 연동 진단 로그 */
export interface TypecastDebugPayload {
  kind: string;
  detail: string;
  at: string;
}


export type BatchItemStatus =
  | 'pending'
  | 'preparing'
  | 'recording'
  | 'speaking'
  | 'saving'
  | 'done'
  | 'failed'
  | 'skipped';

export interface BatchItemState {
  scriptId: string;
  title: string;
  status: BatchItemStatus;
  message?: string;
  outputPath?: string | null;
  durationSecs?: number;
}

export type ScriptStudioView = 'library' | 'batch' | 'manual';
