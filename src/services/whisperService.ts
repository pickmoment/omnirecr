import type {
  WhisperAlignSpec,
  WhisperResetRequest,
  WhisperTranscribeResult,
  WhisperWorkerResponse,
} from './whisperWorkerProtocol';

/**
 * 취소 전용 오류.
 *
 * 취소는 성공도 실패도 아니다. 취소된 작업을 그냥 resolve 하면 잘린 결과가 '완료'로 보고되고
 * 파일까지 쓰인다. 이 오류로 갈라 놓아야 UI 가 '건너뜀'과 '실패'를 구분한다.
 */
export class SubtitleCancelledError extends Error {
  readonly cancelled = true;

  constructor(message: string = '작업이 취소되었습니다.') {
    super(message);
    this.name = 'SubtitleCancelledError';
  }
}

export function isSubtitleCancelled(err: unknown): err is SubtitleCancelledError {
  return err instanceof SubtitleCancelledError;
}

/** await 경계마다 불러 취소를 즉시 오류로 만든다. 취소가 성공으로 resolve 되면 안 된다. */
export function throwIfCancelled(signal?: AbortSignal) {
  if (signal?.aborted) {
    throw new SubtitleCancelledError();
  }
}

// ── Whisper 워커 클라이언트 ─────────────────────────────────────────────
//
// 추론은 `whisperWorker.ts`(전용 Web Worker)에서 돈다. ONNX Runtime(WASM)의 추론 호출은
// 동기라서 메인 스레드에서 돌리면 그 시간 동안 웹뷰 전체가 멈춘다 — 자막 일괄 생성이
// 중간에 굳어 보이던 원인이다. 여기서는 요청을 보내고 진행 보고를 문구로 옮기기만 한다.

interface PendingTranscription {
  resolve: (result: WhisperTranscribeResult) => void;
  reject: (err: unknown) => void;
  report: (message: string, percent?: number) => void;
}

let worker: Worker | null = null;
let nextRequestId = 1;
const pending = new Map<number, PendingTranscription>();

function failAllPending(err: unknown) {
  const waiting = [...pending.values()];
  pending.clear();
  for (const entry of waiting) entry.reject(err);
}

/**
 * 워커를 끊는다.
 *
 * WASM 추론은 중간에 끼어들 수 없다. 취소 플래그만 두면 사용자가 중단을 눌러도 계산은
 * 끝까지 돌아 CPU 를 계속 먹는다(예전에는 그게 메인 스레드였다). 유일하게 확실한 중단은
 * 워커를 죽이는 것이고, 다음 요청이 새 워커를 띄운다. 모델은 브라우저 캐시에 남아 있어
 * 다시 내려받지 않는다.
 */
function terminateWorker(reason: unknown) {
  if (worker) {
    worker.terminate();
    worker = null;
  }
  failAllPending(reason);
}

function handleWorkerMessage(message: WhisperWorkerResponse) {
  const entry = pending.get(message.id);
  if (!entry) return;

  switch (message.type) {
    case 'load-progress': {
      if (message.status === 'progress' && typeof message.progress === 'number') {
        const pct = Math.round(message.progress);
        entry.report(`AI 모델 다운로드/로딩 중... (${pct}%)`, 10 + Math.round(pct * 0.4));
      }
      return;
    }
    case 'stage': {
      if (message.stage === 'model-ready') {
        entry.report('AI 모델 준비 완료, 음성 전사 시작...', 50);
      } else if (message.stage === 'transcribing') {
        entry.report('로컬 AI가 실제 음성 단어 및 타임코드를 정밀 분석 중입니다...', 60);
      } else if (message.stage === 'segment-fallback') {
        entry.report('단어 타임스탬프 실패 — 구간 타임스탬프로 다시 분석 중...', 70);
      } else {
        entry.report('음성 분석 완료! 대본과 정밀 타임라인 정렬 중...', 95);
      }
      return;
    }
    case 'done': {
      pending.delete(message.id);
      entry.resolve({
        text: message.text,
        chunks: message.chunks,
        subtitles: message.subtitles,
      });
      return;
    }
    case 'error': {
      pending.delete(message.id);
      entry.reject(
        new Error(
          message.whileLoading
            ? `AI 모델 로딩 실패: ${message.message}`
            : `Whisper 음성 분석 중 오류 발생: ${message.message}`,
        ),
      );
      return;
    }
  }
}

function ensureWorker(): Worker {
  if (worker) return worker;

  const created = new Worker(new URL('./whisperWorker.ts', import.meta.url), { type: 'module' });
  created.onmessage = (event: MessageEvent<WhisperWorkerResponse>) =>
    handleWorkerMessage(event.data);
  // 워커 스크립트 자체가 죽은 경우다. 기다리는 요청을 실패로 끝내고 워커를 버린다 —
  // 남겨 두면 다음 요청이 응답 없는 워커에 붙어 영원히 기다린다.
  created.onerror = (event: ErrorEvent) => {
    terminateWorker(new Error(`AI 전사 워커 오류: ${event.message || '알 수 없는 오류'}`));
  };
  created.onmessageerror = () => {
    terminateWorker(new Error('AI 전사 워커 메시지를 해석하지 못했습니다.'));
  };

  worker = created;
  return created;
}

/**
 * 캐시된 모델을 내려놓는다(사용자가 모델을 바꿨을 때).
 *
 * 전사가 도는 중이면 워커에 요청만 보내 **그 작업이 끝난 뒤** 정리하게 한다. 놀고 있으면
 * 워커째 끊어 WASM 힙(모델 수백 MB)을 즉시 돌려준다.
 */
export function resetWhisperPipeline() {
  if (pending.size > 0) {
    const reset: WhisperResetRequest = { type: 'reset' };
    worker?.postMessage(reset);
    return;
  }
  if (worker) {
    worker.terminate();
    worker = null;
  }
}

/**
 * Float32Array PCM(16kHz)을 로컬 Whisper 로 전사하고, 그 결과로 자막까지 만든다.
 *
 * 전사와 정렬을 한 번에 시키는 이유: 둘 다 무거운 계산이라 워커에서 끝내야 하고, 청크를
 * 메인 스레드로 왕복시켜 다시 보내면 큰 배열을 두 번 복제하게 된다.
 *
 * 주의: `audioPcm` 의 버퍼는 워커로 **transfer** 된다. 호출이 끝나면 이쪽 버퍼는 분리되어
 * 길이 0 이 되므로 호출자는 넘긴 뒤 다시 읽지 않는다. 사본을 뜨면 1시간 오디오(5,760만
 * 샘플)에서 230MB 를 더 태운다.
 */
export async function runLocalWhisperTranscribe(
  audioPcm: Float32Array,
  modelId: string = 'Xenova/whisper-base',
  onProgress?: (statusMsg: string, percent?: number) => void,
  language: string = 'korean',
  signal?: AbortSignal,
  align: WhisperAlignSpec = { mode: 'ai', maxChars: 28 }
): Promise<WhisperTranscribeResult> {
  // 취소된 뒤에는 진행 콜백을 부르지 않는다. 언마운트된 화면의 setState 를 깨우면 안 된다.
  const report = (message: string, percent?: number) => {
    if (signal?.aborted) return;
    onProgress?.(message, percent);
  };

  throwIfCancelled(signal);
  report('AI 모델을 준비하고 있습니다...', 10);

  const active = ensureWorker();
  const id = nextRequestId++;
  const buffer = audioPcm.buffer as ArrayBuffer;
  const byteOffset = audioPcm.byteOffset;
  const length = audioPcm.length;

  const onAbort = () => terminateWorker(new SubtitleCancelledError());
  signal?.addEventListener('abort', onAbort, { once: true });

  try {
    return await new Promise<WhisperTranscribeResult>((resolve, reject) => {
      pending.set(id, { resolve, reject, report });
      active.postMessage(
        { type: 'transcribe', id, modelId, language, buffer, byteOffset, length, align },
        [buffer],
      );
    });
  } finally {
    pending.delete(id);
    signal?.removeEventListener('abort', onAbort);
  }
}

