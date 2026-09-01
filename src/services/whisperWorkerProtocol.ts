/**
 * 메인 스레드 ↔ Whisper 워커 사이의 메시지 계약.
 *
 * 이 모듈은 **타입과 상수만** 담는다. 메인 스레드가 워커 본체(`whisperWorker.ts`)를 정적으로
 * import 하면 transformers.js 와 onnxruntime-web 이 메인 번들로 딸려 들어와 워커로 옮긴 의미가
 * 사라지므로, 공유가 필요한 것은 전부 여기에 둔다.
 */
import type { SubtitleItem } from '../types';

export interface WhisperChunk {
  text: string;
  timestamp: [number, number];
}

export interface WhisperTranscribeResult {
  text: string;
  chunks: WhisperChunk[];
  /** 전사 결과로 만든 자막. 정렬 DP 도 워커에서 돈다(메인 스레드에서는 수십 초씩 멈춘다). */
  subtitles: SubtitleItem[];
}

/**
 * 전사 뒤 자막을 만드는 방식.
 * - `script`: 대본 줄을 실측 타임스탬프에 강제 정렬한다(무거운 DP).
 * - `ai`: 전사 청크만으로 자막을 만든다.
 */
export type WhisperAlignSpec =
  | { mode: 'script'; lines: string[] }
  | { mode: 'ai'; maxChars: number };

/** 전사 요청. `buffer` 는 transfer 로 넘긴다(사본 없음, 넘긴 쪽 버퍼는 분리된다). */
export interface WhisperTranscribeRequest {
  type: 'transcribe';
  id: number;
  modelId: string;
  /** transformers.js `language` 옵션. 빈 문자열이면 자동 감지. */
  language: string;
  /** raw f32 PCM(16kHz mono) 이 담긴 ArrayBuffer */
  buffer: ArrayBuffer;
  /** 버퍼 안에서 PCM 이 시작하는 바이트 오프셋 */
  byteOffset: number;
  /** 샘플 개수. 총 길이(초) = length / 16000 */
  length: number;
  align: WhisperAlignSpec;
}

/** 캐시된 모델을 내려놓는다(모델 교체 등). 진행 중인 전사가 끝난 뒤에 처리된다. */
export interface WhisperResetRequest {
  type: 'reset';
}

export type WhisperWorkerRequest = WhisperTranscribeRequest | WhisperResetRequest;

/** 모델 다운로드/로딩 진행. transformers.js `progress_callback` 을 그대로 중계한다. */
export interface WhisperLoadProgressMessage {
  type: 'load-progress';
  id: number;
  status: string;
  progress?: number;
  file?: string;
}

/** 진행 단계 보고. 사용자에게 보여 줄 문구는 메인 스레드가 만든다. */
export interface WhisperStageMessage {
  type: 'stage';
  id: number;
  stage: 'model-ready' | 'transcribing' | 'segment-fallback' | 'analyzed';
}

export interface WhisperDoneMessage {
  type: 'done';
  id: number;
  text: string;
  chunks: WhisperChunk[];
  subtitles: SubtitleItem[];
}

export interface WhisperErrorMessage {
  type: 'error';
  id: number;
  message: string;
  /** 모델 로딩 단계에서 실패했는가(전사 중 실패와 문구를 구분한다). */
  whileLoading: boolean;
}

export type WhisperWorkerResponse =
  | WhisperLoadProgressMessage
  | WhisperStageMessage
  | WhisperDoneMessage
  | WhisperErrorMessage;
