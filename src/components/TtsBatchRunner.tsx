import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  AlertCircle,
  Check,
  ChevronDown,
  CircleDashed,
  FolderOpen,
  ListChecks,
  Loader,
  Mic,
  Play,
  Search,
  Settings as SettingsIcon,
  Square,
  X,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { TypecastDiagnosticsLog, TypecastSessionCard } from './TypecastSessionCard';
import type {
  AudioVUMeterPayload,
  BatchItemState,
  BatchItemStatus,
  RecordingStatus,
  ScriptItem,
  ScriptRecordingTarget,
  Settings,
  TypecastBrowserState,
  TypecastStepPayload,
} from '../types';
import type { StartTtsRecordOptions } from './TtsRecorder';
import { formatDuration } from '../utils/format';

interface TtsBatchRunnerProps {
  settings: Settings;
  scripts: ScriptItem[];
  selectedIds: string[];
  onToggleSelect: (id: string) => void;
  onSelectAll: (ids: string[]) => void;
  recordingStatus: RecordingStatus;
  onUpdateSettings: (partial: Partial<Settings>) => Promise<void>;
  onStartRecord: (options: StartTtsRecordOptions) => Promise<string | null>;
  onStopRecord: (options?: { silent?: boolean }) => Promise<void>;
  onRefreshScripts: () => Promise<void>;
  onOpenExplorer: (path: string) => Promise<void>;
  onGoToLibrary: () => void;
  onOpenSettings: () => void;
}

const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

/** 페이지 자동화 단계 보고 하나. 일련번호로 "어느 시점 이후인지"를 판정한다. */
interface StepRecord {
  seq: number;
  name: string;
  detail: string;
}
type StepWaiter = (record: StepRecord) => void;

/** 단계 대기 결과. `aborted` 는 사용자가 중단을 눌러 끝난 경우(실패가 아니다). */
interface StepResult {
  ok: boolean;
  detail: string;
  aborted?: boolean;
}

/** 판정 루프가 읽고 초기화하는 시스템 오디오 피크와 마지막 수신 시각. */
interface VuSample {
  peakDb: number;
  at: number;
}

/** `linear_to_db` 가 클램프하는 하한(dsp.rs)과 같은 값. */
const SILENCE_FLOOR_DB = -60;
/** 보관할 단계 보고 개수. 지나간 보고를 되짚어 볼 수 있을 만큼만 남긴다. */
const STEP_LOG_LIMIT = 300;
/** `audio_vu_meter` 가 이만큼 끊기면 녹음 세션이 사라진 것으로 본다(틱커 주기 50ms). */
const VU_STALL_MS = 3000;
/** 녹음 직후에는 첫 VU 이벤트가 늦을 수 있어 위 판정에 유예를 준다. */
const VU_GRACE_MS = 5000;
/** 첫 재생 클릭 후 이 시간 동안 소리가 없으면 정지 후 한 번만 다시 재생한다. */
const PLAYBACK_RECOVERY_MS = 5000;
/** Typecast 재생 상태를 정지로 되돌린 뒤 다시 누르기 전 안정화 시간. */
const PLAYBACK_RESTART_GAP_MS = 700;

/**
 * 대본을 붙여넣은 뒤 재생을 요청하기까지의 안정화 시간.
 *
 * `step:prepared` 는 편집기 DOM 에 글자가 들어간 것만 확인한다. Typecast(React + Slate)는
 * 그 뒤에도 내부 상태 반영 · 재생 버튼 활성화 · 합성 준비를 이어서 하므로 2초 동안
 * 안정화한 뒤 재생 버튼을 정확히 한 번 누른다. 녹음을 **시작하기 전에** 기다려 이 시간이
 * 결과 파일 앞부분의 무음으로 들어가지 않게 한다.
 */
const EDITOR_SETTLE_MS = 2000;

/** 아직 결말이 나지 않은 상태들. 배치가 중간에 끝나면 '건너뜀' 으로 마무리한다. */
const UNFINISHED_STATUSES: BatchItemStatus[] = [
  'pending',
  'preparing',
  'recording',
  'speaking',
  'saving',
];

const STATUS_LABEL: Record<BatchItemStatus, string> = {
  pending: '대기',
  preparing: '대본 입력 중',
  recording: '재생 대기',
  speaking: '녹음 중',
  saving: '저장 중',
  done: '완료',
  failed: '실패',
  skipped: '건너뜀',
};

const STATUS_STYLE: Record<BatchItemStatus, string> = {
  pending: 'bg-slate-800 text-slate-400 border-slate-700',
  preparing: 'bg-purple-950/60 text-purple-300 border-purple-800/50',
  recording: 'bg-amber-950/60 text-amber-300 border-amber-800/50',
  speaking: 'bg-red-950/60 text-red-300 border-red-800/50',
  saving: 'bg-blue-950/60 text-blue-300 border-blue-800/50',
  done: 'bg-emerald-950/60 text-emerald-300 border-emerald-800/50',
  failed: 'bg-red-950/60 text-red-300 border-red-800/50',
  skipped: 'bg-slate-800 text-slate-500 border-slate-700',
};

export const TtsBatchRunner: React.FC<TtsBatchRunnerProps> = ({
  settings,
  scripts,
  selectedIds,
  onToggleSelect,
  onSelectAll,
  recordingStatus,
  onUpdateSettings,
  onStartRecord,
  onStopRecord,
  onRefreshScripts,
  onOpenExplorer,
  onGoToLibrary,
  onOpenSettings,
}) => {
  const [queue, setQueue] = useState<BatchItemState[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [phaseMessage, setPhaseMessage] = useState<string>('');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [chromeTestMsg, setChromeTestMsg] = useState<string | null>(null);

  const abortRef = useRef(false);
  /** 이 러너가 시작한 녹음이 아직 돌고 있는지. 중복 정지·정지 누락을 막는다. */
  const recordingActiveRef = useRef(false);
  /**
   * 진행 중인 정지 요청. 중단 버튼과 `runBatch` 의 finally 가 동시에 불러도 정지 커맨드가
   * 두 번 나가지 않도록 하나의 프라미스로 직렬화한다.
   */
  const stopInFlightRef = useRef<Promise<StepResult> | null>(null);

  // ── Typecast 단계 보고 · 시스템 오디오 레벨 상시 구독 ─────────
  //
  // 예전에는 `invoke()` 를 **보낸 뒤에** `listen()` 을 걸었는데, `listen()` 등록 자체가
  // 비동기 왕복이라 그사이에 도착한 보고를 놓쳤다. 특히 페이지 스크립트가 동기적으로 내는
  // 실패 보고(`step:prepare-failed:입력 중 오류 …`)는 항상 유실돼, 진짜 원인 대신 10초 뒤
  // "페이지 응답 시간 초과"만 보였다. 구독을 상시로 두고 일련번호(`markSteps`)로 "이 시점
  // 이후"를 판정해 경합 자체를 없앤다. 다만 **첫 등록** 자체도 왕복이라, 마운트 직후 곧바로
  // 시작하면 같은 유실이 그대로 재현된다 — 배치는 `ensureListenersReady()` 로 등록 완료를
  // 기다린 뒤에야 첫 `typecast_*` 를 보낸다.
  const stepLogRef = useRef<StepRecord[]>([]);
  const stepSeqRef = useRef(0);
  const stepWaitersRef = useRef<Set<StepWaiter>>(new Set());
  const vuRef = useRef<VuSample>({ peakDb: SILENCE_FLOOR_DB, at: 0 });
  const cleanupRef = useRef<UnlistenFn[]>([]);
  /**
   * 상시 구독이 실제로 등록될 때까지의 프라미스. 첫 Typecast 조작 전에 이걸 기다려야
   * 등록 왕복 중에 도착한 보고를 놓치지 않는다(아래 `ensureListenersReady`).
   */
  const listenersReadyRef = useRef<Promise<void> | null>(null);

  useEffect(() => {
    let disposed = false;
    const track = (pending: Promise<UnlistenFn>): Promise<void> =>
      pending.then((unlisten) => {
        if (disposed) unlisten();
        else cleanupRef.current.push(unlisten);
      });

    const ready = Promise.all([
      track(
        listen<TypecastStepPayload>('typecast_step', (event) => {
          stepSeqRef.current += 1;
          const record: StepRecord = {
            seq: stepSeqRef.current,
            name: event.payload.name,
            detail: event.payload.detail,
          };
          const log = stepLogRef.current;
          log.push(record);
          if (log.length > STEP_LOG_LIMIT) log.splice(0, log.length - STEP_LOG_LIMIT);
          stepWaitersRef.current.forEach((waiter) => waiter(record));
        }),
      ),
      track(
        listen<AudioVUMeterPayload>('audio_vu_meter', (event) => {
          const level = event.payload.sys_level_db;
          // 피크는 판정 루프가 읽고 초기화한다. 이벤트(50ms)가 루프 주기(100ms)보다 촘촘해,
          // 최신값만 보면 짧은 발화 피크를 놓칠 수 있다.
          vuRef.current = {
            peakDb: Math.max(vuRef.current.peakDb, level),
            at: Date.now(),
          };
        }),
      ),
    ]).then(() => undefined);

    listenersReadyRef.current = ready;
    // 아무도 기다리지 않는 동안 등록이 실패해도 unhandled rejection 으로 터지지 않게 한다.
    // 실제 실패 판단은 배치 시작 전에 `ensureListenersReady()` 가 한다.
    void ready.catch(() => {});

    return () => {
      disposed = true;
      listenersReadyRef.current = null;
      cleanupRef.current.forEach((unlisten) => unlisten());
      cleanupRef.current = [];
    };
  }, []);

  /**
   * 상시 구독(`typecast_step` · `audio_vu_meter`)이 실제로 등록될 때까지 기다린다.
   * 실패 사유가 있으면 그 메시지를, 준비됐으면 null 을 돌려준다.
   *
   * `listen()` 등록은 백엔드 왕복이다. 그게 끝나기 전에 Typecast 를 건드리면 페이지
   * 스크립트가 **동기적으로** 내는 실패 보고(`step:prepare-failed:…`)를 놓쳐, 진짜 원인
   * 대신 10~12초 뒤 허위 타임아웃("페이지 응답 시간 초과")만 남는다. 상시 구독 +
   * `markSteps()` 기준점 설계만으로는 이 첫 등록 경합이 안 막혀서, 첫 `typecast_*`
   * invoke 전에 반드시 이걸 통과시킨다. `audio_vu_meter` 도 같은 프라미스에 묶여 있어,
   * 첫 대본에서 VU 구독이 늦게 붙어 "녹음 사망" 판정이 눈감는 문제도 함께 막는다.
   */
  const ensureListenersReady = async (): Promise<string | null> => {
    const ready = listenersReadyRef.current;
    if (!ready) return '이벤트 구독이 아직 준비되지 않았습니다. 잠시 뒤 다시 시도하세요.';
    try {
      await ready;
      return null;
    } catch (err) {
      return `이벤트 구독에 실패해 자동 처리를 시작할 수 없습니다: ${err}`;
    }
  };

  /** 이 시점 이후의 보고만 기다리도록 기준점을 찍는다. 반드시 `invoke` 앞에서 부를 것. */
  const markSteps = () => stepSeqRef.current;

  /** 직전 구간의 최대 시스템 오디오 레벨을 읽고 초기화한다. */
  const takeVuPeak = (): VuSample => {
    const vu = vuRef.current;
    vuRef.current = { peakDb: SILENCE_FLOOR_DB, at: vu.at };
    return vu;
  };

  const setItem = useCallback((scriptId: string, patch: Partial<BatchItemState>) => {
    setQueue((prev) =>
      prev.map((item) => (item.scriptId === scriptId ? { ...item, ...patch } : item)),
    );
  }, []);

  /**
   * 페이지 자동화 단계 보고(`typecast_step`)를 기다린다.
   * 성공 이름 중 하나가 오면 ok, 실패 이름이 오면 실패, 시간 내 아무것도 없으면 타임아웃.
   * `since` 이후에 이미 도착해 있던 보고도 함께 확인한다.
   */
  const waitForStep = (
    successNames: string[],
    failureNames: string[],
    timeoutMs: number,
    since: number,
  ): Promise<StepResult> =>
    new Promise((resolve) => {
      const match = (record: StepRecord): StepResult | null => {
        if (successNames.includes(record.name)) return { ok: true, detail: record.detail };
        if (failureNames.includes(record.name)) return { ok: false, detail: record.detail };
        return null;
      };

      // 1) 이미 도착해 있는 보고부터 확인한다(invoke 응답보다 먼저 왔을 수 있다).
      for (const record of stepLogRef.current) {
        if (record.seq <= since) continue;
        const hit = match(record);
        if (hit) {
          resolve(hit);
          return;
        }
      }

      // 2) 아직이면 구독해서 기다린다.
      let settled = false;
      const waiter: StepWaiter = (record) => {
        const hit = match(record);
        if (hit) finish(hit);
      };
      const finish = (result: StepResult) => {
        if (settled) return;
        settled = true;
        window.clearInterval(poller);
        stepWaitersRef.current.delete(waiter);
        resolve(result);
      };

      // 중단 요청에도 바로 반응해야 해서 setTimeout 대신 주기 확인을 쓴다.
      const deadline = Date.now() + timeoutMs;
      const poller = window.setInterval(() => {
        if (abortRef.current) finish({ ok: false, detail: '중단됨', aborted: true });
        else if (Date.now() >= deadline) finish({ ok: false, detail: '페이지 응답 시간 초과' });
      }, 150);

      stepWaitersRef.current.add(waiter);
    });

  /**
   * 시스템 오디오 레벨을 보고 낭독 시작 / 종료를 판정한다.
   * Typecast 가 어떤 방식으로 재생하든(미디어 엘리먼트 · Web Audio) 동작한다.
   *
   * 첫 클릭 뒤에도 소리가 없으면 화면만 재생 상태인 Typecast 를 정지한 뒤 한 번만
   * 다시 재생한다. 실제 낭독이 시작된 뒤에는 재생 버튼에 개입하지 않는다. 단락 전환의
   * 정상적인 무음(`step:media-ended`)과 사이트의 중간 정지(`step:media-pause`)는
   * `segmentGapMs` 동안 재생이 스스로 이어지길 기다린다.
   */
  const waitForSpeechCycle = (
    thresholdDb: number,
    startTimeoutMs: number,
    silenceMs: number,
    hardCapMs: number,
    segmentGapMs: number,
    since: number,
    recoverPlayback: () => Promise<StepResult>,
  ): Promise<StepResult> =>
    new Promise((resolve) => {
      let settled = false;
      let started = false;
      let lastLoudAt = Date.now();
      // media-ended / media-pause 이후 재생이 이어지길 기다리는 유예 시작 시각. null 이면 유예 없음.
      let segmentEndedAt: number | null = null;
      // 이전 대본에서 남은 피크를 버리고 시작한다. 하드캡·중단으로 끝난 직후에는
      // 마지막 피크가 아직 크게 남아 있어, 그대로 두면 새 대본이 재생되기도 전에
      // "낭독 시작"으로 오판하고 무음만 담긴 파일을 완료로 처리한다.
      takeVuPeak();
      const beganAt = Date.now();
      let startDeadlineAt = beganAt + startTimeoutMs;
      const recoveryAfterMs = Math.min(PLAYBACK_RECOVERY_MS, startTimeoutMs / 2);
      let recoveryStarted = false;
      let recoveryInFlight = false;

      const applyStep: StepWaiter = (record) => {
        if (record.name === 'media-ended' || record.name === 'media-pause') {
          segmentEndedAt = Date.now();
        } else if (record.name === 'media-play') {
          segmentEndedAt = null;
        }
      };
      // 재생 요청 이후 이미 도착해 있던 media-* 보고도 반영한다.
      stepLogRef.current.forEach((record) => {
        if (record.seq > since) applyStep(record);
      });

      const finish = (result: StepResult) => {
        if (settled) return;
        settled = true;
        window.clearInterval(ticker);
        stepWaitersRef.current.delete(applyStep);
        resolve(result);
      };

      const ticker = window.setInterval(() => {
        const now = Date.now();
        if (abortRef.current) {
          finish({ ok: false, detail: '중단됨', aborted: true });
          return;
        }

        const vu = takeVuPeak();
        // VU 이벤트가 끊겼다 = 녹음 세션이 사라졌다(FFmpeg 종료 · 오디오 스트림 오류 ·
        // 바깥에서 정지 등). 이걸 못 잡으면 그냥 무음으로 오해해 hardCap 까지(수 분) 헛되게
        // 기다린 뒤 "최대 녹음 시간 도달"로 끝나, 진짜 원인(녹음 사망)을 못 보게 된다.
        // 예전에는 그 하드캡을 성공으로까지 보고해 잘린 파일을 대본에 연결했다.
        if (now - beganAt > VU_GRACE_MS && now - vu.at > VU_STALL_MS) {
          finish({
            ok: false,
            detail: '녹음이 예기치 않게 중단되었습니다 (오디오 캡처 신호 끊김)',
          });
          return;
        }

        // 복구 중 첫 시도의 늦은 소리가 들어와도 새 재생의 시작으로 세지 않는다.
        if (recoveryInFlight) return;

        if (vu.peakDb > thresholdDb) {
          if (!started) {
            started = true;
            setPhaseMessage('낭독 녹음 중...');
          }
          lastLoudAt = now;
          segmentEndedAt = null;
        }

        if (!started) {
          if (!recoveryStarted && now - beganAt >= recoveryAfterMs) {
            recoveryStarted = true;
            recoveryInFlight = true;
            setPhaseMessage('재생 응답이 없어 정지 후 다시 재생하는 중...');
            void recoverPlayback().then((recovery) => {
              recoveryInFlight = false;
              if (settled) return;
              if (!recovery.ok) {
                finish({
                  ...recovery,
                  detail: recovery.aborted
                    ? recovery.detail
                    : `재생 자동 복구 실패: ${recovery.detail}`,
                });
                return;
              }
              takeVuPeak();
              startDeadlineAt = Date.now() + startTimeoutMs;
              setPhaseMessage('재생 재시도 후 소리를 기다리는 중...');
            });
            return;
          }
          if (recoveryInFlight) return;
          if (now > startDeadlineAt) {
            finish({
              ok: false,
              detail: '재생을 자동으로 다시 시도했지만 소리가 감지되지 않았습니다 (시스템 오디오 캡처 · 임계값 확인)',
            });
          }
          return;
        }
        if (now - beganAt > hardCapMs) {
          // 하드캡 도달은 정상 종료가 아니다. 낭독이 아직 안 끝났을 가능성이 크고 파일은
          // 그 지점에서 잘려 있다. 예전에는 `ok: true` 로 보고해 잘린 파일을 완성본처럼
          // 대본에 연결하고 '완료' 로 표시했다(AGENTS.md 가 명시적으로 금지한 동작).
          // 실패로 올려 계속/중단 정책이 판단하게 하고, 파일 경로만 상세에 남긴다.
          finish({
            ok: false,
            detail: `최대 녹음 시간(${Math.round(hardCapMs / 1000)}초) 도달 — 녹음이 잘렸을 수 있어 대본에 연결하지 않았습니다`,
          });
          return;
        }
        // 단락 전환 · 재생 오동작 회복 유예 기간 중에는 무음이어도 재생이 이어지길 기다린다.
        if (segmentEndedAt !== null && now - segmentEndedAt < segmentGapMs) {
          return;
        }
        if (now - lastLoudAt > silenceMs) {
          finish({ ok: true, detail: `${Math.round((now - beganAt) / 1000)}초 녹음` });
        }
      }, 100);

      stepWaitersRef.current.add(applyStep);
    });

  /**
   * 이 러너가 시작한 녹음이 돌고 있으면 멈춘다. 이미 멈춰 있으면 아무것도 하지 않는다.
   *
   * 소유권 해제(`recordingActiveRef = false`)는 **정지가 확인된 뒤에만** 한다. 예전처럼
   * `await onStopRecord()` 앞에서 먼저 내려 버리면, 정지가 거부됐을 때 녹음은 계속 도는데
   * 이후 모든 정리 호출(중단 버튼 · `runBatch` 의 finally)이 no-op 이 되어 녹음을 영구히
   * 놓친다 — "빠져나오는 모든 경로에서 녹음을 정리할 것" 불변식이 깨진다.
   * 정지가 실패하면 백엔드 상태를 직접 조회해 실제로 멈췄는지 확인하고, 여전히 녹음
   * 중이면 소유권을 유지해 다음 호출이 재시도할 수 있게 남긴다.
   * 중복 정지는 in-flight 프라미스 하나로 직렬화해 막는다(같은 플래그를 중단 버튼도 쓴다).
   */
  const stopRecordingIfActive = (): Promise<StepResult> => {
    const inFlight = stopInFlightRef.current;
    if (inFlight) return inFlight;
    if (!recordingActiveRef.current) return Promise.resolve({ ok: true, detail: '' });

    const stopOnce = async (): Promise<StepResult> => {
      try {
        await onStopRecord({ silent: true });
        recordingActiveRef.current = false;
        return { ok: true, detail: '' };
      } catch (err) {
        // 정지가 거부됐다. 녹음이 실제로 남아 있는지 백엔드에 직접 물어본다.
        let stillRecording = true;
        try {
          const status = await invoke<RecordingStatus>('get_recording_status');
          stillRecording = status.status !== 'idle';
        } catch {
          // 상태 조회조차 안 되면 최악을 가정해 소유권을 유지한다(재시도 가능하게).
        }
        recordingActiveRef.current = stillRecording;
        return {
          ok: false,
          detail: stillRecording
            ? `${err} (녹음이 아직 진행 중입니다 — 정지를 다시 시도해야 합니다)`
            : String(err),
        };
      }
    };

    const attempt = stopOnce().finally(() => {
      if (stopInFlightRef.current === attempt) stopInFlightRef.current = null;
    });
    stopInFlightRef.current = attempt;
    return attempt;
  };

  const handleTestChrome = async () => {
    setChromeTestMsg(null);
    try {
      const path = await invoke<string>('check_chrome_status', {
        customChromePath: settings.custom_chrome_path || null,
      });
      setChromeTestMsg(`✅ 감지 성공: ${path}`);
    } catch (err) {
      setChromeTestMsg(`❌ 감지 실패: ${err}`);
    }
  };

  /**
   * 사용자가 Typecast 프로젝트 편집기를 직접 열어 둔 상태인지 확인한다.
   * 프로젝트 목록에서 자동 이동하지 않는다. 사이트 자체의 이동 오류와 엉뚱한 프로젝트 선택을
   * 모두 피하고, 준비되지 않았으면 대본이나 녹음을 건드리기 전에 시작을 막는다.
   */
  const ensureBrowserReady = async (): Promise<boolean> => {
    try {
      const state = await invoke<TypecastBrowserState>('get_typecast_browser_state');
      if (!state.is_open) {
        setErrorMsg(
          'Typecast 열기를 누른 뒤, Chrome에서 작업할 프로젝트를 직접 열어 둔 상태로 다시 시작하세요.',
        );
        return false;
      }
      if (!state.looks_signed_in) {
        setErrorMsg(
          'Typecast 에 로그인되어 있지 않습니다. 위 카드에서 로그인한 뒤 작업할 프로젝트를 직접 열어 주세요.' +
            (state.current_url ? ` (현재 ${state.current_url})` : ''),
        );
        return false;
      }

      setPhaseMessage('열려 있는 Typecast 프로젝트를 확인하는 중...');
      const ready = await invoke<boolean>('typecast_editor_ready');
      if (!ready) {
        setErrorMsg(
          'Typecast Chrome 창에서 작업할 프로젝트를 직접 열고, 대본 편집기와 재생 버튼이 보이는 상태에서 다시 시작하세요.' +
            (state.current_url ? ` (현재 ${state.current_url})` : ''),
        );
        return false;
      }
      return true;
    } catch (err) {
      setErrorMsg(`Typecast 프로젝트 편집기를 확인하지 못했습니다: ${err}`);
      return false;
    }
  };

  /**
   * 저장될 파일 경로를 시작 **전에** 한 번에 확인한다.
   *
   * - 제목이 같은 파일로 저장되는 대본이 있으면 뒤 대본이 앞 대본 결과를 조용히 덮어쓴다
   *   (제목이 곧 파일명이고, 특수문자 치환·길이 제한으로 다른 제목이 같은 이름이 될 수도 있다).
   * - 이미 있는 파일은 모아서 한 번만 묻는다. 실행 도중 `confirm` 을 띄우면 Chrome 창 뒤에서
   *   배치가 멈춘 것처럼 보이고, 대본 수만큼 모달이 반복된다.
   */
  const confirmOutputTargets = async (targets: ScriptItem[]): Promise<boolean> => {
    const resolved = await invoke<ScriptRecordingTarget[]>('resolve_script_recording_targets', {
      settings,
      fileNamePrefixes: targets.map((script) => script.title),
    });

    const titlesByPath = new Map<string, string[]>();
    resolved.forEach((target, index) => {
      const titles = titlesByPath.get(target.path) ?? [];
      titles.push(targets[index].title);
      titlesByPath.set(target.path, titles);
    });
    const collisions = [...titlesByPath.values()].filter((titles) => titles.length > 1);
    if (collisions.length > 0) {
      setErrorMsg(
        `같은 파일로 저장되어 서로 덮어쓰는 대본이 있습니다: ${collisions
          .map((titles) => titles.join(' / '))
          .join(', ')}. 제목을 다르게 바꾼 뒤 다시 시작하세요.`,
      );
      return false;
    }

    const existing = resolved.filter((target) => target.exists);
    if (existing.length > 0) {
      const preview = existing
        .slice(0, 8)
        .map((target) => `· ${target.path}`)
        .join('\n');
      const more = existing.length > 8 ? `\n… 외 ${existing.length - 8}개` : '';
      if (
        !window.confirm(
          `이미 저장된 파일 ${existing.length}개를 덮어씁니다:\n${preview}${more}\n\n계속할까요?`,
        )
      ) {
        return false;
      }
    }
    return true;
  };

  const runQueue = async (targets: ScriptItem[]) => {
    const thresholdDb = settings.tts_speech_threshold_db;
    const startTimeoutMs = settings.tts_start_timeout_secs * 1000;
    const silenceMs = Math.max(1, settings.tts_auto_stop_seconds) * 1000;
    // 단락(화자) 전환 시 다음 오디오 생성을 기다리는 유예. 일반 무음 판정보다 넉넉하게 둔다.
    const segmentGapMs = Math.max(silenceMs * 2, 8000);
    const gapMs = settings.tts_gap_secs * 1000;

    /** 실패/중단을 기록하고, 다음 대본으로 계속할지 알려준다. */
    const recordFailure = async (
      script: ScriptItem,
      result: StepResult,
      messagePrefix: string,
      outputPath?: string | null,
    ): Promise<boolean> => {
      if (result.aborted || abortRef.current) {
        setItem(script.id, { status: 'skipped', message: '중단됨', outputPath });
        return false;
      }
      // 실패해도 이미 녹음된 파일이 있으면 경로를 남긴다 — 사용자가 직접 듣고 복구할 수
      // 있어야 한다. 자동으로 대본에 연결하지는 않는다(잘렸을 수 있는 결과를 완성본으로
      // 붙여 버리는 것이 바로 이 화면의 원래 버그였다).
      const head = messagePrefix ? `${messagePrefix}: ${result.detail}` : result.detail;
      setItem(script.id, {
        status: 'failed',
        message: outputPath ? `${head} · 녹음 파일: ${outputPath}` : head,
        outputPath,
      });
      if (!settings.tts_batch_continue_on_error) return false;
      await sleep(gapMs);
      return true;
    };

    // 재생 버튼을 누른다. 버튼이 비활성일 수 있어 페이지 쪽에서 활성화를 기다리므로
    // 여기서도 넉넉한 타임아웃을 준다.
    const pressPlay = async (): Promise<StepResult & { mark: number }> => {
      // 포커스가 없으면 반응하지 않는 컨트롤이 있어 창을 앞으로 올린다.
      try {
        await invoke('focus_typecast_browser');
      } catch {
        // 창이 없으면 아래에서 실패로 잡힌다.
      }
      const mark = markSteps();
      try {
        await invoke('typecast_play');
      } catch (err) {
        return { ok: false, detail: `재생 실행 실패: ${err}`, mark };
      }
      return { ...(await waitForStep(['playing'], ['play-failed'], 12000, mark)), mark };
    };

    /** 소리가 시작되지 않은 첫 재생을 정지 상태로 되돌린 뒤 한 번만 다시 누른다. */
    const restartPlayback = async (): Promise<StepResult> => {
      try {
        await invoke('typecast_stop_playback');
      } catch (err) {
        return { ok: false, detail: `재생 정지 실패: ${err}` };
      }
      await sleep(PLAYBACK_RESTART_GAP_MS);
      if (abortRef.current) return { ok: false, detail: '중단됨', aborted: true };
      return pressPlay();
    };

    for (const script of targets) {
      if (abortRef.current) {
        setItem(script.id, { status: 'skipped', message: '중단됨' });
        continue;
      }

      setCurrentId(script.id);

      // 1. 대본 전체를 편집기에 넣고 실제로 들어갔는지 확인한다.
      //    (녹음 시작 전에 넣어 두어야 입력 대기 시간이 결과 파일에 들어가지 않는다.)
      setItem(script.id, { status: 'preparing', message: '대본 입력 중' });
      setPhaseMessage('대본을 Typecast 편집기에 입력하는 중...');
      const prepareMark = markSteps();
      try {
        // 무인 실행이라 붙여넣을 사람이 없다. 대본마다 복사하면 사용자 클립보드만 망친다.
        await invoke('typecast_prepare_script', {
          text: script.content,
          copyToClipboard: false,
        });
      } catch (err) {
        if (!(await recordFailure(script, { ok: false, detail: String(err) }, '대본 주입 실패')))
          break;
        continue;
      }
      const prepared = await waitForStep(['prepared'], ['prepare-failed'], 10000, prepareMark);
      if (!prepared.ok) {
        if (!(await recordFailure(script, prepared, ''))) break;
        continue;
      }

      // 편집기가 입력을 소화할 시간을 준다(녹음 시작 전이라 파일에 무음이 안 남는다).
      setPhaseMessage('편집기 반영을 기다리는 중...');
      await sleep(EDITOR_SETTLE_MS);
      if (abortRef.current) {
        setItem(script.id, { status: 'skipped', message: '중단됨' });
        break;
      }

      // 2. 녹음 시작 (시스템 사운드만, 무음 자동 종료는 여기서 직접 판정)
      setItem(script.id, { status: 'recording', message: '재생 대기' });
      setPhaseMessage('녹음을 시작하고 재생을 요청합니다...');
      let outputPath: string | null = null;
      try {
        outputPath = await onStartRecord({
          fileNamePrefix: script.title,
          showMiniController: false,
          exactFileName: true,
          // 덮어쓰기는 시작 전에 한 번에 확인했다. 실패는 항목 옆에 인라인으로 보여준다.
          skipOverwriteCheck: true,
          throwOnError: true,
          settingsOverride: {
            system_audio_enabled: true,
            mic_audio_enabled: settings.tts_mic_enabled,
            // 무음 자동 일시정지 · 노이즈 게이트 · 80Hz Low-cut 은 환경 설정 값을 그대로 쓴다.
            // 무음 자동 종료만 끈다. 녹음을 언제 끝낼지는 이 화면이 직접 판정해
            // 다음 대본으로 넘어가는 시점을 관리해야 하기 때문이다.
            auto_stop_enabled: false,
          },
        });
      } catch (err) {
        if (!(await recordFailure(script, { ok: false, detail: String(err) }, '녹음 시작 실패')))
          break;
        continue;
      }
      if (!outputPath) {
        if (
          !(await recordFailure(script, { ok: false, detail: '녹음을 시작하지 못했습니다' }, ''))
        )
          break;
        continue;
      }
      recordingActiveRef.current = true;

      // 3. 재생 버튼 클릭
      await sleep(300);
      const played = await pressPlay();

      // 4. 소리로 낭독 시작/종료를 판정
      let result: StepResult = played;
      if (played.ok) {
        setItem(script.id, { status: 'speaking', message: '낭독 녹음 중' });
        const hardCapMs = Math.max(60000, script.estimated_secs * 3000 + 60000);
        result = await waitForSpeechCycle(
          thresholdDb,
          startTimeoutMs,
          silenceMs,
          hardCapMs,
          segmentGapMs,
          played.mark,
          restartPlayback,
        );
      }

      // 5. 저장 & 대본에 연결
      setItem(script.id, { status: 'saving', message: '저장 중' });
      setPhaseMessage('녹음을 저장하는 중...');
      let stopped = await stopRecordingIfActive();
      if (!stopped.ok && recordingActiveRef.current) {
        // 정지가 거부됐는데 백엔드는 아직 녹음 중이라고 답했다(정지 처리 중일 수 있다).
        // 소유권이 남아 있으니 잠깐 뒤 한 번 더 시도한다.
        await sleep(1000);
        stopped = await stopRecordingIfActive();
      }
      try {
        await invoke('typecast_stop_playback');
      } catch {
        // 재생 정지 버튼이 없어도 무시한다.
      }

      // 녹음이 여전히 살아 있으면 다른 무엇보다 먼저 다룬다. 이 상태로 다음 대본을 시작하면
      // "이미 녹음 중" 으로 연달아 실패하고, 살아 있는 녹음은 계속 이 파일에 쓴다. 배치를
      // 끊어 finally / 중단 버튼이 정지를 다시 시도하게 하고(소유권은 유지된다) 사용자에게 알린다.
      if (recordingActiveRef.current) {
        setErrorMsg(
          `녹음을 정지하지 못해 자동 처리를 중단했습니다: ${stopped.detail}` +
            ' 녹음 화면에서 정지 상태를 확인하세요.',
        );
        await recordFailure(script, stopped, '녹음 정지 실패', outputPath);
        break;
      }

      if (!result.ok) {
        if (!(await recordFailure(script, result, '', outputPath))) break;
        continue;
      }
      if (!stopped.ok) {
        if (!(await recordFailure(script, stopped, '녹음 저장 실패', outputPath))) break;
        continue;
      }

      try {
        await invoke('attach_script_recording', { id: script.id, recordedPath: outputPath });
      } catch (err) {
        // 연결 실패를 '완료' 로 표시하면 사용자는 대본에 녹음이 붙은 줄 알지만 실제로는 없다.
        // 파일 자체는 남아 있으니 경로를 상세에 남기고(자동 재연결은 하지 않는다) 실패 정책을 따른다.
        const keepGoing = await recordFailure(
          script,
          { ok: false, detail: String(err) },
          '대본에 녹음 연결 실패',
          outputPath,
        );
        if (!keepGoing) break;
        continue;
      }
      setItem(script.id, { status: 'done', message: result.detail, outputPath });

      if (abortRef.current) break;
      await sleep(gapMs);
    }
  };

  const runBatch = async () => {
    const targets = scripts.filter((s) => selectedIds.includes(s.id));
    if (targets.length === 0) {
      setErrorMsg('처리할 대본을 먼저 선택하세요.');
      return;
    }
    if (recordingStatus.status !== 'idle') {
      setErrorMsg('다른 녹음이 진행 중입니다. 먼저 종료한 뒤 다시 시작하세요.');
      return;
    }

    setErrorMsg(null);
    setNotice(null);
    abortRef.current = false;
    setQueue([]);
    setIsRunning(true);

    try {
      // 구독이 등록되기 전에 Typecast 를 건드리면 페이지가 동기적으로 내는 실패 보고를 놓쳐
      // 허위 타임아웃이 된다. 첫 `typecast_*` invoke(ensureBrowserReady) 앞에서 기다린다.
      const listenerError = await ensureListenersReady();
      if (listenerError) {
        setErrorMsg(listenerError);
        return;
      }
      // 사전 점검을 통과하기 전에는 큐를 만들지 않는다. 여기서 되돌아가면 아직 아무것도
      // 시작하지 않은 것이므로, 목록에 '건너뜀' 항목이 남지 않게 한다.
      if (!(await ensureBrowserReady())) return;
      if (!(await confirmOutputTargets(targets))) return;
      setQueue(
        targets.map((s) => ({
          scriptId: s.id,
          title: s.title,
          status: 'pending' as BatchItemStatus,
        })),
      );
      await runQueue(targets);
    } catch (err) {
      // 어떤 이유로든 루프가 튕겨도 아래 finally 가 녹음을 반드시 정리하도록 한다.
      setErrorMsg(`자동 처리 중 오류가 발생했습니다: ${err}`);
    } finally {
      // 남은 항목을 '대기' 인 채로 방치하지 않는다(중단·즉시 중단 옵션으로 빠져나온 경우).
      setQueue((prev) =>
        prev.map((item) =>
          UNFINISHED_STATUSES.includes(item.status)
            ? { ...item, status: 'skipped', message: item.message ?? '진행되지 않음' }
            : item,
        ),
      );
      setCurrentId(null);
      setPhaseMessage('');
      setIsRunning(false);
      // 남은 녹음을 반드시 정리한다. 정지가 실패하면 소유권이 유지되므로(중단 버튼으로 재시도)
      // 조용히 넘기지 않고 사용자에게 알린다.
      const stoppedAtEnd = await stopRecordingIfActive();
      if (!stoppedAtEnd.ok) {
        setErrorMsg((prev) =>
          [prev, `녹음을 정지하지 못했습니다: ${stoppedAtEnd.detail}`].filter(Boolean).join(' / '),
        );
      }
      await onRefreshScripts();
    }
  };

  const stopBatch = async () => {
    abortRef.current = true;
    setPhaseMessage('중단하는 중...');
    const stopped = await stopRecordingIfActive();
    if (!stopped.ok) {
      setErrorMsg(`녹음을 정지하지 못했습니다: ${stopped.detail}`);
    }
    try {
      await invoke('typecast_stop_playback');
    } catch {
      // 무시
    }
  };

  const doneCount = queue.filter((q) => q.status === 'done').length;
  const failedCount = queue.filter((q) => q.status === 'failed').length;
  const skippedCount = queue.filter((q) => q.status === 'skipped').length;
  const progress =
    queue.length > 0
      ? Math.round(((doneCount + failedCount + skippedCount) / queue.length) * 100)
      : 0;

  const allSelected = scripts.length > 0 && selectedIds.length === scripts.length;

  return (
    <div className="space-y-4 pb-6">
      {/* Typecast 로그인 · 창 제어 (수동 녹음 화면과 같은 공용 카드) */}
      <TypecastSessionCard
        settings={settings}
        onUpdateSettings={onUpdateSettings}
        onNotice={setNotice}
        onError={setErrorMsg}
      />

      {/* 대본 선택 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg space-y-3">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <ListChecks className="w-4 h-4 text-indigo-400" />
            자동 처리할 대본 선택
            <span className="text-[11px] font-semibold text-indigo-400">
              {selectedIds.length}개 선택됨
            </span>
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => onSelectAll(allSelected ? [] : scripts.map((s) => s.id))}
              disabled={scripts.length === 0}
              className="text-[11px] font-semibold text-slate-400 hover:text-slate-200 disabled:opacity-40 transition"
            >
              {allSelected ? '전체 해제' : '전체 선택'}
            </button>
            <button
              onClick={onGoToLibrary}
              className="text-[11px] font-semibold text-emerald-400 hover:text-emerald-300 transition"
            >
              대본 관리
            </button>
          </div>
        </div>

        {scripts.length === 0 ? (
          <div className="text-xs text-slate-500 py-6 text-center rounded-xl border border-dashed border-slate-800">
            저장된 대본이 없습니다. "대본 관리"에서 먼저 대본을 만들어 주세요.
          </div>
        ) : (
          <div className="max-h-60 overflow-y-auto space-y-1.5 pr-1">
            {scripts.map((script) => {
              const checked = selectedIds.includes(script.id);
              return (
                <label
                  key={script.id}
                  className={`flex items-center gap-3 px-3 py-2 rounded-xl border cursor-pointer transition ${
                    checked
                      ? 'bg-indigo-950/40 border-indigo-700/60'
                      : 'bg-slate-950 border-slate-800 hover:border-slate-700'
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={isRunning}
                    onChange={() => onToggleSelect(script.id)}
                    className="w-4 h-4 accent-indigo-500 shrink-0"
                  />
                  <span className="text-xs font-semibold text-slate-200 truncate flex-1">
                    {script.title}
                  </span>
                  <span className="text-[10px] font-mono text-slate-500 shrink-0">
                    {script.char_count.toLocaleString()}자 · ≈
                    {formatDuration(script.estimated_secs)}
                  </span>
                </label>
              );
            })}
          </div>
        )}
      </div>

      {/* 실행 컨트롤 */}
      <div className="bg-slate-900/90 border border-slate-800 rounded-2xl p-5 shadow-xl space-y-4">
        <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-bold text-slate-200">
              {isRunning ? '자동 처리 진행 중' : '선택한 대본을 순서대로 자동 녹음'}
            </p>
            <p className="text-[11px] text-slate-400 mt-0.5">
              {isRunning
                ? phaseMessage || '진행 중...'
                : '대본 입력 → 재생 → 녹음 → 저장을 대본마다 자동으로 반복합니다.'}
            </p>
          </div>

          <div className="flex items-center gap-2 shrink-0">
            <button
              onClick={() => invoke('typecast_probe').catch((e) => setErrorMsg(String(e)))}
              disabled={isRunning}
              title="편집기와 재생 버튼을 찾을 수 있는지 미리 확인합니다"
              className="flex items-center gap-1.5 px-3.5 py-3 rounded-xl bg-slate-800 hover:bg-slate-700 disabled:opacity-40 text-slate-200 text-xs font-semibold border border-slate-700 transition"
            >
              <Search className="w-3.5 h-3.5 text-cyan-400" />
              <span>연동 테스트</span>
            </button>
            {!isRunning ? (
              <button
                onClick={runBatch}
                disabled={selectedIds.length === 0}
                className="flex items-center gap-2 px-6 py-3 rounded-xl bg-gradient-to-r from-indigo-600 to-cyan-600 hover:from-indigo-500 hover:to-cyan-500 disabled:from-slate-800 disabled:to-slate-800 disabled:text-slate-500 text-white font-bold text-sm shadow-xl shadow-indigo-600/25 active:scale-95 transition"
              >
                <Play className="w-4 h-4 fill-current" />
                <span>{selectedIds.length}개 자동 처리 시작</span>
              </button>
            ) : (
              <button
                onClick={stopBatch}
                className="flex items-center gap-2 px-6 py-3 rounded-xl bg-red-600 hover:bg-red-500 text-white font-bold text-sm shadow-xl shadow-red-600/25 active:scale-95 transition"
              >
                <Square className="w-4 h-4 fill-current" />
                <span>중단</span>
              </button>
            )}
          </div>
        </div>

        {queue.length > 0 && (
          <>
            <div className="h-2 rounded-full bg-slate-950 overflow-hidden border border-slate-800">
              <div
                className="h-full bg-gradient-to-r from-indigo-500 to-cyan-400 transition-all duration-300"
                style={{ width: `${progress}%` }}
              />
            </div>
            <div className="flex items-center gap-3 text-[11px] font-mono text-slate-400">
              <span className="text-emerald-400">완료 {doneCount}</span>
              <span className="text-red-400">실패 {failedCount}</span>
              {skippedCount > 0 && <span className="text-slate-500">건너뜀 {skippedCount}</span>}
              <span>전체 {queue.length}</span>
              {(recordingStatus.status === 'recording' || recordingStatus.status === 'paused') && (
                <>
                  <span className="text-red-400 flex items-center gap-1">
                    <Mic className="w-3 h-3" />
                    {Math.floor(recordingStatus.duration_secs)}초
                  </span>
                  {/* 소리가 실제로 캡처되고 있는지 바로 보이도록 시스템 오디오 레벨을 노출한다. */}
                  <span
                    className={
                      recordingStatus.sys_vu_level > settings.tts_speech_threshold_db
                        ? 'text-emerald-400'
                        : 'text-slate-600'
                    }
                    title="시스템 오디오 입력 레벨. 낭독 중인데 계속 회색이면 소리가 캡처되지 않는 것입니다."
                  >
                    시스템 {Math.round(recordingStatus.sys_vu_level)}dB
                  </span>
                </>
              )}
            </div>

            <div className="space-y-1.5 max-h-64 overflow-y-auto pr-1">
              {queue.map((item) => (
                <div
                  key={item.scriptId}
                  className={`flex items-center gap-3 px-3 py-2 rounded-xl border ${
                    item.scriptId === currentId
                      ? 'bg-slate-950 border-indigo-700/60'
                      : 'bg-slate-950 border-slate-800'
                  }`}
                >
                  <span className="shrink-0">
                    {item.status === 'done' ? (
                      <Check className="w-4 h-4 text-emerald-400" />
                    ) : item.status === 'failed' ? (
                      <X className="w-4 h-4 text-red-400" />
                    ) : item.scriptId === currentId ? (
                      <Loader className="w-4 h-4 text-indigo-400 animate-spin" />
                    ) : (
                      <CircleDashed className="w-4 h-4 text-slate-600" />
                    )}
                  </span>
                  <span className="text-xs font-semibold text-slate-200 truncate flex-1">
                    {item.title}
                  </span>
                  {item.message && (
                    <span
                      className="text-[10px] text-slate-500 truncate max-w-[240px]"
                      title={item.message}
                    >
                      {item.message}
                    </span>
                  )}
                  {/* 실패·건너뜀이라도 파일이 남아 있으면 열 수 있게 한다 — 잘린 결과를 완성본으로
                      대본에 붙이지 않는 대신, 사용자가 직접 확인해 복구할 경로를 준다. */}
                  {item.outputPath && (
                    <button
                      onClick={() => onOpenExplorer(item.outputPath!)}
                      title="폴더 열기"
                      className="p-1 rounded-lg hover:bg-slate-800 text-slate-400 transition shrink-0"
                    >
                      <FolderOpen className="w-3.5 h-3.5" />
                    </button>
                  )}
                  <span
                    className={`text-[10px] font-bold px-2 py-0.5 rounded border shrink-0 ${STATUS_STYLE[item.status]}`}
                  >
                    {STATUS_LABEL[item.status]}
                  </span>
                </div>
              ))}
            </div>
          </>
        )}

        {(errorMsg || notice) && (
          <div
            className={`rounded-xl px-3.5 py-2.5 text-xs font-semibold flex items-center gap-2 border ${
              errorMsg
                ? 'bg-red-950/50 border-red-800/60 text-red-300'
                : 'bg-emerald-950/50 border-emerald-800/60 text-emerald-300'
            }`}
          >
            {errorMsg ? <AlertCircle className="w-3.5 h-3.5" /> : <Check className="w-3.5 h-3.5" />}
            <span>{errorMsg || notice}</span>
          </div>
        )}
      </div>

      {/* 자동화 세부 설정 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl shadow-lg overflow-hidden">
        <button
          onClick={() => setShowAdvanced((v) => !v)}
          className="w-full flex items-center justify-between px-4 py-3 text-xs font-bold text-slate-300 hover:text-slate-100 transition"
        >
          <span className="flex items-center gap-2">
            <SettingsIcon className="w-4 h-4 text-slate-400" />
            자동화 세부 설정
          </span>
          <ChevronDown
            className={`w-4 h-4 transition-transform ${showAdvanced ? 'rotate-180' : ''}`}
          />
        </button>

        {showAdvanced && (
          <div className="px-4 pb-4 space-y-3 border-t border-slate-800 pt-3">
            <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
              <label className="block">
                <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
                  낭독 종료 판정 무음 ({settings.tts_auto_stop_seconds}초)
                </span>
                <input
                  type="range"
                  min={1}
                  max={10}
                  step={0.5}
                  value={settings.tts_auto_stop_seconds}
                  onChange={(e) =>
                    onUpdateSettings({ tts_auto_stop_seconds: Number(e.target.value) })
                  }
                  className="w-full accent-indigo-500"
                />
              </label>

              <label className="block">
                <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
                  소리 감지 임계값 ({settings.tts_speech_threshold_db} dB)
                </span>
                <input
                  type="range"
                  min={-60}
                  max={-20}
                  step={1}
                  value={settings.tts_speech_threshold_db}
                  onChange={(e) =>
                    onUpdateSettings({ tts_speech_threshold_db: Number(e.target.value) })
                  }
                  className="w-full accent-indigo-500"
                />
              </label>

              <label className="block">
                <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
                  재생 시작 대기 ({settings.tts_start_timeout_secs}초)
                </span>
                <input
                  type="range"
                  min={5}
                  max={90}
                  step={5}
                  value={settings.tts_start_timeout_secs}
                  onChange={(e) =>
                    onUpdateSettings({ tts_start_timeout_secs: Number(e.target.value) })
                  }
                  className="w-full accent-indigo-500"
                />
              </label>

              <label className="block">
                <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
                  대본 사이 간격 ({settings.tts_gap_secs}초)
                </span>
                <input
                  type="range"
                  min={0}
                  max={15}
                  step={1}
                  value={settings.tts_gap_secs}
                  onChange={(e) => onUpdateSettings({ tts_gap_secs: Number(e.target.value) })}
                  className="w-full accent-indigo-500"
                />
              </label>

            </div>

            {/* 환경 설정의 무음 · DSP 설정이 그대로 적용된다는 것을 보여 준다. */}
            <div className="rounded-xl bg-slate-950 border border-slate-800 p-3 space-y-1.5">
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] font-bold text-slate-300">
                  환경 설정의 무음 처리 적용 중
                </span>
                <button
                  onClick={onOpenSettings}
                  className="text-[10px] font-semibold text-indigo-400 hover:text-indigo-300 transition"
                >
                  환경 설정에서 변경
                </button>
              </div>
              <div className="text-[10px] text-slate-400 font-mono leading-relaxed">
                <div>
                  무음 자동 일시정지:{' '}
                  <b className={settings.auto_pause_enabled ? 'text-emerald-400' : 'text-slate-500'}>
                    {settings.auto_pause_enabled
                      ? `ON · ${settings.auto_pause_seconds.toFixed(1)}초`
                      : 'OFF'}
                  </b>
                  {settings.auto_pause_enabled && ' — 낭독 사이 무음이 파일에서 잘려 나갑니다'}
                </div>
                <div>
                  노이즈 게이트:{' '}
                  <b className={settings.noise_gate_enabled ? 'text-emerald-400' : 'text-slate-500'}>
                    {settings.noise_gate_enabled
                      ? `${settings.noise_gate_threshold_db}dB`
                      : 'OFF'}
                  </b>
                  {' · '}80Hz Low-cut:{' '}
                  <b className={settings.highpass_filter_enabled ? 'text-emerald-400' : 'text-slate-500'}>
                    {settings.highpass_filter_enabled ? 'ON' : 'OFF'}
                  </b>
                </div>
                <div className="text-slate-500">
                  무음 자동 종료는 이 화면이 직접 판정합니다 (아래 "낭독 종료 판정 무음").
                </div>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-4">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={settings.tts_batch_continue_on_error}
                  onChange={(e) =>
                    onUpdateSettings({ tts_batch_continue_on_error: e.target.checked })
                  }
                  className="w-4 h-4 accent-indigo-500"
                />
                <span className="text-[11px] font-semibold text-slate-300">
                  실패해도 다음 대본 계속 진행
                </span>
              </label>
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={settings.tts_mic_enabled}
                  onChange={(e) => onUpdateSettings({ tts_mic_enabled: e.target.checked })}
                  className="w-4 h-4 accent-indigo-500"
                />
                <span className="text-[11px] font-semibold text-slate-300">마이크도 함께 녹음</span>
              </label>
            </div>
            <div>
              <label className="text-[11px] font-semibold text-slate-400 mb-1 block">
                사용자 지정 Chrome 경로 (선택)
              </label>
              <div className="flex items-center gap-2">
                <input
                  value={settings.custom_chrome_path || ''}
                  onChange={(e) =>
                    onUpdateSettings({ custom_chrome_path: e.target.value.trim() || null })
                  }
                  placeholder="자동 감지 사용 (비워두면 OS별 기본 설치 위치 자동 탐색)"
                  className="flex-1 px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 font-mono placeholder:text-slate-600 focus:outline-none focus:border-indigo-600"
                />
                <button
                  type="button"
                  onClick={handleTestChrome}
                  className="px-3 py-2 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-bold rounded-xl transition"
                >
                  테스트
                </button>
              </div>
              {chromeTestMsg && (
                <p className="text-[11px] font-mono text-slate-400 mt-1">{chromeTestMsg}</p>
              )}
              <p className="text-[10px] text-slate-500 mt-1">
                Typecast 는 앱 내장 화면이 아니라 실제 Google Chrome 을 별도로 실행해 자동화합니다.
                로그인 세션은 이 앱 전용 Chrome 프로필에만 저장되며 평소 쓰는 Chrome 프로필과는
                공유되지 않습니다.
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-3 pt-1">
              <div>
                <label className="text-[11px] font-semibold text-slate-400 mb-1 block">
                  편집기 CSS 선택자 (선택)
                </label>
                <input
                  value={settings.typecast_editor_selector}
                  onChange={(e) => onUpdateSettings({ typecast_editor_selector: e.target.value })}
                  placeholder="비우면 자동 탐색"
                  className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 font-mono placeholder:text-slate-600 focus:outline-none focus:border-indigo-600"
                />
              </div>
              <div>
                <label className="text-[11px] font-semibold text-slate-400 mb-1 block">
                  재생 버튼 CSS 선택자 (선택)
                </label>
                <input
                  value={settings.typecast_play_selector}
                  onChange={(e) => onUpdateSettings({ typecast_play_selector: e.target.value })}
                  placeholder="비우면 내장 선택자 + 자동 탐색"
                  className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 font-mono placeholder:text-slate-600 focus:outline-none focus:border-indigo-600"
                />
              </div>
            </div>
            <p className="text-[10px] text-slate-500">
              Typecast 하단 플레이어 바의 재생 버튼은 내장 선택자로 잡습니다. "연동 테스트"를 눌러 아래
              진단 로그의 <code className="text-cyan-500">play=</code> 항목과 어떤 경로로 찾았는지를
              확인하고, 잘못 잡히면 선택자를 직접 지정하세요.
            </p>
          </div>
        )}
      </div>

      <TypecastDiagnosticsLog onCopy={setNotice} />
    </div>
  );
};
