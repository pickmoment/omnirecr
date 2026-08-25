import { pipeline, env } from '@xenova/transformers';
import type { SubtitleItem } from '../types';

// Configure transformers.js for local/in-browser execution
env.allowLocalModels = false;
env.useBrowserCache = true;

export interface WhisperChunk {
  text: string;
  timestamp: [number, number];
}

export interface WhisperTranscribeResult {
  text: string;
  chunks: WhisperChunk[];
}

let transcriberInstance: any = null;
let currentModelName: string | null = null;

/**
 * Load or reuse Whisper ASR pipeline
 */
export async function getWhisperPipeline(
  modelId: string = 'Xenova/whisper-tiny',
  onProgress?: (progress: { status: string; progress?: number; file?: string }) => void
) {
  if (transcriberInstance && currentModelName === modelId) {
    return transcriberInstance;
  }

  transcriberInstance = await pipeline('automatic-speech-recognition', modelId, {
    progress_callback: onProgress,
  });
  currentModelName = modelId;
  return transcriberInstance;
}

/**
 * Transcribe Float32Array PCM audio using local Whisper AI
 */
export async function runLocalWhisperTranscribe(
  audioPcm: Float32Array,
  modelId: string = 'Xenova/whisper-tiny',
  onProgress?: (statusMsg: string, percent?: number) => void
): Promise<WhisperTranscribeResult> {
  onProgress?.('AI 모델을 준비하고 있습니다...', 10);

  const transcriber = await getWhisperPipeline(modelId, (data) => {
    if (data.status === 'progress' && typeof data.progress === 'number') {
      const pct = Math.round(data.progress);
      onProgress?.(`AI 모델 로딩 중... (${pct}%)`, 10 + Math.round(pct * 0.4));
    } else if (data.status === 'done') {
      onProgress?.('AI 모델 준비 완료, 음성 전사 시작...', 50);
    }
  });

  onProgress?.('로컬 AI가 실제 음성 단어 및 타임코드를 분석 중입니다...', 60);

  const output = await transcriber(audioPcm, {
    return_timestamps: true,
    chunk_length_s: 30,
    stride_length_s: 5,
    language: 'korean',
    task: 'transcribe',
  });

  onProgress?.('음성 분석 완료! 대본과 정밀 정렬 중...', 95);

  const chunks: WhisperChunk[] = [];
  if (output && Array.isArray(output.chunks)) {
    for (const c of output.chunks) {
      if (c && c.timestamp && Array.isArray(c.timestamp)) {
        const start = Math.max(0, c.timestamp[0] ?? 0);
        const end = Math.max(start + 0.1, c.timestamp[1] ?? start + 1.0);
        chunks.push({
          text: (c.text || '').trim(),
          timestamp: [start, end],
        });
      }
    }
  }

  return {
    text: output.text || '',
    chunks,
  };
}

/**
 * Forced Alignment: Align user script lines with AI Whisper detected speech chunks
 */
export function alignScriptWithWhisperChunks(
  scriptLines: string[],
  chunks: WhisperChunk[],
  totalDuration: number
): SubtitleItem[] {
  if (scriptLines.length === 0) return [];

  // If no chunks produced by whisper, return simple fallback
  if (chunks.length === 0) {
    const durPerLine = totalDuration / scriptLines.length;
    return scriptLines.map((line, idx) => {
      const st = idx * durPerLine;
      const et = (idx + 1) * durPerLine;
      return {
        index: idx + 1,
        start_secs: st,
        end_secs: et,
        start_formatted: formatSrtTime(st),
        end_formatted: formatSrtTime(et),
        text: line,
      };
    });
  }

  // Flatten chunks into word tokens with timestamps
  const tokens: Array<{ text: string; start: number; end: number }> = [];
  for (const chunk of chunks) {
    const words = chunk.text.split(/\s+/).filter((w) => w.length > 0);
    if (words.length === 0) continue;
    const chunkDur = chunk.timestamp[1] - chunk.timestamp[0];
    const wordDur = chunkDur / words.length;

    words.forEach((w, i) => {
      tokens.push({
        text: cleanText(w),
        start: chunk.timestamp[0] + i * wordDur,
        end: chunk.timestamp[0] + (i + 1) * wordDur,
      });
    });
  }

  // If token list is empty, treat each chunk as a unit
  if (tokens.length === 0) {
    return chunks.map((c, idx) => ({
      index: idx + 1,
      start_secs: c.timestamp[0],
      end_secs: c.timestamp[1],
      start_formatted: formatSrtTime(c.timestamp[0]),
      end_formatted: formatSrtTime(c.timestamp[1]),
      text: scriptLines[idx] || c.text,
    }));
  }

  // Calculate character length weight of each script line
  const lineWeights = scriptLines.map((l) => Math.max(1, cleanText(l).length));
  const totalLineWeight = lineWeights.reduce((a, b) => a + b, 0);

  const numLines = scriptLines.length;
  const numTokens = tokens.length;

  let results: SubtitleItem[] = [];
  let tokenIdx = 0;

  for (let i = 0; i < numLines; i++) {
    const line = scriptLines[i];
    const lineWeight = lineWeights[i];
    const tokenQuota = Math.max(1, Math.round((lineWeight / totalLineWeight) * numTokens));

    const startToken = tokens[tokenIdx] || tokens[tokens.length - 1];
    const endTokenIdx = Math.min(tokens.length - 1, tokenIdx + tokenQuota - 1);
    const endToken = tokens[endTokenIdx] || startToken;

    let startTime = startToken.start;
    let endTime = Math.max(startTime + 0.35, endToken.end);

    // Prevent overlap with previous subtitle
    if (results.length > 0) {
      const prevEnd = results[results.length - 1].end_secs;
      if (startTime < prevEnd + 0.05) {
        startTime = prevEnd + 0.05;
      }
      endTime = Math.max(startTime + 0.35, endTime);
    }

    results.push({
      index: i + 1,
      start_secs: startTime,
      end_secs: endTime,
      start_formatted: formatSrtTime(startTime),
      end_formatted: formatSrtTime(endTime),
      text: line,
    });

    tokenIdx = Math.min(tokens.length - 1, tokenIdx + tokenQuota);
  }

  return results;
}

function cleanText(t: string): string {
  return t.replace(/[.,?!…~'"`\-_/\\]/g, '').toLowerCase().trim();
}

function formatSrtTime(secs: number): string {
  const totalMillis = Math.round(Math.max(0, secs) * 1000);
  const ms = totalMillis % 1000;
  const s = Math.floor(totalMillis / 1000) % 60;
  const m = Math.floor(totalMillis / (1000 * 60)) % 60;
  const h = Math.floor(totalMillis / (1000 * 60 * 60));
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')},${String(ms).padStart(3, '0')}`;
}
