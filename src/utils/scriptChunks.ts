/**
 * 대본을 TTS 서비스의 1회 입력 한도에 맞춰 조각으로 나눈다.
 *
 * Typecast 편집기는 한 번에 넣을 수 있는 글자 수(문단 최대 길이)가 3,000자로 제한된다.
 * 줄 경계를 최대한 지켜서 나누고, 한 줄이 통째로 한도를 넘으면 그 줄만 글자 수로 강제 분할한다.
 */
/**
 * 편집기에 넣기 전 대본을 정리한다.
 *
 * Typecast 편집기(Slate.js)는 줄 하나를 단락 하나로 만든다. 빈 줄을 그대로 넣으면
 * 소리 없는 빈 단락이 생기고 화자 선택 UI만 늘어나므로 미리 걷어낸다.
 */
export const cleanScript = (text: string): string =>
  text
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .join('\n');

/**
 * 한 줄이 통째로 한도를 넘을 때 글자 경계에서만 잘라 나눈다.
 *
 * 길이는 `String.prototype.length`(UTF-16 코드 유닛)로 재고, 자르는 위치만 글자
 * 경계로 맞춘다 — 이 파일 전체가 같은 기준이다. 코드 포인트 개수로 재면 이모지가
 * 섞인 조각이 편집기 쪽 글자 수 계산(코드 유닛)보다 짧게 보여 한도를 넘길 수 있다.
 *
 * 경계 케이스: 한 글자가 `limit` 보다 크면(limit=1 에 이모지) 그 글자만 담은
 * 조각이 한도를 넘는다. 글자를 쪼개는 것보다 낫기 때문에 의도한 동작이다.
 */
const splitLineByLimit = (line: string, limit: number): string[] => {
  const pieces: string[] = [];
  let start = 0;
  let i = 0;

  while (i < line.length) {
    // 서로게이트 쌍(이모지 등 비 BMP 문자)은 코드 유닛 2개를 함께 넘긴다.
    // 1씩 전진하면 조각 경계가 쌍 사이에 떨어져 반쪽 글자(U+FFFD)가 만들어진다.
    const code = line.charCodeAt(i);
    let size = 1;
    if (code >= 0xd800 && code <= 0xdbff && i + 1 < line.length) {
      const next = line.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) size = 2;
    }

    if (i > start && i + size - start > limit) {
      pieces.push(line.slice(start, i));
      start = i;
    }
    i += size;
  }

  if (start < line.length) pieces.push(line.slice(start));
  return pieces;
};

/**
 * 정리한 대본을 `limit` 코드 유닛 이하의 조각으로 나눈다.
 *
 * 경계 케이스(프론트엔드 테스트 러너가 없어 주석으로 남긴다):
 * - `splitIntoChunks('abc\nde', 6)` → `['abc\nde']` 조각 하나.
 *   조각에 실제로 들어가는 문자열(줄 사이 개행 1개 포함)의 길이로 판단한다.
 *   예전에는 `(current + line).length + 1` 로 재서, 마지막에 `trimEnd()` 로 사라질
 *   개행까지 한 번 더 세는 바람에 한도에 딱 맞는 대본이 쓸데없이 쪼개졌다.
 * - `splitIntoChunks('abc\nde', 5)` → `['abc', 'de']`.
 * - `splitIntoChunks('ab😀cd', 3)` → `['ab', '😀c', 'd']`.
 *   `😀` 는 코드 유닛 2개지만 반으로 잘리지 않는다.
 * - `limit <= 0` 이면 나누지 않고 정리된 전문을 그대로 돌려준다.
 * - 빈 대본(공백·빈 줄뿐)이면 빈 배열.
 */
export const splitIntoChunks = (rawText: string, limit: number): string[] => {
  const text = cleanScript(rawText);
  if (!text) return [];
  if (limit <= 0 || text.length <= limit) {
    return [text];
  }

  const chunks: string[] = [];
  // `current` 는 항상 조각에 들어갈 최종 형태로 유지한다(뒤에 붙는 개행 없음).
  // 예전에는 줄마다 `\n` 을 붙여 두고 마지막에 `trimEnd()` 로 떼어냈는데,
  // 그 때문에 용량 계산과 실제 조각 길이가 어긋났다.
  let current = '';

  for (const line of text.split('\n')) {
    if (line.length > limit) {
      if (current) {
        chunks.push(current);
        current = '';
      }
      for (const piece of splitLineByLimit(line, limit)) {
        chunks.push(piece);
      }
      continue;
    }

    const candidate = current ? `${current}\n${line}` : line;
    if (candidate.length > limit && current) {
      chunks.push(current);
      current = line;
      continue;
    }
    current = candidate;
  }

  if (current) chunks.push(current);
  return chunks;
};
