/// <reference lib="webworker" />
/**
 * 로컬 Whisper 전사 전용 워커.
 *
 * ONNX Runtime(WASM) 추론은 **동기 호출**이라 메인 스레드에서 돌리면 그 시간 동안 웹뷰가
 * 통째로 멈춘다. 30초 청크 하나가 수 초씩 걸리므로 긴 파일에서는 화면이 굳고, 자막 일괄
 * 생성처럼 파일을 연달아 처리하면 몇 분 동안 진행 표시도 중단 버튼도 반응하지 않았다.
 * 추론을 이 워커로 옮겨 UI 스레드를 비워 둔다.
 *
 * 취소는 메인 스레드가 워커를 `terminate()` 하는 것으로 처리한다 — WASM 추론은 중간에
 * 끼어들 수 없어서, 프로세스를 끊지 않으면 취소해도 계산이 계속 돌며 CPU 를 먹는다.
 */
import { pipeline, env, type AutomaticSpeechRecognitionPipeline } from '@xenova/transformers';
import {
  alignScriptWithWhisperChunks,
  generateSubtitlesFromAiChunks,
} from './subtitleAlign';
import type {
  WhisperChunk,
  WhisperWorkerRequest,
  WhisperWorkerResponse,
} from './whisperWorkerProtocol';

// 브라우저(웹뷰) 실행: 모델은 HF 허브에서 받아 Cache Storage 에 남긴다.
env.allowLocalModels = false;
env.useBrowserCache = true;

const ctx = self as unknown as DedicatedWorkerGlobalScope;

const post = (message: WhisperWorkerResponse) => ctx.postMessage(message);

/** 워커는 모델을 한 번에 하나만 들고 있는다(small 이 240MB). */
let cached: { modelId: string; pipe: AutomaticSpeechRecognitionPipeline } | null = null;

/**
 * 메시지를 도착 순서대로 **직렬 처리**한다.
 *
 * `onmessage` 핸들러는 await 지점에서 서로 끼어들 수 있어, 전사 도중 도착한 `reset` 이
 * 사용 중인 ONNX 세션을 dispose 하면 그 전사가 그 자리에서 죽는다.
 */
let queue: Promise<void> = Promise.resolve();

async function disposeCached() {
  const entry = cached;
  cached = null;
  if (!entry) return;
  try {
    await entry.pipe.dispose();
  } catch (err: unknown) {
    // 사용자에게 알릴 것은 없지만 조용히 삼키면 ORT 세션 누수를 추적할 수 없다.
    console.warn(`Whisper 파이프라인(${entry.modelId}) 정리 실패:`, err);
  }
}

async function ensurePipe(modelId: string, id: number) {
  if (cached && cached.modelId === modelId) return cached.pipe;

  // 다른 모델이 올라와 있으면 먼저 내려놓는다. 둘을 동시에 들고 있으면 메모리가 두 배가 된다.
  await disposeCached();

  const pipe = (await pipeline('automatic-speech-recognition', modelId, {
    progress_callback: (data: { status?: string; progress?: number; file?: string }) => {
      post({
        type: 'load-progress',
        id,
        status: String(data?.status ?? ''),
        progress: typeof data?.progress === 'number' ? data.progress : undefined,
        file: typeof data?.file === 'string' ? data.file : undefined,
      });
    },
  })) as AutomaticSpeechRecognitionPipeline;

  cached = { modelId, pipe };
  return pipe;
}

/** NaN/Inf 를 제자리에서 0 으로 바꾼다. 하나만 섞여도 특징 추출이 전부 NaN 으로 물든다. */
function sanitizePcmInPlace(pcm: Float32Array) {
  for (let i = 0; i < pcm.length; i++) {
    if (!Number.isFinite(pcm[i])) {
      pcm[i] = 0;
    }
  }
}

interface TranscriptionResultShape {
  text?: string;
  chunks?: Array<{ text?: string; timestamp?: [number, number] }>;
}

function normalizeChunks(output: TranscriptionResultShape | null): WhisperChunk[] {
  const chunks: WhisperChunk[] = [];
  if (!output || !Array.isArray(output.chunks)) return chunks;

  for (const c of output.chunks) {
    if (!c || !Array.isArray(c.timestamp)) continue;
    const rawStart = typeof c.timestamp[0] === 'number' && !isNaN(c.timestamp[0]) ? c.timestamp[0] : 0;
    const rawEnd =
      typeof c.timestamp[1] === 'number' && !isNaN(c.timestamp[1]) ? c.timestamp[1] : rawStart + 0.3;
    const start = Math.max(0, rawStart);
    const end = Math.max(start + 0.05, rawEnd);
    const text = (c.text || '').trim();
    if (text.length > 0) {
      chunks.push({ text, timestamp: [start, end] });
    }
  }
  return chunks;
}

async function transcribe(request: Extract<WhisperWorkerRequest, { type: 'transcribe' }>) {
  const { id, modelId, language, buffer, byteOffset, length, align } = request;

  let pipe: AutomaticSpeechRecognitionPipeline;
  try {
    pipe = await ensurePipe(modelId, id);
  } catch (err: unknown) {
    post({
      type: 'error',
      id,
      message: err instanceof Error ? err.message : String(err),
      whileLoading: true,
    });
    return;
  }

  post({ type: 'stage', id, stage: 'model-ready' });

  try {
    const audio = new Float32Array(buffer, byteOffset, length);
    sanitizePcmInPlace(audio);

    const langParam = language === 'auto' || !language ? undefined : language;
    post({ type: 'stage', id, stage: 'transcribing' });

    let output: TranscriptionResultShape | null = null;
    try {
      // 1. 단어 단위 타임스탬프(1초 미만 정밀도)
      const res = await pipe(audio, {
        return_timestamps: 'word',
        chunk_length_s: 30,
        stride_length_s: 5,
        language: langParam,
        task: 'transcribe',
      });
      output = (Array.isArray(res) ? res[0] : res) as TranscriptionResultShape;
    } catch (wordErr: unknown) {
      console.warn('Word-level timestamps failed, falling back to segment timestamps:', wordErr);
      post({ type: 'stage', id, stage: 'segment-fallback' });
      // 2. 구간 단위 타임스탬프로 재시도
      const res = await pipe(audio, {
        return_timestamps: true,
        chunk_length_s: 30,
        stride_length_s: 5,
        language: langParam,
        task: 'transcribe',
      });
      output = (Array.isArray(res) ? res[0] : res) as TranscriptionResultShape;
    }

    post({ type: 'stage', id, stage: 'analyzed' });

    // 정렬 DP 도 여기서 돈다. 대본 단어 수 × 밴드 폭만큼 도는 계산이라 긴 녹음에서는
    // 수천만 셀이 되고, 메인 스레드에서 돌리면 전사가 끝난 직후 화면이 다시 굳는다.
    const chunks = normalizeChunks(output);
    const subtitles =
      align.mode === 'ai'
        ? generateSubtitlesFromAiChunks(chunks, align.maxChars)
        : alignScriptWithWhisperChunks(align.lines, chunks, length / 16000);

    post({
      type: 'done',
      id,
      text: (output?.text || '').trim(),
      chunks,
      subtitles,
    });
  } catch (err: unknown) {
    // 세션이 깨진 파이프라인을 그대로 두면 다음 시도도 같은 자리에서 죽는다.
    await disposeCached();
    post({
      type: 'error',
      id,
      message: err instanceof Error ? err.message : String(err),
      whileLoading: false,
    });
  }
}

async function handle(request: WhisperWorkerRequest) {
  if (request.type === 'reset') {
    await disposeCached();
    return;
  }
  await transcribe(request);
}

ctx.onmessage = (event: MessageEvent<WhisperWorkerRequest>) => {
  const request = event.data;
  queue = queue.then(() =>
    handle(request).catch((err: unknown) => {
      // 여기까지 온 예외는 보고 경로 자체가 깨진 경우다. 큐는 계속 살려 둔다.
      console.error('Whisper 워커 처리 실패:', err);
      if (request.type === 'transcribe') {
        post({
          type: 'error',
          id: request.id,
          message: err instanceof Error ? err.message : String(err),
          whileLoading: false,
        });
      }
    }),
  );
};
