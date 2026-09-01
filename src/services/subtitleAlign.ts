/**
 * 대본 분할과 강제 정렬(Forced Alignment). 순수 계산만 있고 I/O 도 모델도 없다.
 *
 * 메인 스레드와 Whisper 워커가 함께 쓴다 — 정렬 DP 는 대본 단어 수 × 밴드 폭만큼 도는데
 * 1시간 녹음(대본 9천 단어 × 토큰 1만)이면 수천만 셀이라 메인 스레드에서 돌리면 그동안
 * 화면이 굳는다. 그래서 실제 정렬은 워커가 수행하고, 이 모듈은 양쪽이 import 한다.
 */
import type { SubtitleItem, SubtitleSplitMode } from '../types';
import type { WhisperChunk } from './whisperWorkerProtocol';

/** LRC 타임스탬프 `[mm:ss.xx]` */
const LRC_TIMESTAMP = /\[\d{1,2}:\d{2}(?:\.\d+)?\]/g;
/** SRT/VTT 타임코드 줄 `00:00:01,000 --> 00:00:03,000` */
const CUE_TIME_LINE = /^\d{2}:\d{2}:\d{2}[,.]\d{3}\s*-->\s*\d{2}:\d{2}:\d{2}[,.]\d{3}/;
/** SRT 블록 머리의 숫자 인덱스 줄 */
const CUE_INDEX_LINE = /^\d{1,6}$/;
/** VTT 큐 설정(`align:start position:50%`)만 남은 줄 */
const CUE_SETTINGS_ONLY = /^[a-zA-Z]+:\S+(?:\s+[a-zA-Z]+:\S+)*$/;

/**
 * 대본 원문에서 자막 본문으로 쓸 줄만 뽑는다.
 *
 * 이미 만든 SRT 를 그대로 붙여 넣는 경우가 많아 블록 단위로 파싱한다. 예전 코드는 타임코드만
 * 지웠기 때문에 블록 머리의 숫자 인덱스("1", "2"...)가 자막 본문으로 남아, 자막 줄 절반이
 * 숫자만 있는 줄이 됐다.
 */
function extractContentLines(rawText: string): string[] {
  const rawLines = rawText.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
  const contentLines: string[] = [];

  const nextNonEmpty = (from: number): number => {
    for (let j = from; j < rawLines.length; j++) {
      if (rawLines[j].trim().length > 0) return j;
    }
    return -1;
  };

  for (let i = 0; i < rawLines.length; i++) {
    const line = rawLines[i].trim();
    if (line.length === 0) continue;
    if (/^WEBVTT/.test(line)) continue; // VTT 헤더

    // 숫자 인덱스 + 타임코드 = SRT 블록 머리. 인덱스와 타임코드 줄을 함께 소비한다.
    if (CUE_INDEX_LINE.test(line)) {
      const next = nextNonEmpty(i + 1);
      if (next >= 0 && CUE_TIME_LINE.test(rawLines[next].trim())) {
        i = next;
        continue;
      }
    }

    if (CUE_TIME_LINE.test(line)) {
      // 타임코드와 같은 줄에 본문이 붙어 있는 변종도 있다. 큐 설정만 남으면 버린다.
      const rest = line.replace(CUE_TIME_LINE, '').trim();
      if (rest.length > 0 && !CUE_SETTINGS_ONLY.test(rest)) {
        contentLines.push(rest);
      }
      continue;
    }

    const withoutLrc = line.replace(LRC_TIMESTAMP, '').trim();
    if (withoutLrc.length > 0) contentLines.push(withoutLrc);
  }

  return contentLines;
}

/**
 * Robust sentence and chunk splitter for subtitles
 *
 * 길이 기준은 코드포인트다. Rust 쪽(`chars().count()`)과 같은 기준이어야 VAD 엔진과 Whisper
 * 엔진이 같은 대본에서 같은 줄을 만든다.
 */
export function splitScriptIntoLines(
  rawText: string,
  mode: SubtitleSplitMode = 'sentence',
  maxChars: number = 28,
  splitOnComma: boolean = false
): string[] {
  const initialLines = extractContentLines(rawText);

  // Line mode: 정리된 본문 줄을 그대로 쓴다
  if (mode === 'line') {
    return initialLines;
  }

  const limit = Math.max(1, Math.floor(maxChars));
  const sentences: string[] = [];

  for (const line of initialLines) {
    const parts: string[] = [];
    let cur = '';

    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      cur += ch;

      const isComma = ch === ',' || ch === '、' || ch === '，';
      const isEndingPunct = ch === '.' || ch === '?' || ch === '!' || ch === '…' || ch === '。' || ch === '~'
        || (splitOnComma && mode === 'sentence' && isComma);
      if (isEndingPunct) {
        // Check if next char is a digit, i.e. a decimal point (3.14) or thousands separator (1,000)
        const isDecimal = (ch === '.' || isComma) && i > 0 && /\d/.test(line[i - 1]) && i + 1 < line.length && /\d/.test(line[i + 1]);
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
        if (codePointLength(p) <= limit) {
          sentences.push(p);
        } else {
          const subChunks = splitByLength(p, limit);
          sentences.push(...subChunks);
        }
      }
    }
  }

  return sentences.filter((s) => s.length > 0);
}

/**
 * 코드포인트 개수. Rust `chars().count()` 와 같은 값이어야 한다.
 *
 * `String.length`(UTF-16 코드유닛)로 세면 이모지·희귀 한자가 두 글자로 잡혀 자막 줄 길이가
 * 백엔드와 어긋난다. 반복자 대신 인덱스로 훑어 코드포인트마다 문자열을 새로 만들지 않는다.
 */
function codePointLength(text: string): number {
  let count = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
      const low = text.charCodeAt(i + 1);
      if (low >= 0xdc00 && low <= 0xdfff) i++; // 서로게이트 페어는 코드포인트 1개
    }
    count++;
  }
  return count;
}

/** 코드포인트 경계에서만 자른다(서로게이트 페어를 반토막 내지 않는다). */
function splitByCodePoints(text: string, limit: number): string[] {
  const pieces: string[] = [];
  let start = 0;
  let count = 0;

  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
      const low = text.charCodeAt(i + 1);
      if (low >= 0xdc00 && low <= 0xdfff) i++;
    }
    count++;
    if (count >= limit) {
      pieces.push(text.slice(start, i + 1));
      start = i + 1;
      count = 0;
    }
  }

  if (start < text.length) {
    pieces.push(text.slice(start));
  }
  return pieces;
}

/**
 * 긴 줄을 maxChars(코드포인트) 이하로 쪼갠다.
 *
 * 예전 코드는 공백에서만 쪼개고 단어가 하나면 원문을 그대로 돌려줬다. 그래서 공백 없이 이어 쓴
 * 한국어/중국어 장문은 한계를 완전히 무시한 한 줄로 남아 화면을 덮었다. 토큰 자체가 한계를
 * 넘으면 코드포인트 경계로 자른다.
 */
function splitByLength(text: string, maxChars: number): string[] {
  const limit = Math.max(1, Math.floor(maxChars));
  if (codePointLength(text) <= limit) return [text];

  const chunks: string[] = [];
  let current = '';
  let currentLen = 0;

  const flush = () => {
    const trimmed = current.trim();
    if (trimmed.length > 0) chunks.push(trimmed);
    current = '';
    currentLen = 0;
  };

  for (const word of text.split(/\s+/)) {
    if (word.length === 0) continue;
    const wordLen = codePointLength(word);

    if (wordLen > limit) {
      flush();
      const pieces = splitByCodePoints(word, limit);
      for (let i = 0; i < pieces.length - 1; i++) {
        chunks.push(pieces[i]);
      }
      // 마지막 조각은 뒤따르는 단어와 합칠 수 있으므로 현재 줄로 넘긴다.
      current = pieces[pieces.length - 1];
      currentLen = codePointLength(current);
      continue;
    }

    if (currentLen > 0 && currentLen + 1 + wordLen > limit) {
      flush();
    }

    if (currentLen > 0) {
      current += ' ' + word;
      currentLen += 1 + wordLen;
    } else {
      current = word;
      currentLen = wordLen;
    }
  }

  flush();
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

  // 3. Dynamic Programming Alignment (Needleman-Wunsch with banded diagonal)
  const NEG_INF = -1e9;
  const band = Math.max(50, Math.round(V * 0.45));

  // dp 는 (u-1) 행과 u 행만 읽으므로 두 줄만 들고 돈다. 전체 행렬을 잡으면 1시간 녹음
  // (대본 9천 단어 × 토큰 1만)에서 점수 행렬만 360MB 를 태운다.
  let prevRow = new Float32Array(V + 1);
  let curRow = new Float32Array(V + 1);

  // 백포인터는 역추적에 전부 필요하지만 밴드 밖은 애초에 기록되지 않는다. 그래서 밴드 폭만 잡고
  // 셀당 2비트(0: 미기입, 1: 매치, 2: 대본 건너뜀, 3: 토큰 건너뜀)로 눌러 담는다.
  const rowWidth = Math.min(V, 2 * band + 1);
  const rowMin = new Int32Array(U + 1);
  const moves = new Uint8Array(Math.ceil((U * rowWidth) / 4));

  // 밴드 밖 조회는 0(미기입)이다. 예전 전체 행렬에서 손대지 않은 칸을 읽던 것과 결과가 같다.
  const readMove = (u: number, v: number): number => {
    const offset = v - rowMin[u];
    if (offset < 0 || offset >= rowWidth) return 0;
    const cell = (u - 1) * rowWidth + offset;
    return (moves[cell >> 2] >> ((cell & 3) << 1)) & 3;
  };

  prevRow.fill(NEG_INF);
  prevRow[0] = 0;
  for (let v = 1; v <= V; v++) {
    prevRow[v] = prevRow[v - 1] - 0.05; // tiny penalty for leading noise
  }

  for (let u = 1; u <= U; u++) {
    const sWord = scriptWords[u - 1];
    const centerV = Math.round((u / U) * V);
    const minV = Math.max(1, centerV - band);
    const maxV = Math.min(V, centerV + band);
    rowMin[u] = minV;

    // 행을 재사용하므로 이전 사용 흔적을 반드시 지운다. 밴드 밖은 NEG_INF 여야 한다.
    curRow.fill(NEG_INF);
    curRow[0] = prevRow[0] - 0.3;

    const rowBase = (u - 1) * rowWidth;

    for (let v = minV; v <= maxV; v++) {
      const tWord = cleanTokens[v - 1];
      const sim = computeWordSimilarity(sWord.clean, tWord.clean);

      // Match transition
      let matchScore = NEG_INF;
      const diag = prevRow[v - 1];
      if (diag > NEG_INF / 2) {
        matchScore = sim >= 0.35 ? diag + sim * 2.5 - 0.1 : diag - 0.6;
      }

      // Skip script word (deletion in audio)
      const up = prevRow[v];
      const skipScriptScore = up > NEG_INF / 2 ? up - 0.35 : NEG_INF;

      // Skip token (insertion in audio / noise)
      const left = curRow[v - 1];
      const skipTokenScore = left > NEG_INF / 2 ? left - 0.15 : NEG_INF;

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

      curRow[v] = best;
      const cell = rowBase + (v - minV);
      moves[cell >> 2] |= move << ((cell & 3) << 1); // 셀마다 한 번만 쓰므로 OR 로 충분하다
    }

    const swap = prevRow;
    prevRow = curRow;
    curRow = swap;
  }

  // swap 때문에 마지막 행(u = U)은 prevRow 에 들어 있다.
  const lastRow = prevRow;

  // 4. Backtrack to extract matched token intervals
  let currU = U;

  let bestEndV = V;
  let maxEndScore = lastRow[V];
  for (let v = Math.max(1, V - 20); v <= V; v++) {
    if (lastRow[v] > maxEndScore) {
      maxEndScore = lastRow[v];
      bestEndV = v;
    }
  }
  let currV = bestEndV;

  const scriptMatches: Array<{ tokenIndex: number; sim: number; start: number; end: number } | null> = new Array(U).fill(null);

  while (currU > 0 && currV > 0) {
    const move = readMove(currU, currV);
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
  const limit = Math.max(1, Math.floor(maxChars));
  let currentWords: string[] = [];
  // 줄 길이는 코드포인트 기준(백엔드 `chars().count()` 와 동일)으로 누적한다. 매 단어마다
  // join 해서 길이를 재면 긴 자막에서 O(n^2) 로 늘어나기도 한다.
  let currentLen = 0;
  let currentStart = words[0].start;
  let currentEnd = words[0].end;

  for (let i = 0; i < words.length; i++) {
    const w = words[i];
    const prevW = i > 0 ? words[i - 1] : null;
    const wordLen = codePointLength(w.text);

    // Check pause between words (> 0.45s indicates phrase break)
    const hasLongPause = prevW ? w.start - prevW.end > 0.45 : false;
    const isPunctuationEnd = prevW ? /[.?!…~]$/.test(prevW.text) : false;

    if (currentWords.length > 0 && (hasLongPause || isPunctuationEnd || currentLen + 1 + wordLen > limit)) {
      items.push({
        index: items.length + 1,
        start_secs: currentStart,
        end_secs: currentEnd,
        start_formatted: formatSrtTime(currentStart),
        end_formatted: formatSrtTime(currentEnd),
        text: currentWords.join(' '),
      });
      currentWords = [w.text];
      currentLen = wordLen;
      currentStart = w.start;
      currentEnd = w.end;
    } else {
      if (currentWords.length === 0) {
        currentStart = w.start;
        currentLen = wordLen;
      } else {
        currentLen += 1 + wordLen;
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
