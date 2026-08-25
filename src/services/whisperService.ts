import { pipeline, env } from '@xenova/transformers';
import type { SubtitleItem, SubtitleSplitMode } from '../types';

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

export function resetWhisperPipeline() {
  transcriberInstance = null;
  currentModelName = null;
}

/**
 * Load or reuse Whisper ASR pipeline safely
 */
export async function getWhisperPipeline(
  modelId: string = 'Xenova/whisper-tiny',
  onProgress?: (progress: { status: string; progress?: number; file?: string }) => void
) {
  if (transcriberInstance && currentModelName === modelId) {
    return transcriberInstance;
  }

  try {
    transcriberInstance = await pipeline('automatic-speech-recognition', modelId, {
      progress_callback: onProgress,
    });
    currentModelName = modelId;
    return transcriberInstance;
  } catch (err) {
    transcriberInstance = null;
    currentModelName = null;
    throw err;
  }
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

  let transcriber;
  try {
    transcriber = await getWhisperPipeline(modelId, (data) => {
      if (data.status === 'progress' && typeof data.progress === 'number') {
        const pct = Math.round(data.progress);
        onProgress?.(`AI 모델 다운로드/로딩 중... (${pct}%)`, 10 + Math.round(pct * 0.4));
      } else if (data.status === 'done') {
        onProgress?.('AI 모델 준비 완료, 음성 전사 시작...', 50);
      }
    });
  } catch (err: any) {
    resetWhisperPipeline();
    throw new Error(`AI 모델 로딩 실패: ${err?.message || err}`);
  }

  onProgress?.('로컬 AI가 실제 음성 단어 및 타임코드를 분석 중입니다...', 60);

  const cleanPcm = new Float32Array(audioPcm);

  let output;
  try {
    output = await transcriber(cleanPcm, {
      return_timestamps: true,
      chunk_length_s: 30,
      stride_length_s: 5,
      language: 'korean',
      task: 'transcribe',
    });
  } catch (err: any) {
    resetWhisperPipeline();
    throw new Error(`Whisper 음성 분석 중 오류 발생: ${err?.message || err}`);
  }

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
    text: output?.text || '',
    chunks,
  };
}

/**
 * Robust sentence and chunk splitter for subtitles
 */
export function splitScriptIntoLines(
  rawText: string,
  mode: SubtitleSplitMode = 'sentence',
  maxChars: number = 28
): string[] {
  // 1. Remove timestamps like [00:01.00]
  const lrcRegex = /\[\d{1,2}:\d{2}(?:\.\d+)?\]/g;
  const srtRegex = /\d{2}:\d{2}:\d{2}[,\.]\d{3}\s*-->\s*\d{2}:\d{2}:\d{2}[,\.]\d{3}/g;
  const cleanRaw = rawText
    .replace(lrcRegex, '')
    .replace(srtRegex, '')
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n');

  // 2. Line mode: return non-empty lines directly
  if (mode === 'line') {
    return cleanRaw
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l.length > 0);
  }

  // 3. Sentence / Auto / Length splitting
  const initialLines = cleanRaw.split('\n').map((l) => l.trim()).filter((l) => l.length > 0);
  const sentences: string[] = [];

  for (const line of initialLines) {
    // Regex splits by: . ? ! … 。 optionally followed by closing quotes, when not part of decimals (e.g. 3.14)
    // We match sentence boundaries robustly
    const parts: string[] = [];
    let cur = '';

    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      cur += ch;

      const isEndingPunct = ch === '.' || ch === '?' || ch === '!' || ch === '…' || ch === '。' || ch === '~';
      if (isEndingPunct) {
        // Check if next char is decimal digit (e.g., 3.14)
        const isDecimal = ch === '.' && i > 0 && /\d/.test(line[i - 1]) && i + 1 < line.length && /\d/.test(line[i + 1]);
        if (!isDecimal) {
          // Check if followed by closing quote or parenthesis
          let nextIdx = i + 1;
          while (nextIdx < line.length && /["'”’」』\)\]]/.test(line[nextIdx])) {
            cur += line[nextIdx];
            i = nextIdx;
            nextIdx++;
          }

          if (cur.trim()) {
            parts.push(cur.trim());
            cur = '';
          }
        }
      }
    }

    if (cur.trim()) {
      parts.push(cur.trim());
    }

    // If no punctuation was found in this line, the whole line is treated as 1 sentence
    if (parts.length === 0 && line.trim()) {
      parts.push(line.trim());
    }

    for (const p of parts) {
      if (mode === 'sentence') {
        sentences.push(p);
      } else if (mode === 'auto' || mode === 'length') {
        if (p.length <= maxChars) {
          sentences.push(p);
        } else {
          // Split long sentence by commas or spaces into sub-chunks
          const subChunks = splitByLength(p, maxChars);
          sentences.push(...subChunks);
        }
      }
    }
  }

  return sentences.filter((s) => s.length > 0);
}

function splitByLength(text: string, maxChars: number): string[] {
  if (text.length <= maxChars) return [text];

  const words = text.split(/\s+/).filter((w) => w.length > 0);
  if (words.length <= 1) return [text];

  const chunks: string[] = [];
  let current = '';

  for (const w of words) {
    if (current.length + w.length + 1 > maxChars && current.length > 0) {
      chunks.push(current.trim());
      current = w;
    } else {
      current += (current ? ' ' : '') + w;
    }
  }

  if (current.trim()) {
    chunks.push(current.trim());
  }

  return chunks;
}

/**
 * Forced Alignment: Align user script lines with AI Whisper detected speech chunks
 * using text search and token timestamp mapping
 */
export function alignScriptWithWhisperChunks(
  scriptLines: string[],
  chunks: WhisperChunk[],
  totalDuration: number
): SubtitleItem[] {
  if (scriptLines.length === 0) return [];

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
    const chunkDur = Math.max(0.1, chunk.timestamp[1] - chunk.timestamp[0]);
    const wordDur = chunkDur / words.length;

    words.forEach((w, i) => {
      tokens.push({
        text: cleanText(w),
        start: chunk.timestamp[0] + i * wordDur,
        end: chunk.timestamp[0] + (i + 1) * wordDur,
      });
    });
  }

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

  const numLines = scriptLines.length;
  const numTokens = tokens.length;

  // Compute character weight of each line
  const lineWeights = scriptLines.map((l) => Math.max(1, cleanText(l).length));
  const totalLineWeight = lineWeights.reduce((a, b) => a + b, 0);

  const results: SubtitleItem[] = [];
  let tokenIdx = 0;

  for (let i = 0; i < numLines; i++) {
    const line = scriptLines[i];
    const lineClean = cleanText(line);
    const lineWords = lineClean.split(/\s+/).filter((w) => w.length > 0);

    // Try to find the matching words in tokens starting from tokenIdx
    let matchStartIdx = tokenIdx;
    let matchEndIdx = tokenIdx;

    const firstWord = lineWords[0];
    const lastWord = lineWords[lineWords.length - 1];

    // Search for firstWord within a search window of next 15 tokens
    if (firstWord) {
      for (let s = tokenIdx; s < Math.min(numTokens, tokenIdx + 15); s++) {
        if (tokens[s].text.includes(firstWord) || firstWord.includes(tokens[s].text)) {
          matchStartIdx = s;
          break;
        }
      }
    }

    // Expected token count for this line
    const expectedTokenCount = Math.max(1, Math.round((lineWeights[i] / totalLineWeight) * numTokens));
    matchEndIdx = Math.min(numTokens - 1, matchStartIdx + expectedTokenCount - 1);

    // Refine matchEndIdx with lastWord if possible
    if (lastWord) {
      for (let e = matchStartIdx; e < Math.min(numTokens, matchStartIdx + expectedTokenCount + 10); e++) {
        if (tokens[e].text.includes(lastWord) || lastWord.includes(tokens[e].text)) {
          matchEndIdx = e;
          break;
        }
      }
    }

    const startToken = tokens[matchStartIdx] || tokens[tokenIdx] || tokens[0];
    const endToken = tokens[matchEndIdx] || startToken;

    let startTime = startToken.start;
    let endTime = Math.max(startTime + 0.35, endToken.end);

    // Ensure non-overlapping monotonicity
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

    tokenIdx = Math.min(numTokens - 1, matchEndIdx + 1);
  }

  return results;
}

function cleanText(t: string): string {
  return t.replace(/[.,?!…~'"`\-_/\\()\[\]]/g, '').toLowerCase().trim();
}

function formatSrtTime(secs: number): string {
  const totalMillis = Math.round(Math.max(0, secs) * 1000);
  const ms = totalMillis % 1000;
  const s = Math.floor(totalMillis / 1000) % 60;
  const m = Math.floor(totalMillis / (1000 * 60)) % 60;
  const h = Math.floor(totalMillis / (1000 * 60 * 60));
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')},${String(ms).padStart(3, '0')}`;
}
