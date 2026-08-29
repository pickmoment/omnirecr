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

export const splitIntoChunks = (rawText: string, limit: number): string[] => {
  const text = cleanScript(rawText);
  if (!text) return [];
  if (limit <= 0 || text.length <= limit) {
    return [text];
  }

  const chunks: string[] = [];
  let current = '';

  for (const line of text.split('\n')) {
    if (line.length > limit) {
      if (current.trim()) {
        chunks.push(current.trimEnd());
        current = '';
      }
      for (let i = 0; i < line.length; i += limit) {
        chunks.push(line.slice(i, i + limit));
      }
      continue;
    }

    if ((current + line).length + 1 > limit && current.trim()) {
      chunks.push(current.trimEnd());
      current = '';
    }
    current += `${line}\n`;
  }

  if (current.trim()) chunks.push(current.trimEnd());
  return chunks;
};
