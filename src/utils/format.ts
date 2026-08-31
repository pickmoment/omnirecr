/**
 * 화면 여러 곳에서 쓰이던 표시용 포맷 함수 모음.
 * 이전에는 컴포넌트마다 같은 함수를 따로 정의하고 있었다.
 */

/** 초를 `MM:SS` 로, 1시간을 넘으면 `HH:MM:SS` 로 표시한다. */
export const formatTimer = (seconds: number) => {
  const total = Math.max(0, Math.floor(seconds));
  const hrs = Math.floor(total / 3600);
  const mins = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const mm = String(mins).padStart(2, '0');
  const ss = String(secs).padStart(2, '0');
  return hrs > 0 ? `${String(hrs).padStart(2, '0')}:${mm}:${ss}` : `${mm}:${ss}`;
};

/** 바이트 수를 사람이 읽는 단위로. MB 이상은 소수 첫째 자리까지 보여 준다. */
export const formatFileSize = (bytes: number, fractionDigits?: number) => {
  if (bytes <= 0) return '0 B';
  const k = 1024;
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), units.length - 1);
  const digits = fractionDigits ?? (index > 1 ? 1 : 0);
  return `${(bytes / Math.pow(k, index)).toFixed(digits)} ${units[index]}`;
};

/**
 * 초를 `N분 SS초` 형태로. 예상 낭독 시간·오디오 길이 표시에 쓴다.
 * 1시간을 넘으면 `H시간 MM분 SS초` 로 늘린다(`125분` 같은 표시를 피한다).
 *
 * 반올림은 반드시 **총 초에 한 번만** 적용한다. 분을 먼저 내림한 뒤 `secs % 60` 을
 * 따로 반올림하면 59.6초가 `0분 60초`, 119.7초가 `1분 60초` 로 표시된다.
 */
export const formatDuration = (secs: number) => {
  if (!Number.isFinite(secs) || secs <= 0) return '0초';
  const total = Math.round(secs);
  if (total <= 0) return '0초';
  const hrs = Math.floor(total / 3600);
  const mins = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  const ss = String(rest).padStart(2, '0');
  if (hrs > 0) return `${hrs}시간 ${String(mins).padStart(2, '0')}분 ${ss}초`;
  if (mins <= 0) return `${rest}초`;
  return `${mins}분 ${ss}초`;
};
