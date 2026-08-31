import React, { useMemo, useRef, useState } from 'react';
import {
  AlertCircle,
  Check,
  ChevronDown,
  CircleDashed,
  FileText,
  FolderOpen,
  Loader,
  Play,
  Settings as SettingsIcon,
  Square,
  X,
} from 'lucide-react';
import type { ScriptItem, Settings } from '../types';
import { generateSubtitles, isSubtitleCancelled } from '../services/subtitleGeneration';
import { formatDuration } from '../utils/format';
import { SubtitleOptionsPanel } from './SubtitleOptionsPanel';

type ItemStatus = 'pending' | 'running' | 'done' | 'failed' | 'skipped';

interface ItemState {
  scriptId: string;
  title: string;
  status: ItemStatus;
  message?: string;
  srtPath?: string | null;
  vttPath?: string | null;
  lineCount?: number;
}

interface SubtitleBatchRunnerProps {
  settings: Settings;
  scripts: ScriptItem[];
  onUpdateSettings: (partial: Partial<Settings>) => Promise<void>;
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenSettings: () => void;
}

const STATUS_LABEL: Record<ItemStatus, string> = {
  pending: '대기',
  running: '생성 중',
  done: '완료',
  failed: '실패',
  skipped: '건너뜀',
};

const STATUS_STYLE: Record<ItemStatus, string> = {
  pending: 'bg-slate-800 text-slate-400 border-slate-700',
  running: 'bg-amber-950/60 text-amber-300 border-amber-800/50',
  done: 'bg-emerald-950/60 text-emerald-300 border-emerald-800/50',
  failed: 'bg-red-950/60 text-red-300 border-red-800/50',
  skipped: 'bg-slate-800 text-slate-500 border-slate-700',
};

export const SubtitleBatchRunner: React.FC<SubtitleBatchRunnerProps> = ({
  settings,
  scripts,
  onUpdateSettings,
  onOpenExplorer,
  onOpenSettings,
}) => {
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [queue, setQueue] = useState<ItemState[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [phaseMessage, setPhaseMessage] = useState('');
  const [progressPercent, setProgressPercent] = useState(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);

  const abortRef = useRef(false);
  // 진행 중인 항목까지 실제로 끊는다. 이게 없으면 중단을 눌러도 그 항목은 끝까지 돌아
  // '완료'로 보고되고 파일까지 쓴다(항목 사이에서만 멈추던 예전 동작).
  const abortControllerRef = useRef<AbortController | null>(null);

  // 녹음 결과가 연결된 대본만 자막을 만들 수 있다.
  const recorded = useMemo(
    () => scripts.filter((s) => !!s.last_recorded_path),
    [scripts],
  );

  const toggle = (id: string) =>
    setSelectedIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  const allSelected = recorded.length > 0 && selectedIds.length === recorded.length;

  const setItem = (scriptId: string, patch: Partial<ItemState>) =>
    setQueue((prev) =>
      prev.map((item) => (item.scriptId === scriptId ? { ...item, ...patch } : item)),
    );

  const run = async () => {
    const targets = recorded.filter((s) => selectedIds.includes(s.id));
    if (targets.length === 0) {
      setErrorMsg('자막을 만들 대본을 선택하세요.');
      return;
    }

    setErrorMsg(null);
    abortRef.current = false;
    abortControllerRef.current = new AbortController();
    setIsRunning(true);
    setQueue(
      targets.map((s) => ({ scriptId: s.id, title: s.title, status: 'pending' as ItemStatus })),
    );

    for (const script of targets) {
      if (abortRef.current) {
        setItem(script.id, { status: 'skipped', message: '중단됨' });
        continue;
      }

      const audioPath = script.last_recorded_path;
      if (!audioPath) {
        setItem(script.id, { status: 'failed', message: '연결된 녹음 파일이 없습니다' });
        continue;
      }

      setCurrentId(script.id);
      setItem(script.id, { status: 'running' });
      setProgressPercent(0);

      try {
        const outcome = await generateSubtitles({
          audioPath,
          scriptText: script.content,
          // 대본이 있는 항목만 다루므로 항상 대본 + 싱크 모드로 만든다.
          workflow: 'with-script',
          syncEngine: settings.subtitle_sync_engine,
          whisperModel: settings.subtitle_whisper_model,
          whisperLanguage: settings.subtitle_whisper_language,
          splitMode: settings.subtitle_split_mode,
          splitOnComma: settings.subtitle_split_on_comma,
          maxChars: settings.subtitle_max_chars,
          silenceThresholdDb: settings.subtitle_silence_threshold_db,
          minSilenceDuration: settings.subtitle_min_silence_duration,
          startOffsetSecs: settings.subtitle_start_offset_secs,
          // 일괄 생성은 파일로 남기는 것이 목적이므로 항상 저장한다.
          autoSave: true,
          outputDir: settings.output_dir,
          signal: abortControllerRef.current?.signal,
          onProgress: (message, percent) => {
            setPhaseMessage(`${script.title} — ${message}`);
            if (percent) setProgressPercent(percent);
          },
        });

        // 자동 저장을 요청했는데 한 포맷이라도 저장되지 않았으면 완료가 아니다.
        if (outcome.saveFailures.length > 0) {
          setItem(script.id, {
            status: 'failed',
            message: `자막 파일 저장 실패 — ${outcome.saveFailures
              .map((f) => `${f.format.toUpperCase()}: ${f.message}`)
              .join(' · ')}`,
            srtPath: outcome.srtPath,
            vttPath: outcome.vttPath,
            lineCount: outcome.subtitles.length,
          });
          if (!settings.tts_batch_continue_on_error) break;
          continue;
        }

        setItem(script.id, {
          status: 'done',
          message: `${outcome.subtitles.length}줄 · ${formatDuration(outcome.totalDuration)}`,
          srtPath: outcome.srtPath,
          vttPath: outcome.vttPath,
          lineCount: outcome.subtitles.length,
        });
      } catch (err: unknown) {
        // 취소는 실패가 아니다.
        if (isSubtitleCancelled(err) || abortRef.current) {
          setItem(script.id, { status: 'skipped', message: '중단됨' });
          break;
        }
        setItem(script.id, {
          status: 'failed',
          message: `${err instanceof Error ? err.message : String(err)}`,
        });
        if (!settings.tts_batch_continue_on_error) break;
      }
    }

    // 중단으로 끝났으면 남은 항목을 '건너뜀'으로 마무리한다(대기 상태로 남기지 않는다).
    if (abortRef.current) {
      setQueue((prev) =>
        prev.map((item) =>
          item.status === 'pending' || item.status === 'running'
            ? { ...item, status: 'skipped' as ItemStatus, message: '중단됨' }
            : item,
        ),
      );
    }
    abortControllerRef.current = null;
    setCurrentId(null);
    setPhaseMessage('');
    setProgressPercent(0);
    setIsRunning(false);
  };

  const doneCount = queue.filter((q) => q.status === 'done').length;
  const failedCount = queue.filter((q) => q.status === 'failed').length;
  const overall = queue.length > 0 ? Math.round(((doneCount + failedCount) / queue.length) * 100) : 0;

  return (
    <div className="space-y-4 pb-6">
      {/* 대상 선택 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg space-y-3">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <FileText className="w-4 h-4 text-amber-400" />
            자막을 만들 대본 선택
            <span className="text-[11px] font-semibold text-amber-400">
              {selectedIds.length}개 선택됨
            </span>
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setSelectedIds(allSelected ? [] : recorded.map((s) => s.id))}
              disabled={recorded.length === 0 || isRunning}
              className="text-[11px] font-semibold text-slate-400 hover:text-slate-200 disabled:opacity-40 transition"
            >
              {allSelected ? '전체 해제' : '전체 선택'}
            </button>
          </div>
        </div>

        <p className="text-[11px] text-slate-400 leading-relaxed">
          대본에 연결된 녹음 파일을 원본 대본과 정렬해 <b className="text-slate-300">.srt · .vtt</b>로
          저장합니다. 대본 글자 그대로 자막이 만들어지므로 오타 없는 결과가 나옵니다.
        </p>

        {recorded.length === 0 ? (
          <div className="text-xs text-slate-500 py-6 text-center rounded-xl border border-dashed border-slate-800">
            녹음 파일이 연결된 대본이 없습니다.
            <br />
"대본 & TTS" 탭의 자동 일괄 녹음으로 먼저 녹음하면 여기에 나타납니다.
          </div>
        ) : (
          <div className="max-h-60 overflow-y-auto space-y-1.5 pr-1">
            {recorded.map((script) => {
              const checked = selectedIds.includes(script.id);
              const fileName = script.last_recorded_path?.split(/[\\/]/).pop();
              return (
                <label
                  key={script.id}
                  className={`flex items-center gap-3 px-3 py-2 rounded-xl border cursor-pointer transition ${
                    checked
                      ? 'bg-amber-950/30 border-amber-700/60'
                      : 'bg-slate-950 border-slate-800 hover:border-slate-700'
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={isRunning}
                    onChange={() => toggle(script.id)}
                    className="w-4 h-4 accent-amber-500 shrink-0"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="text-xs font-semibold text-slate-200 truncate">
                      {script.title}
                    </div>
                    <div className="text-[10px] font-mono text-slate-500 truncate">{fileName}</div>
                  </div>
                  <span className="text-[10px] font-mono text-slate-500 shrink-0">
                    {script.char_count.toLocaleString()}자
                  </span>
                </label>
              );
            })}
          </div>
        )}
      </div>

      {/* 실행 */}
      <div className="bg-slate-900/90 border border-slate-800 rounded-2xl p-5 shadow-xl space-y-4">
        <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-bold text-slate-200">
              {isRunning ? '자막 생성 중' : '선택한 대본의 자막을 한 번에 생성'}
            </p>
            <p className="text-[11px] text-slate-400 mt-0.5 truncate">
              {isRunning
                ? phaseMessage || '진행 중...'
                : `싱크 엔진: ${
                    settings.subtitle_sync_engine === 'ai-whisper'
                      ? `로컬 AI Whisper (${settings.subtitle_whisper_model.split('/')[1]})`
                      : '고속 음성 파형 VAD'
                  }`}
            </p>
          </div>

          <div className="flex items-center gap-2 shrink-0">
            {!isRunning ? (
              <button
                onClick={run}
                disabled={selectedIds.length === 0}
                className="flex items-center gap-2 px-6 py-3 rounded-xl bg-gradient-to-r from-amber-600 to-orange-600 hover:from-amber-500 hover:to-orange-500 disabled:from-slate-800 disabled:to-slate-800 disabled:text-slate-500 text-white font-bold text-sm shadow-xl shadow-amber-600/25 active:scale-95 transition"
              >
                <Play className="w-4 h-4 fill-current" />
                <span>{selectedIds.length}개 자막 생성</span>
              </button>
            ) : (
              <button
                onClick={() => {
                  abortRef.current = true;
                  // 진행 중인 항목도 즉시 끊는다(전사·저장이 끝까지 돌지 않는다).
                  abortControllerRef.current?.abort();
                  setPhaseMessage('중단합니다...');
                }}
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
                className="h-full bg-gradient-to-r from-amber-500 to-orange-400 transition-all duration-300"
                style={{ width: `${overall}%` }}
              />
            </div>
            {isRunning && progressPercent > 0 && (
              <div className="h-1 rounded-full bg-slate-950 overflow-hidden">
                <div
                  className="h-full bg-amber-500/60 transition-all duration-200"
                  style={{ width: `${progressPercent}%` }}
                />
              </div>
            )}
            <div className="flex items-center gap-3 text-[11px] font-mono text-slate-400">
              <span className="text-emerald-400">완료 {doneCount}</span>
              <span className="text-red-400">실패 {failedCount}</span>
              <span>전체 {queue.length}</span>
            </div>

            <div className="space-y-1.5 max-h-64 overflow-y-auto pr-1">
              {queue.map((item) => (
                <div
                  key={item.scriptId}
                  className={`flex items-center gap-3 px-3 py-2 rounded-xl border ${
                    item.scriptId === currentId
                      ? 'bg-slate-950 border-amber-700/60'
                      : 'bg-slate-950 border-slate-800'
                  }`}
                >
                  <span className="shrink-0">
                    {item.status === 'done' ? (
                      <Check className="w-4 h-4 text-emerald-400" />
                    ) : item.status === 'failed' ? (
                      <X className="w-4 h-4 text-red-400" />
                    ) : item.scriptId === currentId ? (
                      <Loader className="w-4 h-4 text-amber-400 animate-spin" />
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
                  {item.srtPath && (
                    <button
                      onClick={() => onOpenExplorer(item.srtPath!)}
                      title="자막 파일 폴더 열기"
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

        {errorMsg && (
          <div className="rounded-xl px-3.5 py-2.5 text-xs font-semibold flex items-center gap-2 border bg-red-950/50 border-red-800/60 text-red-300">
            <AlertCircle className="w-3.5 h-3.5" />
            <span>{errorMsg}</span>
          </div>
        )}
      </div>

      {/* 자막 옵션 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl shadow-lg overflow-hidden">
        <button
          onClick={() => setShowAdvanced((v) => !v)}
          className="w-full flex items-center justify-between px-4 py-3 text-xs font-bold text-slate-300 hover:text-slate-100 transition"
        >
          <span className="flex items-center gap-2">
            <SettingsIcon className="w-4 h-4 text-slate-400" />
            자막 생성 옵션
          </span>
          <ChevronDown
            className={`w-4 h-4 transition-transform ${showAdvanced ? 'rotate-180' : ''}`}
          />
        </button>

        {showAdvanced && (
          <div className="px-4 pb-4 space-y-3 border-t border-slate-800 pt-3">
            <p className="text-[10px] text-slate-500">
              자막 생성기 탭과 같은 설정을 씁니다. 여기서 바꾸면 양쪽 모두에 반영됩니다.
            </p>

            <SubtitleOptionsPanel
              settings={settings}
              onUpdateSettings={onUpdateSettings}
              disabled={isRunning}
            />

            <div className="flex flex-wrap items-center gap-4">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={settings.tts_batch_continue_on_error}
                  onChange={(e) =>
                    onUpdateSettings({ tts_batch_continue_on_error: e.target.checked })
                  }
                  disabled={isRunning}
                  className="w-4 h-4 accent-amber-500"
                />
                <span className="text-[11px] font-semibold text-slate-300">
                  실패해도 다음 대본 계속 진행
                </span>
              </label>
              <button
                onClick={onOpenSettings}
                className="text-[11px] font-semibold text-amber-400 hover:text-amber-300 transition ml-auto"
              >
                저장 폴더 설정
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
