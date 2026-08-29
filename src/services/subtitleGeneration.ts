import { invoke } from '@tauri-apps/api/core';
import type {
  SubtitleGenerateResult,
  SubtitleGenerateTask,
  SubtitleItem,
  SubtitleSplitMode,
} from '../types';
import {
  alignScriptWithWhisperChunks,
  generateSubtitlesFromAiChunks,
  runLocalWhisperTranscribe,
  splitScriptIntoLines,
} from './whisperService';

export type SubtitleWorkflow = 'with-script' | 'ai-only';
export type SubtitleSyncEngine = 'ai-whisper' | 'vad';
export type WhisperModel =
  | 'Xenova/whisper-tiny'
  | 'Xenova/whisper-base'
  | 'Xenova/whisper-small';

export interface SubtitleGenerationOptions {
  audioPath: string;
  scriptText: string;
  workflow: SubtitleWorkflow;
  syncEngine: SubtitleSyncEngine;
  whisperModel: WhisperModel;
  whisperLanguage: string;
  splitMode: SubtitleSplitMode;
  splitOnComma: boolean;
  maxChars: number;
  silenceThresholdDb: number;
  minSilenceDuration: number;
  startOffsetSecs: number;
  autoSave: boolean;
  outputDir?: string | null;
  onProgress?: (message: string, percent?: number) => void;
}

export interface SubtitleGenerationOutcome {
  subtitles: SubtitleItem[];
  srtContent: string;
  vttContent: string;
  srtPath?: string | null;
  vttPath?: string | null;
  totalDuration: number;
  segmentsDetected: number;
  /** 대본 없는 AI 전사 모드에서 인식된 전체 텍스트 */
  transcribedText?: string;
}

export const formatSrtTimestamp = (secs: number) => {
  const totalMillis = Math.round(Math.max(0, secs) * 1000);
  const ms = totalMillis % 1000;
  const s = Math.floor(totalMillis / 1000) % 60;
  const m = Math.floor(totalMillis / (1000 * 60)) % 60;
  const h = Math.floor(totalMillis / (1000 * 60 * 60));
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')},${String(ms).padStart(3, '0')}`;
};

export const formatVttTimestamp = (secs: number) => {
  const totalMillis = Math.round(Math.max(0, secs) * 1000);
  const ms = totalMillis % 1000;
  const s = Math.floor(totalMillis / 1000) % 60;
  const m = Math.floor(totalMillis / (1000 * 60)) % 60;
  const h = Math.floor(totalMillis / (1000 * 60 * 60));
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}.${String(ms).padStart(3, '0')}`;
};

export const buildSrt = (items: SubtitleItem[]) =>
  items
    .map(
      (item, idx) =>
        `${idx + 1}\n${formatSrtTimestamp(item.start_secs)} --> ${formatSrtTimestamp(item.end_secs)}\n${item.text}\n`,
    )
    .join('\n');

export const buildVtt = (items: SubtitleItem[]) =>
  'WEBVTT\n\n' +
  items
    .map(
      (item, idx) =>
        `${idx + 1}\n${formatVttTimestamp(item.start_secs)} --> ${formatVttTimestamp(item.end_secs)}\n${item.text}\n`,
    )
    .join('\n');

/** 오디오 파일 경로에서 자막 파일이 저장될 경로(확장자 제외)를 만든다. */
const resolveOutputStem = (audioPath: string, outputDir?: string | null) => {
  const stem = audioPath.split(/[\\/]/).pop()?.replace(/\.[^/.]+$/, '') || 'subtitles';
  const lastSlash = Math.max(audioPath.lastIndexOf('/'), audioPath.lastIndexOf('\\'));
  const targetDir =
    outputDir?.trim() || (lastSlash > 0 ? audioPath.substring(0, lastSlash) : '.');
  return `${targetDir}/${stem}`;
};

const saveSubtitleFiles = async (
  audioPath: string,
  outputDir: string | null | undefined,
  srtContent: string,
  vttContent: string,
): Promise<{ srtPath?: string; vttPath?: string }> => {
  const stem = resolveOutputStem(audioPath, outputDir);
  const result: { srtPath?: string; vttPath?: string } = {};

  try {
    await invoke('save_subtitle_file', { path: `${stem}.srt`, content: srtContent });
    result.srtPath = `${stem}.srt`;
  } catch (err) {
    console.error('SRT 저장 실패:', err);
  }
  try {
    await invoke('save_subtitle_file', { path: `${stem}.vtt`, content: vttContent });
    result.vttPath = `${stem}.vtt`;
  } catch (err) {
    console.error('VTT 저장 실패:', err);
  }
  return result;
};

/**
 * 자막 생성 파이프라인 본체.
 *
 * 자막 생성기 화면과 자막 일괄 생성이 같은 코드를 쓰도록 여기 한 곳에 모아 둔다.
 * 엔진은 두 가지다.
 * - `ai-whisper`: 로컬 Whisper 로 단어 타임스탬프를 실측하고 대본과 강제 정렬
 * - `vad`: Rust 쪽 음성 파형 VAD + DP 정렬 (`generate_subtitles` 커맨드)
 */
export const generateSubtitles = async (
  options: SubtitleGenerationOptions,
): Promise<SubtitleGenerationOutcome> => {
  const {
    audioPath,
    scriptText,
    workflow,
    syncEngine,
    whisperModel,
    whisperLanguage,
    splitMode,
    splitOnComma,
    maxChars,
    silenceThresholdDb,
    minSilenceDuration,
    startOffsetSecs,
    autoSave,
    outputDir,
    onProgress,
  } = options;

  if (!audioPath.trim()) {
    throw new Error('음성 또는 영상 미디어 파일을 선택해 주세요.');
  }
  if (workflow === 'with-script' && !scriptText.trim()) {
    throw new Error('대본 내용이 비어 있습니다.');
  }

  // ── 로컬 AI Whisper 모드 ────────────────────────────────
  if (workflow === 'ai-only' || syncEngine === 'ai-whisper') {
    onProgress?.('오디오 16kHz PCM 데이터를 추출하는 중...', 5);

    const rawSamples = await invoke<number[]>('extract_audio_pcm_16k', { path: audioPath });
    if (!rawSamples || rawSamples.length === 0) {
      throw new Error('오디오 데이터를 읽을 수 없습니다.');
    }

    const floatArray = new Float32Array(rawSamples);
    const totalDuration = floatArray.length / 16000;

    const whisperResult = await runLocalWhisperTranscribe(
      floatArray,
      whisperModel,
      (message, percent) => onProgress?.(message, percent),
      whisperLanguage,
    );

    let subtitles: SubtitleItem[];
    if (workflow === 'ai-only') {
      subtitles = generateSubtitlesFromAiChunks(whisperResult.chunks, maxChars);
    } else {
      const lines = splitScriptIntoLines(scriptText, splitMode, maxChars, splitOnComma);
      if (lines.length === 0) {
        throw new Error('대본에서 유효한 텍스트 문장을 찾을 수 없습니다.');
      }
      subtitles = alignScriptWithWhisperChunks(lines, whisperResult.chunks, totalDuration);
    }

    const srtContent = buildSrt(subtitles);
    const vttContent = buildVtt(subtitles);
    const saved = autoSave
      ? await saveSubtitleFiles(audioPath, outputDir, srtContent, vttContent)
      : {};

    return {
      subtitles,
      srtContent,
      vttContent,
      srtPath: saved.srtPath,
      vttPath: saved.vttPath,
      totalDuration,
      segmentsDetected: whisperResult.chunks.length,
      transcribedText: whisperResult.text,
    };
  }

  // ── 고속 음성 파형 VAD + DP 모드 ────────────────────────
  onProgress?.('음성 파형을 분석하는 중...', 40);

  const task: SubtitleGenerateTask = {
    audio_path: audioPath,
    script_text: scriptText,
    split_mode: splitMode,
    split_on_comma: splitOnComma,
    max_chars: maxChars,
    min_silence_duration_secs: minSilenceDuration,
    silence_threshold_db: silenceThresholdDb,
    start_offset_secs: startOffsetSecs,
    end_margin_secs: 0.2,
    auto_save: autoSave,
    output_dir: outputDir || null,
  };

  const result = await invoke<SubtitleGenerateResult>('generate_subtitles', { task });

  return {
    subtitles: result.subtitles,
    srtContent: result.srt_content,
    vttContent: result.vtt_content,
    srtPath: result.srt_path,
    vttPath: result.vtt_path,
    totalDuration: result.total_duration,
    segmentsDetected: result.speech_segments_detected,
  };
};
