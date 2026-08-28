import { pipeline, env, type AutomaticSpeechRecognitionPipeline } from '@xenova/transformers';
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

let transcriberInstance: AutomaticSpeechRecognitionPipeline | null = null;
let currentModelName: string | null = null;

export function resetWhisperPipeline() {
  transcriberInstance = null;
  currentModelName = null;
}

/**
 * Load or reuse Whisper ASR pipeline safely
 */
export async function getWhisperPipeline(
  modelId: string = 'Xenova/whisper-base',
  onProgress?: (progress: { status: string; progress?: number; file?: string }) => void
): Promise<AutomaticSpeechRecognitionPipeline> {
  if (transcriberInstance && currentModelName === modelId) {
    return transcriberInstance;
  }

  try {
    const pipe = (await pipeline('automatic-speech-recognition', modelId, {
      progress_callback: onProgress,
    })) as AutomaticSpeechRecognitionPipeline;
    transcriberInstance = pipe;
    currentModelName = modelId;
    return transcriberInstance;
  } catch (err) {
    transcriberInstance = null;
    currentModelName = null;
    throw err;
  }
}

/**
 * Transcribe Float32Array PCM audio using local Whisper AI with exact token/word timestamps
 */
export async function runLocalWhisperTranscribe(
  audioPcm: Float32Array,
  modelId: string = 'Xenova/whisper-base',
  onProgress?: (statusMsg: string, percent?: number) => void,
  language: string = 'korean'
): Promise<WhisperTranscribeResult> {
  onProgress?.('AI 모델을 준비하고 있습니다...', 10);

  let transcriber: AutomaticSpeechRecognitionPipeline;
  try {
    transcriber = await getWhisperPipeline(modelId, (data) => {
      if (data.status === 'progress' && typeof data.progress === 'number') {
        const pct = Math.round(data.progress);
        onProgress?.(`AI 모델 다운로드/로딩 중... (${pct}%)`, 10 + Math.round(pct * 0.4));
      } else if (data.status === 'done') {
        onProgress?.('AI 모델 준비 완료, 음성 전사 시작...', 50);
      }
    });
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    resetWhisperPipeline();
    throw new Error(`AI 모델 로딩 실패: ${msg}`);
  }

  onProgress?.('로컬 AI가 실제 음성 단어 및 타임코드를 정밀 분석 중입니다...', 60);

  const cleanPcm = new Float32Array(audioPcm);
  const langParam = language === 'auto' || !language ? undefined : language;

  interface TranscriptionResultShape {
    text?: string;
    chunks?: Array<{ text?: string; timestamp?: [number, number] }>;
  }

  let output: TranscriptionResultShape | null = null;

  // 1. First attempt: Word-level timestamps for sub-second precision
  try {
    const res = await transcriber(cleanPcm, {
      return_timestamps: 'word',
      chunk_length_s: 30,
      stride_length_s: 5,
      language: langParam,
      task: 'transcribe',
    });
    output = (Array.isArray(res) ? res[0] : res) as TranscriptionResultShape;
  } catch (wordErr) {
    console.warn('Word-level timestamps failed, falling back to segment timestamps:', wordErr);
    // 2. Fallback attempt: Segment timestamps
    try {
      const res = await transcriber(cleanPcm, {
        return_timestamps: true,
        chunk_length_s: 30,
        stride_length_s: 5,
        language: langParam,
        task: 'transcribe',
      });
      output = (Array.isArray(res) ? res[0] : res) as TranscriptionResultShape;
    } catch (segErr: unknown) {
      const msg = segErr instanceof Error ? segErr.message : String(segErr);
      resetWhisperPipeline();
      throw new Error(`Whisper 음성 분석 중 오류 발생: ${msg}`);
    }
  }

  onProgress?.('음성 분석 완료! 대본과 정밀 타임라인 정렬 중...', 95);

  const rawChunks: WhisperChunk[] = [];
  if (output && Array.isArray(output.chunks)) {
    for (const c of output.chunks) {
      if (c && c.timestamp && Array.isArray(c.timestamp)) {
        const rawStart = typeof c.timestamp[0] === 'number' && !isNaN(c.timestamp[0]) ? c.timestamp[0] : 0;
        const rawEnd = typeof c.timestamp[1] === 'number' && !isNaN(c.timestamp[1]) ? c.timestamp[1] : rawStart + 0.3;
        const start = Math.max(0, rawStart);
        const end = Math.max(start + 0.05, rawEnd);
        const text = (c.text || '').trim();
        if (text.length > 0) {
          rawChunks.push({
            text,
            timestamp: [start, end],
          });
        }
      }
    }
  }

  return {
    text: (output?.text || '').trim(),
    chunks: rawChunks,
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
  // 1. Remove timestamps like [00:01.00] or SRT format headers
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

// Hangul Jamo Decomposition for phonetic similarity
const HANGUL_CHOSUNG = ['ㄱ','ㄲ','ㄴ','ㄷ','ㄸ','ㄹ','ㅁ','ㅂ','ㅃ','ㅅ','ㅆ','ㅇ','ㅈ','ㅉ','ㅊ','ㅋ','ㅌ','ㅍ','ㅎ'];
const HANGUL_JUNGSUNG = ['ㅏ','ㅐ','ㅑ','ㅒ','ㅓ','ㅔ','ㅕ','ㅖ','ㅗ','ㅘ','ㅙ','ㅚ','ㅛ','ㅜ','ㅝ','ㅞ','ㅟ','ㅠ','ㅡ','ㅢ','ㅣ'];
const HANGUL_JONGSUNG = ['', 'ㄱ','ㄲ','ㄳ','ㄴ','ㄵ','ㄶ','ㄷ','ㄹ','ㄺ','ㄻ','ㄼ','ㄽ','ㄾ','ㄿ','ㅀ','ㅁ','ㅂ','ㅄ','ㅅ','ㅆ','ㅇ','ㅈ','ㅊ','ㅋ','ㅌ','ㅍ','ㅎ'];

export function decomposeHangul(str: string): string {
  let result = '';
  for (let i = 0; i < str.length; i++) {
    const code = str.charCodeAt(i);
    if (code >= 0xAC00 && code <= 0xD7A3) {
      const offset = code - 0xAC00;
      const cho = Math.floor(offset / (21 * 28));
      const jung = Math.floor((offset % (21 * 28)) / 28);
      const jong = offset % 28;
      result += HANGUL_CHOSUNG[cho] + HANGUL_JUNGSUNG[jung] + (HANGUL_JONGSUNG[jong] || '');
    } else {
      result += str[i].toLowerCase();
    }
  }
  return result;
}

export function cleanText(t: string): string {
  return t.replace(/[.,?!…~'"`\-_/\\()\[\]{}·:;]/g, '').toLowerCase().trim();
}

export function normalizeNumbersAndUnits(str: string): string {
  let s = str.toLowerCase();
  s = s.replace(/(\d+)\s*%/g, '$1프로');
  s = s.replace(/10000/g, '만');
  s = s.replace(/1000/g, '천');
  s = s.replace(/100/g, '백');
  s = s.replace(/10/g, '십');
  s = s.replace(/1/g, '일').replace(/2/g, '이').replace(/3/g, '삼').replace(/4/g, '사').replace(/5/g, '오')
       .replace(/6/g, '육').replace(/7/g, '칠').replace(/8/g, '팔').replace(/9/g, '구').replace(/0/g, '영');
  return s;
}

function levenshteinRatio(s1: string, s2: string): number {
  if (s1 === s2) return 1.0;
  if (!s1.length || !s2.length) return 0.0;
  const m = s1.length;
  const n = s2.length;
  const dp = new Array(n + 1);
  for (let j = 0; j <= n; j++) dp[j] = j;

  for (let i = 1; i <= m; i++) {
    let prev = dp[0];
    dp[0] = i;
    const c1 = s1.charCodeAt(i - 1);
    for (let j = 1; j <= n; j++) {
      const temp = dp[j];
      if (c1 === s2.charCodeAt(j - 1)) {
        dp[j] = prev;
      } else {
        dp[j] = 1 + Math.min(prev, dp[j], dp[j - 1]);
      }
      prev = temp;
    }
  }
  const maxLen = Math.max(m, n);
  return 1 - dp[n] / maxLen;
}

const COMMON_ALIASES: Record<string, string[]> = {
  'ai': ['에이아이', '인공지능'],
  'rec': ['녹음', '레코드', '레코더', '렉'],
  'omnirec': ['옴니렉', '옴니레코드', '옴니레코더'],
  'srt': ['에스알티', '자막'],
  'vtt': ['브이티티'],
  'pcm': ['피씨엠'],
  'db': ['디비', '데시벨'],
  'fps': ['에프피에스', '프레임'],
};

export function computeWordSimilarity(w1: string, w2: string): number {
  const c1 = cleanText(normalizeNumbersAndUnits(w1));
  const c2 = cleanText(normalizeNumbersAndUnits(w2));
  if (!c1 || !c2) return 0.0;
  if (c1 === c2) return 1.0;

  // Acronym / Loanword lookup
  if (COMMON_ALIASES[c1] && COMMON_ALIASES[c1].some((alias) => c2.includes(alias) || alias === c2)) return 0.95;
  if (COMMON_ALIASES[c2] && COMMON_ALIASES[c2].some((alias) => c1.includes(alias) || alias === c1)) return 0.95;

  // Substring inclusion
  if (c1.includes(c2) || c2.includes(c1)) {
    const minLen = Math.min(c1.length, c2.length);
    const maxLen = Math.max(c1.length, c2.length);
    return (minLen / maxLen) * 0.92;
  }

  // Jamo similarity for Korean spelling variants
  const jamo1 = decomposeHangul(c1);
  const jamo2 = decomposeHangul(c2);
  const jamoSim = levenshteinRatio(jamo1, jamo2);
  if (jamoSim >= 0.65) {
    return jamoSim * 0.9;
  }

  // Character Levenshtein
  const charSim = levenshteinRatio(c1, c2);
  return charSim * 0.85;
}

interface FlattenedToken {
  rawText: string;
  clean: string;
  start: number;
  end: number;
}

interface ScriptWord {
  rawText: string;
  clean: string;
  lineIndex: number;
  wordIndex: number;
}

/**
 * Forced Alignment: Align user script lines with AI Whisper speech chunks
 * using Global Dynamic Programming Sequence Alignment (Needleman-Wunsch with banded DP)
 */
export function alignScriptWithWhisperChunks(
  scriptLines: string[],
  chunks: WhisperChunk[],
  totalDuration: number
): SubtitleItem[] {
  if (scriptLines.length === 0) return [];

  // 1. Flatten Whisper chunks into word tokens with timestamps
  const tokens: FlattenedToken[] = [];
  for (const chunk of chunks) {
    const rawWords = (chunk.text || '').split(/\s+/).filter((w) => w.length > 0);
    if (rawWords.length === 0) continue;

    const start = Math.max(0, chunk.timestamp[0]);
    const end = Math.max(start + 0.05, chunk.timestamp[1]);
    const dur = end - start;

    if (rawWords.length === 1) {
      tokens.push({
        rawText: rawWords[0],
        clean: cleanText(normalizeNumbersAndUnits(rawWords[0])),
        start,
        end,
      });
    } else {
      // Multiple words in chunk (fallback): distribute duration by character length
      const weights = rawWords.map((w) => Math.max(1, cleanText(w).length));
      const totalW = weights.reduce((a, b) => a + b, 0);

      let curStart = start;
      for (let i = 0; i < rawWords.length; i++) {
        const w = rawWords[i];
        const wDur = (weights[i] / totalW) * dur;
        const wEnd = i === rawWords.length - 1 ? end : curStart + wDur;
        tokens.push({
          rawText: w,
          clean: cleanText(normalizeNumbersAndUnits(w)),
          start: curStart,
          end: wEnd,
        });
        curStart = wEnd;
      }
    }
  }

  // Deduplicate and ensure monotonic tokens
  const cleanTokens: FlattenedToken[] = [];
  for (let i = 0; i < tokens.length; i++) {
    const tok = tokens[i];
    if (cleanTokens.length > 0) {
      const prev = cleanTokens[cleanTokens.length - 1];
      // Skip duplicate tokens resulting from chunk overlap strides
      if (prev.clean === tok.clean && Math.abs(prev.start - tok.start) < 0.2) {
        continue;
      }
      if (tok.start < prev.start) {
        tok.start = prev.start;
      }
      if (tok.end <= tok.start) {
        tok.end = tok.start + 0.1;
      }
    }
    cleanTokens.push(tok);
  }

  if (cleanTokens.length === 0) {
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

  // 2. Parse script words and record their line index
  const scriptWords: ScriptWord[] = [];
  for (let lineIdx = 0; lineIdx < scriptLines.length; lineIdx++) {
    const line = scriptLines[lineIdx];
    const words = line.split(/\s+/).filter((w) => w.length > 0);
    for (let wordIdx = 0; wordIdx < words.length; wordIdx++) {
      const w = words[wordIdx];
      scriptWords.push({
        rawText: w,
        clean: cleanText(normalizeNumbersAndUnits(w)),
        lineIndex: lineIdx,
        wordIndex: wordIdx,
      });
    }
  }

  const U = scriptWords.length;
  const V = cleanTokens.length;

  if (U === 0) return [];

  // 3. Dynamic Programming Matrix Alignment (Needleman-Wunsch with banded diagonal)
  const NEG_INF = -1e9;
  const band = Math.max(50, Math.round(V * 0.45));

  const dp: Float32Array[] = Array.from({ length: U + 1 }, () => new Float32Array(V + 1).fill(NEG_INF));
  const ptr: Uint8Array[] = Array.from({ length: U + 1 }, () => new Uint8Array(V + 1)); // 1: match, 2: skip_script, 3: skip_token

  dp[0][0] = 0;

  for (let v = 1; v <= V; v++) {
    dp[0][v] = dp[0][v - 1] - 0.05; // tiny penalty for leading noise
    ptr[0][v] = 3;
  }

  for (let u = 1; u <= U; u++) {
    const sWord = scriptWords[u - 1];
    const centerV = Math.round((u / U) * V);
    const minV = Math.max(1, centerV - band);
    const maxV = Math.min(V, centerV + band);

    dp[u][0] = dp[u - 1][0] - 0.3;
    ptr[u][0] = 2;

    for (let v = minV; v <= maxV; v++) {
      const tWord = cleanTokens[v - 1];
      const sim = computeWordSimilarity(sWord.clean, tWord.clean);

      // Match transition
      let matchScore = NEG_INF;
      if (dp[u - 1][v - 1] > NEG_INF / 2) {
        if (sim >= 0.35) {
          matchScore = dp[u - 1][v - 1] + (sim * 2.5) - 0.1;
        } else {
          matchScore = dp[u - 1][v - 1] - 0.6;
        }
      }

      // Skip script word (deletion in audio)
      const skipScriptScore = dp[u - 1][v] > NEG_INF / 2 ? dp[u - 1][v] - 0.35 : NEG_INF;

      // Skip token (insertion in audio / noise)
      const skipTokenScore = dp[u][v - 1] > NEG_INF / 2 ? dp[u][v - 1] - 0.15 : NEG_INF;

      let best = matchScore;
      let move = 1;

      if (skipScriptScore > best) {
        best = skipScriptScore;
        move = 2;
      }
      if (skipTokenScore > best) {
        best = skipTokenScore;
        move = 3;
      }

      dp[u][v] = best;
      ptr[u][v] = move;
    }
  }

  // 4. Backtrack to extract matched token intervals
  let currU = U;
  let currV = V;

  let bestEndV = V;
  let maxEndScore = dp[U][V];
  for (let v = Math.max(1, V - 20); v <= V; v++) {
    if (dp[U][v] > maxEndScore) {
      maxEndScore = dp[U][v];
      bestEndV = v;
    }
  }
  currV = bestEndV;

  const scriptMatches: Array<{ tokenIndex: number; sim: number; start: number; end: number } | null> = new Array(U).fill(null);

  while (currU > 0 && currV > 0) {
    const move = ptr[currU][currV];
    if (move === 1) {
      const sim = computeWordSimilarity(scriptWords[currU - 1].clean, cleanTokens[currV - 1].clean);
      if (sim >= 0.3) {
        scriptMatches[currU - 1] = {
          tokenIndex: currV - 1,
          sim,
          start: cleanTokens[currV - 1].start,
          end: cleanTokens[currV - 1].end,
        };
      }
      currU--;
      currV--;
    } else if (move === 2) {
      currU--;
    } else {
      currV--;
    }
  }

  // Group matched tokens by script line
  const lineTokenMatches: Array<Array<{ start: number; end: number; sim: number }>> = Array.from(
    { length: scriptLines.length },
    () => []
  );

  for (let u = 0; u < U; u++) {
    const match = scriptMatches[u];
    if (match) {
      const lineIdx = scriptWords[u].lineIndex;
      lineTokenMatches[lineIdx].push(match);
    }
  }

  // 5. Compute raw timestamps per line
  const rawTimestamps: Array<{ start: number; end: number; matched: boolean }> = [];
  for (let i = 0; i < scriptLines.length; i++) {
    const matches = lineTokenMatches[i];
    if (matches.length > 0) {
      const st = matches[0].start;
      const et = matches[matches.length - 1].end;
      rawTimestamps.push({ start: st, end: Math.max(st + 0.3, et), matched: true });
    } else {
      rawTimestamps.push({ start: -1, end: -1, matched: false });
    }
  }

  // Interpolate lines without direct matches
  for (let i = 0; i < rawTimestamps.length; i++) {
    if (!rawTimestamps[i].matched) {
      let prevIdx = i - 1;
      while (prevIdx >= 0 && !rawTimestamps[prevIdx].matched) prevIdx--;
      let nextIdx = i + 1;
      while (nextIdx < rawTimestamps.length && !rawTimestamps[nextIdx].matched) nextIdx++;

      const prevEnd = prevIdx >= 0 ? rawTimestamps[prevIdx].end : 0.0;
      const nextStart = nextIdx < rawTimestamps.length ? rawTimestamps[nextIdx].start : totalDuration;

      const unmappedCount = nextIdx - prevIdx;
      const slot = i - prevIdx;
      const availDur = Math.max(0.5, nextStart - prevEnd);
      const st = prevEnd + ((slot - 1) * availDur) / unmappedCount;
      const et = prevEnd + (slot * availDur) / unmappedCount;

      rawTimestamps[i] = { start: st, end: et, matched: false };
    }
  }

  // 6. Post-processing: zero cumulative drift, clean boundaries, preserve natural pauses
  const results: SubtitleItem[] = [];
  for (let i = 0; i < scriptLines.length; i++) {
    let st = Math.max(0, rawTimestamps[i].start);
    let et = Math.max(st + 0.25, rawTimestamps[i].end);

    if (results.length > 0) {
      const prev = results[results.length - 1];
      if (st < prev.end_secs) {
        // Resolve overlap cleanly at midpoint
        const mid = (prev.end_secs + st) / 2;
        prev.end_secs = Math.max(prev.start_secs + 0.2, mid);
        prev.end_formatted = formatSrtTime(prev.end_secs);
        st = Math.max(prev.end_secs, mid);
        et = Math.max(st + 0.25, et);
      } else {
        // Natural gap: extend previous subtitle slightly for reading comfort if gap is short
        const gap = st - prev.end_secs;
        if (gap > 0.05 && gap <= 0.45) {
          prev.end_secs = Math.min(st - 0.03, prev.end_secs + 0.2);
          prev.end_formatted = formatSrtTime(prev.end_secs);
        }
      }
    }

    results.push({
      index: i + 1,
      start_secs: st,
      end_secs: et,
      start_formatted: formatSrtTime(st),
      end_formatted: formatSrtTime(et),
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
  if (!chunks || chunks.length === 0) return [];

  // Flatten into word tokens
  const words: Array<{ text: string; start: number; end: number }> = [];
  for (const c of chunks) {
    const raw = (c.text || '').split(/\s+/).filter((w) => w.length > 0);
    if (raw.length === 0) continue;

    const start = Math.max(0, c.timestamp[0]);
    const end = Math.max(start + 0.05, c.timestamp[1]);
    const dur = end - start;

    if (raw.length === 1) {
      words.push({ text: raw[0], start, end });
    } else {
      const weights = raw.map((w) => Math.max(1, cleanText(w).length));
      const totalW = weights.reduce((a, b) => a + b, 0);
      let curStart = start;
      for (let i = 0; i < raw.length; i++) {
        const wDur = (weights[i] / totalW) * dur;
        const wEnd = i === raw.length - 1 ? end : curStart + wDur;
        words.push({ text: raw[i], start: curStart, end: wEnd });
        curStart = wEnd;
      }
    }
  }

  if (words.length === 0) return [];

  const items: SubtitleItem[] = [];
  let currentWords: string[] = [];
  let currentStart = words[0].start;
  let currentEnd = words[0].end;

  for (let i = 0; i < words.length; i++) {
    const w = words[i];
    const prevW = i > 0 ? words[i - 1] : null;

    // Check pause between words (> 0.45s indicates phrase break)
    const hasLongPause = prevW ? w.start - prevW.end > 0.45 : false;
    const currentLineLen = currentWords.join(' ').length;
    const isPunctuationEnd = prevW && /[.?!…~]$/.test(prevW.text);

    if (currentWords.length > 0 && (hasLongPause || isPunctuationEnd || currentLineLen + w.text.length + 1 > maxChars)) {
      items.push({
        index: items.length + 1,
        start_secs: currentStart,
        end_secs: currentEnd,
        start_formatted: formatSrtTime(currentStart),
        end_formatted: formatSrtTime(currentEnd),
        text: currentWords.join(' '),
      });
      currentWords = [w.text];
      currentStart = w.start;
      currentEnd = w.end;
    } else {
      if (currentWords.length === 0) {
        currentStart = w.start;
      }
      currentWords.push(w.text);
      currentEnd = w.end;
    }
  }

  if (currentWords.length > 0) {
    items.push({
      index: items.length + 1,
      start_secs: currentStart,
      end_secs: currentEnd,
      start_formatted: formatSrtTime(currentStart),
      end_formatted: formatSrtTime(currentEnd),
      text: currentWords.join(' '),
    });
  }

  return items;
}

export function formatSrtTime(secs: number): string {
  const totalMs = Math.max(0, Math.round(secs * 1000));
  const hrs = Math.floor(totalMs / 3600000);
  const mins = Math.floor((totalMs % 3600000) / 60000);
  const sc = Math.floor((totalMs % 60000) / 1000);
  const ms = totalMs % 1000;
  return `${String(hrs).padStart(2, '0')}:${String(mins).padStart(2, '0')}:${String(sc).padStart(2, '0')},${String(ms).padStart(3, '0')}`;
}

export function formatVttTime(secs: number): string {
  const totalMs = Math.max(0, Math.round(secs * 1000));
  const hrs = Math.floor(totalMs / 3600000);
  const mins = Math.floor((totalMs % 3600000) / 60000);
  const sc = Math.floor((totalMs % 60000) / 1000);
  const ms = totalMs % 1000;
  return `${String(hrs).padStart(2, '0')}:${String(mins).padStart(2, '0')}:${String(sc).padStart(2, '0')}.${String(ms).padStart(3, '0')}`;
}
