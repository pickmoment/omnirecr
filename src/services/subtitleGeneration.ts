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
  throwIfCancelled,
} from './whisperService';

// 취소는 성공도 실패도 아니다. 호출자가 "생성 실패"로 표시하지 않도록 판별 수단을 같이 내보낸다.
export { SubtitleCancelledError, isSubtitleCancelled } from './whisperService';

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
  /**
   * 취소 신호. 중단되면 진행 콜백이 멈추고 SubtitleCancelledError 로 reject 한다.
   * 이게 없으면 일괄 생성에서 [중단]을 눌러도 진행 중 항목이 끝까지 돌아 "완료"로 보고된다.
   */
  signal?: AbortSignal;
}

/** 자막 파일 저장이 실패한 포맷과 이유. 저장 실패를 성공으로 보고하지 않기 위한 통로다. */
export interface SubtitleSaveFailure {
  format: 'srt' | 'vtt';
  /** 저장을 시도한 경로(백엔드가 경로를 알려 주지 않은 경우 null) */
  path: string | null;
  message: string;
}

export interface SubtitleGenerationOutcome {
  subtitles: SubtitleItem[];
  srtContent: string;
  vttContent: string;
  srtPath?: string | null;
  vttPath?: string | null;
  /** 자동 저장을 요청했는데 실패한 포맷 목록. 비어 있어야 정상이다. */
  saveFailures: SubtitleSaveFailure[];
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

interface SubtitleSaveOutcome {
  srtPath?: string;
  vttPath?: string;
  failures: SubtitleSaveFailure[];
}

/**
 * 자막 파일을 저장한다.
 *
 * 예전 코드는 invoke 오류를 잡아 콘솔에만 남기고 경로 없는 결과를 돌려줬다. 그래서 권한이 없거나
 * 디스크가 꽉 차서 한 줄도 못 썼는데 화면에는 "생성 완료"가 떴다. 실패한 포맷과 이유를 호출자에게
 * 그대로 올려 보낸다.
 */
const saveSubtitleFiles = async (
  audioPath: string,
  outputDir: string | null | undefined,
  srtContent: string,
  vttContent: string,
): Promise<SubtitleSaveOutcome> => {
  const stem = resolveOutputStem(audioPath, outputDir);
  const result: SubtitleSaveOutcome = { failures: [] };

  const targets: Array<{ format: 'srt' | 'vtt'; path: string; content: string }> = [
    { format: 'srt', path: `${stem}.srt`, content: srtContent },
    { format: 'vtt', path: `${stem}.vtt`, content: vttContent },
  ];

  for (const target of targets) {
    try {
      await invoke('save_subtitle_file', { path: target.path, content: target.content });
      if (target.format === 'srt') {
        result.srtPath = target.path;
      } else {
        result.vttPath = target.path;
      }
    } catch (err) {
      console.error(`${target.format.toUpperCase()} 저장 실패:`, err);
      result.failures.push({
        format: target.format,
        path: target.path,
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }

  return result;
};

/**
 * IPC 로 받은 raw f32le 바이트를 사본 없이 Float32Array 로 본다.
 *
 * 뷰만 만들기 때문에 1시간 오디오(5,760만 샘플 = 230MB)에서도 추가 할당이 0 이다.
 * Tauri 는 `tauri::ipc::Response` 를 ArrayBuffer 로 넘기지만, 웹뷰에 따라 Uint8Array 로
 * 도착할 수 있어 둘 다 받는다(`new Uint8Array(buffer)` 는 뷰이지 사본이 아니다).
 */
const viewPcmSamples = (raw: ArrayBuffer | Uint8Array): Float32Array => {
  const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
  if (bytes.byteOffset % 4 !== 0) {
    throw new Error('오디오 PCM 데이터가 f32 경계에 정렬되어 있지 않습니다.');
  }
  return new Float32Array(bytes.buffer, bytes.byteOffset, Math.floor(bytes.byteLength / 4));
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
    signal,
  } = options;

  // 취소된 뒤에는 진행 콜백을 부르지 않는다. 언마운트된 화면의 setState 를 깨우면 안 된다.
  const report = (message: string, percent?: number) => {
    if (signal?.aborted) return;
    onProgress?.(message, percent);
  };

  if (!audioPath.trim()) {
    throw new Error('음성 또는 영상 미디어 파일을 선택해 주세요.');
  }
  if (workflow === 'with-script' && !scriptText.trim()) {
    throw new Error('대본 내용이 비어 있습니다.');
  }

  // ── 로컬 AI Whisper 모드 ────────────────────────────────
  if (workflow === 'ai-only' || syncEngine === 'ai-whisper') {
    throwIfCancelled(signal);
    report('오디오 16kHz PCM 데이터를 추출하는 중...', 5);

    // 백엔드가 raw f32le 바이트를 그대로 넘긴다(tauri::ipc::Response).
    // 예전에는 number[] 로 받아 JSON 배열 → Float32Array → 방어용 사본까지 세 벌을 만들었고,
    // 1시간 오디오에서 사본 하나가 230MB 였다. 지금은 같은 버퍼를 뷰로 본다.
    const pcmBytes = await invoke<ArrayBuffer>('extract_audio_pcm_16k', { path: audioPath });
    const pcm = viewPcmSamples(pcmBytes);
    if (pcm.length === 0) {
      throw new Error('오디오 데이터를 읽을 수 없습니다.');
    }

    throwIfCancelled(signal);
    const totalDuration = pcm.length / 16000;

    // pcm 은 전사 과정에서 제자리 세정되므로 이후 다시 쓰지 않는다.
    const whisperResult = await runLocalWhisperTranscribe(
      pcm,
      whisperModel,
      report,
      whisperLanguage,
      signal,
    );

    throwIfCancelled(signal);

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

    // 취소된 뒤에 파일을 쓰면 사용자가 지운 결과가 다시 살아난다.
    throwIfCancelled(signal);
    const saved: SubtitleSaveOutcome = autoSave
      ? await saveSubtitleFiles(audioPath, outputDir, srtContent, vttContent)
      : { failures: [] };

    return {
      subtitles,
      srtContent,
      vttContent,
      srtPath: saved.srtPath,
      vttPath: saved.vttPath,
      saveFailures: saved.failures,
      totalDuration,
      segmentsDetected: whisperResult.chunks.length,
      transcribedText: whisperResult.text,
    };
  }

  // ── 고속 음성 파형 VAD + DP 모드 ────────────────────────
  throwIfCancelled(signal);
  report('음성 파형을 분석하는 중...', 40);

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
  throwIfCancelled(signal);

  // 저장을 맡겼는데 경로가 돌아오지 않았다면 파일이 안 생긴 것이다. 성공으로 넘기지 않는다.
  const saveFailures: SubtitleSaveFailure[] = [];
  if (autoSave) {
    if (!result.srt_path) {
      saveFailures.push({
        format: 'srt',
        path: null,
        message: '백엔드가 SRT 저장 경로를 반환하지 않았습니다.',
      });
    }
    if (!result.vtt_path) {
      saveFailures.push({
        format: 'vtt',
        path: null,
        message: '백엔드가 VTT 저장 경로를 반환하지 않았습니다.',
      });
    }
  }

  return {
    subtitles: result.subtitles,
    srtContent: result.srt_content,
    vttContent: result.vtt_content,
    srtPath: result.srt_path,
    vttPath: result.vtt_path,
    saveFailures,
    totalDuration: result.total_duration,
    segmentsDetected: result.speech_segments_detected,
  };
};
