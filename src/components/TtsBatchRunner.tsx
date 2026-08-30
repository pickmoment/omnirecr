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
  Settings,
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
  onStopRecord: () => Promise<void>;
  onRefreshScripts: () => Promise<void>;
  onOpenExplorer: (path: string) => Promise<void>;
  onGoToLibrary: () => void;
  onOpenSettings: () => void;
}

const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

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
  // 진행 중 정리해야 할 이벤트 구독들
  const cleanupRef = useRef<UnlistenFn[]>([]);

  useEffect(
    () => () => {
      cleanupRef.current.forEach((u) => u());
      cleanupRef.current = [];
    },
    [],
  );

  const setItem = useCallback((scriptId: string, patch: Partial<BatchItemState>) => {
    setQueue((prev) =>
      prev.map((item) => (item.scriptId === scriptId ? { ...item, ...patch } : item)),
    );
  }, []);

  /**
   * 페이지 자동화 단계 보고(`typecast_step`)를 기다린다.
   * 성공 이름 중 하나가 오면 ok, 실패 이름이 오면 실패, 시간 내 아무것도 없으면 타임아웃.
   */
  const waitForStep = (
    successNames: string[],
    failureNames: string[],
    timeoutMs: number,
  ): Promise<{ ok: boolean; detail: string }> =>
    new Promise((resolve) => {
      let settled = false;
      let unlisten: UnlistenFn | null = null;

      const finish = (result: { ok: boolean; detail: string }) => {
        if (settled) return;
        settled = true;
        window.clearTimeout(timer);
        if (unlisten) unlisten();
        resolve(result);
      };

      const timer = window.setTimeout(
        () => finish({ ok: false, detail: '페이지 응답 시간 초과' }),
        timeoutMs,
      );

      listen<TypecastStepPayload>('typecast_step', (event) => {
        const { name, detail } = event.payload;
        if (successNames.includes(name)) finish({ ok: true, detail });
        else if (failureNames.includes(name)) finish({ ok: false, detail });
      }).then((fn) => {
        unlisten = fn;
        if (settled) fn();
      });
    });

  /**
   * 시스템 오디오 레벨을 보고 낭독 시작 / 종료를 판정한다.
   * Typecast 가 어떤 방식으로 재생하든(미디어 엘리먼트 · Web Audio) 동작한다.
   *
   * Typecast 사이트를 최대한 건드리지 않는다. 단락(화자) 전환으로 다음 오디오를 새로
   * 생성하는 동안의 정상적인 무음(`step:media-ended`)과, 사이트가 낭독 도중 스스로
   * 재생을 멈추는 오동작(`ended` 없는 `step:media-pause`) 둘 다 재생 버튼을 다시 누르는
   * 등 추가로 개입하지 않고 `segmentGapMs` 만큼 무음 판정만 미뤄 다음 재생을 기다린다.
   * (재생 버튼을 다시 누르는 방식은 이미 재생 중인데 또 클릭해 토글을 꺼버리는 등
   * 오히려 사이트 동작을 방해할 수 있어 쓰지 않는다.) 그 유예 안에 재생이 돌아오지
   * 않으면 낭독이 실제로 끝난 것으로 보고 정상 종료 처리한다.
   */
  const waitForSpeechCycle = (
    thresholdDb: number,
    startTimeoutMs: number,
    silenceMs: number,
    hardCapMs: number,
    segmentGapMs: number,
  ): Promise<{ ok: boolean; detail: string }> =>
    new Promise((resolve) => {
      let settled = false;
      let unlistenVu: UnlistenFn | null = null;
      let unlistenStep: UnlistenFn | null = null;
      let started = false;
      let lastLoudAt = Date.now();
      // media-ended / media-pause 이후 재생이 이어지길 기다리는 유예 시작 시각. null 이면 유예 없음.
      let segmentEndedAt: number | null = null;
      const beganAt = Date.now();

      const finish = (result: { ok: boolean; detail: string }) => {
        if (settled) return;
        settled = true;
        window.clearInterval(ticker);
        if (unlistenVu) unlistenVu();
        if (unlistenStep) unlistenStep();
        resolve(result);
      };

      const ticker = window.setInterval(() => {
        if (abortRef.current) {
          finish({ ok: false, detail: '사용자가 중단했습니다' });
          return;
        }
        const now = Date.now();
        if (!started) {
          if (now - beganAt > startTimeoutMs) {
            finish({
              ok: false,
              detail: '재생 소리가 감지되지 않았습니다 (시스템 오디오 캡처 · 임계값 확인)',
            });
          }
          return;
        }
        if (now - beganAt > hardCapMs) {
          finish({ ok: true, detail: '최대 녹음 시간 도달' });
          return;
        }
        // 단락 전환 · 재생 오동작 회복 유예 기간 중에는 무음이어도 재생이 이어지길 기다린다.
        if (segmentEndedAt !== null && now - segmentEndedAt < segmentGapMs) {
          return;
        }
        if (now - lastLoudAt > silenceMs) {
          finish({ ok: true, detail: `${Math.round((now - beganAt) / 1000)}초 녹음` });
        }
      }, 200);

      listen<AudioVUMeterPayload>('audio_vu_meter', (event) => {
        if (event.payload.sys_level_db > thresholdDb) {
          if (!started) {
            started = true;
            setPhaseMessage('낭독 녹음 중...');
          }
          lastLoudAt = Date.now();
          segmentEndedAt = null;
        }
      }).then((fn) => {
        unlistenVu = fn;
        if (settled) fn();
      });

      listen<TypecastStepPayload>('typecast_step', (event) => {
        if (event.payload.name === 'media-ended' || event.payload.name === 'media-pause') {
          segmentEndedAt = Date.now();
        } else if (event.payload.name === 'media-play') {
          segmentEndedAt = null;
        }
      }).then((fn) => {
        unlistenStep = fn;
        if (settled) fn();
      });
    });

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

  const ensureBrowserReady = async (): Promise<boolean> => {
    try {
      const state = await invoke<{ is_open: boolean }>('get_typecast_browser_state');
      if (!state.is_open) {
        await invoke('open_typecast_browser', { url: settings.typecast_editor_url });
        await sleep(2500);
      }
      return true;
    } catch (err) {
      setErrorMsg(`Typecast 창을 준비하지 못했습니다: ${err}`);
      return false;
    }
  };

  const runBatch = async () => {
    const targets = scripts.filter((s) => selectedIds.includes(s.id));
    if (targets.length === 0) {
      setErrorMsg('처리할 대본을 먼저 선택하세요.');
      return;
    }

    setErrorMsg(null);
    abortRef.current = false;
    setIsRunning(true);
    setQueue(
      targets.map((s) => ({ scriptId: s.id, title: s.title, status: 'pending' as BatchItemStatus })),
    );

    const ready = await ensureBrowserReady();
    if (!ready) {
      setIsRunning(false);
      return;
    }

    const thresholdDb = settings.tts_speech_threshold_db;
    const startTimeoutMs = settings.tts_start_timeout_secs * 1000;
    const silenceMs = Math.max(1, settings.tts_auto_stop_seconds) * 1000;
    // 단락(화자) 전환 시 다음 오디오 생성을 기다리는 유예. 일반 무음 판정보다 넉넉하게 둔다.
    const segmentGapMs = Math.max(silenceMs * 2, 8000);
    const gapMs = settings.tts_gap_secs * 1000;

    // 재생 버튼을 누른다. 버튼이 비활성일 수 있어 페이지 쪽에서 활성화를 기다리므로
    // 여기서도 넉넉한 타임아웃을 준다.
    const pressPlay = async (): Promise<{ ok: boolean; detail: string }> => {
      // 포커스가 없으면 반응하지 않는 컨트롤이 있어 창을 앞으로 올린다.
      try {
        await invoke('focus_typecast_browser');
      } catch {
        // 창이 없으면 아래에서 실패로 잡힌다.
      }
      try {
        await invoke('typecast_play');
      } catch (err) {
        return { ok: false, detail: `재생 실행 실패: ${err}` };
      }
      return waitForStep(['playing'], ['play-failed'], 12000);
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
      try {
        await invoke('typecast_prepare_script', { text: script.content });
      } catch (err) {
        setItem(script.id, { status: 'failed', message: `대본 주입 실패: ${err}` });
        if (!settings.tts_batch_continue_on_error) break;
        continue;
      }
      const prepared = await waitForStep(['prepared'], ['prepare-failed'], 10000);
      if (!prepared.ok) {
        setItem(script.id, { status: 'failed', message: prepared.detail });
        if (!settings.tts_batch_continue_on_error) break;
        continue;
      }

      // 2. 녹음 시작 (시스템 사운드만, 무음 자동 종료는 여기서 직접 판정)
      setItem(script.id, { status: 'recording', message: '재생 대기' });
      setPhaseMessage('녹음을 시작하고 재생을 요청합니다...');
      const outputPath = await onStartRecord({
        fileNamePrefix: script.title,
        showMiniController: false,
        exactFileName: true,
        settingsOverride: {
          system_audio_enabled: true,
          mic_audio_enabled: settings.tts_mic_enabled,
          // 무음 자동 일시정지 · 노이즈 게이트 · 80Hz Low-cut 은 환경 설정 값을 그대로 쓴다.
          // 무음 자동 종료만 끈다. 녹음을 언제 끝낼지는 이 화면이 직접 판정해
          // 다음 대본으로 넘어가는 시점을 관리해야 하기 때문이다.
          auto_stop_enabled: false,
        },
      });
      if (!outputPath) {
        setItem(script.id, { status: 'failed', message: '녹음을 시작하지 못했습니다' });
        if (!settings.tts_batch_continue_on_error) break;
        continue;
      }

      // 3. 재생 버튼 클릭
      await sleep(300);
      const played = await pressPlay();

      // 4. 소리로 낭독 시작/종료를 판정
      let result = played;
      if (played.ok) {
        setItem(script.id, { status: 'speaking', message: '낭독 녹음 중' });
        const hardCapMs = Math.max(60000, script.estimated_secs * 3000 + 60000);
        result = await waitForSpeechCycle(thresholdDb, startTimeoutMs, silenceMs, hardCapMs, segmentGapMs);
      }

      // 5. 저장 & 대본에 연결
      setItem(script.id, { status: 'saving', message: '저장 중' });
      setPhaseMessage('녹음을 저장하는 중...');
      await onStopRecord();
      try {
        await invoke('typecast_stop_playback');
      } catch {
        // 재생 정지 버튼이 없어도 무시한다.
      }

      if (!result.ok) {
        setItem(script.id, { status: 'failed', message: result.detail, outputPath });
        if (!settings.tts_batch_continue_on_error || abortRef.current) break;
        await sleep(gapMs);
        continue;
      }

      try {
        await invoke('attach_script_recording', { id: script.id, recordedPath: outputPath });
      } catch (err) {
        console.error('대본에 녹음 연결 실패:', err);
      }
      setItem(script.id, { status: 'done', message: result.detail, outputPath });

      if (abortRef.current) break;
      await sleep(gapMs);
    }

    setCurrentId(null);
    setPhaseMessage('');
    setIsRunning(false);
    await onRefreshScripts();
  };

  const stopBatch = async () => {
    abortRef.current = true;
    setPhaseMessage('중단하는 중...');
    if (recordingStatus.status === 'recording' || recordingStatus.status === 'paused') {
      await onStopRecord();
    }
    try {
      await invoke('typecast_stop_playback');
    } catch {
      // 무시
    }
  };

  const doneCount = queue.filter((q) => q.status === 'done').length;
  const failedCount = queue.filter((q) => q.status === 'failed').length;
  const progress = queue.length > 0 ? Math.round(((doneCount + failedCount) / queue.length) * 100) : 0;

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
                    <span className="text-[10px] text-slate-500 truncate max-w-[240px]">
                      {item.message}
                    </span>
                  )}
                  {item.outputPath && item.status === 'done' && (
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
