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
        const rawStart = typeof c.timestamp[0] === 'number' && !isNaN(c.timestamp[0]) ? c.timestamp[0] : 0;
        const rawEnd = typeof c.timestamp[1] === 'number' && !isNaN(c.timestamp[1]) ? c.timestamp[1] : rawStart + 0.5;
        const start = Math.max(0, rawStart);
        const end = Math.max(start + 0.1, rawEnd);
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

  // --- Pre-build prefix-concatenated strings to avoid repeated slice+join ---
  // prefixStr[i] = tokens[0..i-1] joined by space; prefixLen[i] = prefixStr[i].length
  // Substring for tokens [a..b] = prefixStr[b+1].substring(prefixLen[a])
  const tokenTexts = tokens.map((t) => t.text);
  const prefixStr: string[] = new Array(numTokens + 1);
  const prefixLen: number[] = new Array(numTokens + 1);
  prefixStr[0] = '';
  prefixLen[0] = 0;
  for (let i = 0; i < numTokens; i++) {
    prefixStr[i + 1] = i === 0 ? tokenTexts[i] : prefixStr[i] + ' ' + tokenTexts[i];
    prefixLen[i + 1] = prefixStr[i + 1].length;
  }

  // Helper: get concatenated text for tokens [a..b] without allocation
  function rangeText(a: number, b: number): string {
    // Offset into prefixStr[b+1] starting after prefixStr[a]
    const startOffset = a === 0 ? 0 : prefixLen[a] + 1; // +1 for the space separator
    return prefixStr[b + 1].substring(startOffset);
  }

  // Compute character weight of each line for proportional token allocation
  const lineWeights = scriptLines.map((l) => Math.max(1, cleanText(l).length));
  const totalLineWeight = lineWeights.reduce((a, b) => a + b, 0);

  // Proportional token allocation
  const proportionalSizes: number[] = [];
  for (let i = 0; i < numLines; i++) {
    proportionalSizes.push(Math.max(1, Math.round((lineWeights[i] / totalLineWeight) * numTokens)));
  }
  let totalAllocated = proportionalSizes.reduce((a, b) => a + b, 0);
  while (totalAllocated < numTokens) {
    let maxIdx = 0;
    for (let j = 1; j < numLines; j++) {
      if (proportionalSizes[j] > proportionalSizes[maxIdx]) maxIdx = j;
    }
    proportionalSizes[maxIdx]++;
    totalAllocated++;
  }
  while (totalAllocated > numTokens && numLines > 0) {
    let maxIdx = 0;
    for (let j = 1; j < numLines; j++) {
      if (proportionalSizes[j] > proportionalSizes[maxIdx]) maxIdx = j;
    }
    if (proportionalSizes[maxIdx] > 1) {
      proportionalSizes[maxIdx]--;
      totalAllocated--;
    } else {
      break;
    }
  }

  // --- Pre-allocate reusable DP buffers for LCS (max line length ~200 chars) ---
  let dpBufSize = 0;
  for (let i = 0; i < numLines; i++) {
    const len = cleanText(scriptLines[i]).length;
    if (len > dpBufSize) dpBufSize = len;
  }
  dpBufSize += 1;
  let dpPrev = new Uint16Array(dpBufSize);
  let dpCurr = new Uint16Array(dpBufSize);

  // Inline LCS ratio using pre-allocated buffers — avoids per-call allocation
  function lcsRatioFast(a: string, b: string): number {
    let short = a, long = b;
    if (a.length > b.length) { short = b; long = a; }
    const m = short.length;
    const n = long.length;
    if (m === 0 || n === 0) return 0;

    // Grow buffers if needed (rare — only if candidate text exceeds initial estimate)
    if (m + 1 > dpPrev.length) {
      dpPrev = new Uint16Array(m + 1);
      dpCurr = new Uint16Array(m + 1);
    }

    dpPrev.fill(0, 0, m + 1);

    for (let j = 1; j <= n; j++) {
      dpCurr[0] = 0;
      const bChar = long.charCodeAt(j - 1);
      for (let i = 1; i <= m; i++) {
        if (short.charCodeAt(i - 1) === bChar) {
          dpCurr[i] = dpPrev[i - 1] + 1;
        } else {
          dpCurr[i] = dpCurr[i - 1] > dpPrev[i] ? dpCurr[i - 1] : dpPrev[i];
        }
      }
      const swap = dpPrev; dpPrev = dpCurr; dpCurr = swap;
    }

    const lcsLen = dpPrev[m];
    return (2 * lcsLen) / (m + n);
  }

  // --- Phase 1: Coarse-then-fine fuzzy alignment ---
  interface LineMatch {
    startToken: number;
    endToken: number;
    score: number;
  }

  const lineMatches: LineMatch[] = new Array(numLines);
  let searchFloor = 0;

  // Early-exit threshold: a near-perfect match means no need to keep scanning
  const GOOD_ENOUGH = 0.85;

  for (let i = 0; i < numLines; i++) {
    const lineClean = cleanText(scriptLines[i]);
    const expectedCount = proportionalSizes[i];

    const remainingLines = numLines - i - 1;
    const searchCeiling = Math.min(numTokens, numTokens - remainingLines);

    const windowHalf = Math.max(15, Math.round(expectedCount * 2));
    const scanFrom = searchFloor; // monotonicity: never go back
    const scanTo = Math.min(searchCeiling, searchFloor + expectedCount + windowHalf);

    let bestScore = -Infinity;
    let bestStart = searchFloor;
    let bestEnd = Math.min(numTokens - 1, searchFloor + expectedCount - 1);

    const minSpan = Math.max(1, Math.round(expectedCount * 0.5));
    const maxSpan = Math.min(scanTo - scanFrom, Math.round(expectedCount * 2.0));

    // Coarse pass: stride 2 over start positions, only expectedCount span
    let coarseBestStart = scanFrom;
    let coarseBestScore = -Infinity;

    for (let start = scanFrom; start < scanTo; start += 2) {
      const end = Math.min(numTokens - 1, start + expectedCount - 1);
      if (end < start || end >= searchCeiling + 3) continue;

      const candidate = rangeText(start, end);
      const score = lcsRatioFast(lineClean, candidate);

      if (score > coarseBestScore) {
        coarseBestScore = score;
        coarseBestStart = start;
        if (score >= GOOD_ENOUGH) break;
      }
    }

    // Fine pass: dense search around coarse winner ± 3 positions, with span variations
    const fineFrom = Math.max(scanFrom, coarseBestStart - 3);
    const fineTo = Math.min(scanTo, coarseBestStart + 4);

    for (let start = fineFrom; start < fineTo; start++) {
      for (let spanDelta = -2; spanDelta <= 3; spanDelta++) {
        const span = expectedCount + spanDelta;
        if (span < minSpan || span > maxSpan) continue;
        const end = Math.min(numTokens - 1, start + span - 1);
        if (end < start || end >= searchCeiling + 3) continue;

        const candidate = rangeText(start, end);
        const score = lcsRatioFast(lineClean, candidate);

        if (score > bestScore) {
          bestScore = score;
          bestStart = start;
          bestEnd = end;
          if (score >= 0.95) break; // near-perfect in fine pass — stop
        }
      }
      if (bestScore >= 0.95) break;
    }

    // Use coarse result if fine pass didn't beat it
    if (coarseBestScore > bestScore) {
      bestScore = coarseBestScore;
      bestStart = coarseBestStart;
      bestEnd = Math.min(numTokens - 1, coarseBestStart + expectedCount - 1);
    }

    lineMatches[i] = { startToken: bestStart, endToken: bestEnd, score: bestScore };
    searchFloor = bestEnd + 1;
  }

  // --- Phase 2: Build subtitle items from Whisper's actual speech timestamps ---
  const results: SubtitleItem[] = [];

  for (let i = 0; i < numLines; i++) {
    const match = lineMatches[i];
    const startTok = tokens[match.startToken] || tokens[0];
    const endTok = tokens[match.endToken] || startTok;

    let startTime = startTok.start;
    let endTime = Math.max(startTime + 0.35, endTok.end);

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
      text: scriptLines[i],
    });
  }

  return results;
}

/**
 * Generate Subtitles directly from AI Whisper transcription chunks (No Script needed)
 */
export function generateSubtitlesFromAiChunks(
  chunks: WhisperChunk[],
  maxChars: number = 28
): SubtitleItem[] {
  if (chunks.length === 0) return [];

  const results: SubtitleItem[] = [];
  let currentText = '';
  let currentStart = 0;
  let currentEnd = 0;
  let hasActive = false;

  for (const chunk of chunks) {
    const chunkText = chunk.text.trim();
    if (!chunkText) continue;

    const chunkStart = Math.max(0, chunk.timestamp[0]);
    const chunkEnd = Math.max(chunkStart + 0.2, chunk.timestamp[1]);

    if (!hasActive) {
      currentText = chunkText;
      currentStart = chunkStart;
      currentEnd = chunkEnd;
      hasActive = true;
    } else {
      const isEndingPunct = /[.?!…~。]$/.test(currentText);
      const isLongEnough = (currentText.length + chunkText.length + 1) > maxChars;
      const hasLargeGap = chunkStart > currentEnd + 0.6; // Silence gap > 0.6s

      if (isEndingPunct || isLongEnough || hasLargeGap) {
        // Finalize current subtitle
        results.push({
          index: results.length + 1,
          start_secs: currentStart,
          end_secs: currentEnd,
          start_formatted: formatSrtTime(currentStart),
          end_formatted: formatSrtTime(currentEnd),
          text: currentText,
        });

        currentText = chunkText;
        currentStart = chunkStart;
        currentEnd = chunkEnd;
      } else {
        currentText += (currentText ? ' ' : '') + chunkText;
        currentEnd = Math.max(currentEnd, chunkEnd);
      }
    }
  }

  if (hasActive && currentText.trim()) {
    results.push({
      index: results.length + 1,
      start_secs: currentStart,
      end_secs: currentEnd,
      start_formatted: formatSrtTime(currentStart),
      end_formatted: formatSrtTime(currentEnd),
      text: currentText,
    });
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

