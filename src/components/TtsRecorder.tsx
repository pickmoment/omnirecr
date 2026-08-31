import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertCircle,
  Check,
  ChevronRight,
  ClipboardCopy,
  ExternalLink,
  FileText,
  FolderOpen,
  Mic,
  Pause,
  Play,
  Square,
  Timer,
  Wand2,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import type { RecordingStatus, ScriptItem, Settings } from '../types';
import { AudioVisualizer } from './AudioVisualizer';
import { TypecastDiagnosticsLog, TypecastSessionCard } from './TypecastSessionCard';
import { formatDuration, formatTimer } from '../utils/format';
import { splitIntoChunks } from '../utils/scriptChunks';

export interface StartTtsRecordOptions {
  fileNamePrefix?: string;
  settingsOverride?: Partial<Settings>;
  showMiniController?: boolean;
  /** true 면 타임스탬프 없이 fileNamePrefix 그대로를 파일명으로 쓴다(대본 & TTS 녹음 전용). */
  exactFileName?: boolean;
  /** 덮어쓰기 확인을 이미 받았을 때 true. 자동 일괄 녹음이 시작 전에 한 번에 확인한다. */
  skipOverwriteCheck?: boolean;
  /** true 면 실패를 `alert` 대신 예외로 알린다(자동 일괄 녹음이 인라인으로 표시). */
  throwOnError?: boolean;
}

interface TtsRecorderProps {
  settings: Settings;
  scripts: ScriptItem[];
  selectedScript: ScriptItem | null;
  onSelectScript: (id: string | null) => void;
  recordingStatus: RecordingStatus;
  onUpdateSettings: (partial: Partial<Settings>) => Promise<void>;
  onStartRecord: (options: StartTtsRecordOptions) => Promise<string | null>;
  onPauseRecord: () => Promise<void>;
  onResumeRecord: () => Promise<void>;
  onStopRecord: (options?: { silent?: boolean }) => Promise<void>;
  onRefreshScripts: () => Promise<void>;
  onSendToSubtitle: (audioPath: string, scriptText: string) => void;
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenDefaultPlayer: (path: string) => Promise<void>;
  onGoToLibrary: () => void;
}

// 대본을 나눠 보내고 싶을 때 쓰는 선택 옵션(기본은 전체를 한 번에 보낸다).
const CHUNK_PRESETS = [
  { label: '전체 한 번에', value: 0 },
  { label: '3,000자씩', value: 3000 },
  { label: '2,000자씩', value: 2000 },
  { label: '1,000자씩', value: 1000 },
];

/**
 * 녹음이 끝난 순간에 고정되는 결과 스냅샷.
 * 결과 영역의 후속 동작(자막 생성 등)은 "현재 선택된 대본"이 아니라 반드시 이 값을 쓴다.
 * 섞이면 A 를 녹음하고 B 를 선택한 사용자에게 A 오디오 + B 텍스트 자막이 만들어진다.
 */
interface RecordedResult {
  path: string;
  scriptId: string | null;
  scriptTitle: string;
  scriptContent: string;
}

export const TtsRecorder: React.FC<TtsRecorderProps> = ({
  settings,
  scripts,
  selectedScript,
  onSelectScript,
  recordingStatus,
  onUpdateSettings,
  onStartRecord,
  onPauseRecord,
  onResumeRecord,
  onStopRecord,
  onRefreshScripts,
  onSendToSubtitle,
  onOpenExplorer,
  onOpenDefaultPlayer,
  onGoToLibrary,
}) => {
  const [chunkLimit, setChunkLimit] = useState<number>(0);
  const [chunkIndex, setChunkIndex] = useState(0);
  const [countdown, setCountdown] = useState<number | null>(null);
  const [isStarting, setIsStarting] = useState(false);
  const [result, setResult] = useState<RecordedResult | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const isRecording = recordingStatus.status === 'recording';
  const isPaused = recordingStatus.status === 'paused';
  const isStopping = recordingStatus.status === 'stopping';
  const isActive = isRecording || isPaused;

  // 이 탭에서 시작한 녹음인지 표시. 종료 시 결과 파일을 대본에 연결하는 데 쓴다.
  const sessionActiveRef = useRef(false);
  const wasActiveRef = useRef(false);
  // 녹음 시작 시 백엔드가 알려준 저장 경로 (미니 컨트롤러 · 무음 자동 종료로 끝나도 동일)
  const pendingPathRef = useRef<string | null>(null);
  const sessionScriptRef = useRef<ScriptItem | null>(null);
  const flashTimerRef = useRef<number | null>(null);

  const chunks = useMemo(
    () => (selectedScript ? splitIntoChunks(selectedScript.content, chunkLimit) : []),
    [selectedScript, chunkLimit],
  );

  const flash = useCallback((msg: string) => {
    setErrorMsg(null);
    setFeedback(msg);
    if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    flashTimerRef.current = window.setTimeout(() => {
      flashTimerRef.current = null;
      setFeedback((cur) => (cur === msg ? null : cur));
    }, 3000);
  }, []);

  // 탭을 옮겨 이 화면이 사라져도 안내 타이머가 남지 않게 한다.
  useEffect(
    () => () => {
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    setChunkIndex(0);
  }, [selectedScript?.id, chunkLimit]);

  // ── 녹음 종료 감지 → 결과 파일을 대본에 연결 ──────────────
  // 종료 경로가 3가지(이 화면의 종료 버튼 · 미니 컨트롤러 · 무음 자동 종료)라
  // 상태가 idle 로 떨어지는 시점을 공통 신호로 삼는다.
  useEffect(() => {
    if (isActive) {
      wasActiveRef.current = true;
      return;
    }
    // 'stopping' 은 아직 저장 중이므로 idle 이 될 때까지 기다린다.
    if (isStopping) return;
    if (!wasActiveRef.current) return;

    wasActiveRef.current = false;
    if (!sessionActiveRef.current) return;
    sessionActiveRef.current = false;

    const script = sessionScriptRef.current;
    const knownPath = pendingPathRef.current;
    pendingPathRef.current = null;
    sessionScriptRef.current = null;

    (async () => {
      try {
        const path =
          knownPath ?? (await invoke<string | null>('get_last_recorded_path'));
        if (!path) return;
        // 이 시점의 대본을 통째로 박제한다. 이후 선택이 바뀌어도 결과 영역은 흔들리지 않는다.
        setResult({
          path,
          scriptId: script?.id ?? null,
          scriptTitle: script?.title ?? '(대본 정보 없음)',
          scriptContent: script?.content ?? '',
        });
        if (script) {
          await invoke('attach_script_recording', { id: script.id, recordedPath: path });
          await onRefreshScripts();
        }
        flash(
          script
            ? `녹음이 저장되고 "${script.title}" 대본에 연결되었습니다.`
            : '녹음이 저장되었습니다.',
        );
      } catch (err) {
        setErrorMsg(`녹음 결과 연결 실패: ${err}`);
      }
    })();
  }, [isActive, isStopping, onRefreshScripts, flash]);

  // ── 2단계: 대본 보내기 ──────────────────────────────────
  const pushChunk = async (index: number) => {
    const text = chunks[index];
    if (!text) return;
    try {
      // 자동 일괄 녹음과 같은 주입 경로를 쓴다(클립보드 복사도 함께 이뤄진다).
      await invoke('typecast_prepare_script', { text });
      setChunkIndex(index);
      flash(`${index + 1}번째 조각을 Typecast 편집기로 보냈습니다. (클립보드에도 복사됨)`);
    } catch (err) {
      setErrorMsg(`대본 전달 실패: ${err}`);
    }
  };

  const copyChunk = async (index: number) => {
    const text = chunks[index];
    if (!text) return;
    try {
      await invoke('copy_text_to_clipboard', { text });
      setChunkIndex(index);
      flash('클립보드에 복사했습니다. Typecast 편집기에서 붙여넣기(⌘V / Ctrl+V) 하세요.');
    } catch (err) {
      setErrorMsg(`클립보드 복사 실패: ${err}`);
    }
  };

  // ── 3단계: 녹음 ────────────────────────────────────────
  const notifyPage = async (message: string, tone?: string) => {
    try {
      await invoke('notify_typecast', { message, tone: tone ?? 'info' });
    } catch {
      // 창이 닫혀 있으면 무시한다.
    }
  };

  const startRecording = async () => {
    if (!selectedScript) {
      setErrorMsg('먼저 녹음할 대본을 선택하세요.');
      return;
    }
    setErrorMsg(null);
    setResult(null);
    setIsStarting(true);

    try {
      // Typecast 창을 앞으로 올려 재생 버튼을 바로 누를 수 있게 한다.
      await invoke('focus_typecast_browser').catch(() => undefined);

      const total = Math.max(0, settings.tts_countdown_secs);
      for (let remaining = total; remaining > 0; remaining -= 1) {
        setCountdown(remaining);
        await notifyPage(`${remaining}초 후 녹음이 시작됩니다. 재생 버튼에 마우스를 올려두세요.`);
        await new Promise((resolve) => window.setTimeout(resolve, 1000));
      }
      setCountdown(null);

      const outputPath = await onStartRecord({
        fileNamePrefix: selectedScript.title,
        showMiniController: true,
        exactFileName: true,
        settingsOverride: {
          system_audio_enabled: true,
          mic_audio_enabled: settings.tts_mic_enabled,
          // 무음 자동 일시정지 · DSP 필터는 환경 설정 값을 그대로 따른다.
          // 낭독이 끝나면 자동 저장되도록 무음 자동 종료를 켠다.
          auto_stop_enabled: true,
          auto_stop_seconds: settings.tts_auto_stop_seconds,
        },
      });

      if (outputPath !== null) {
        sessionActiveRef.current = true;
        pendingPathRef.current = outputPath;
        sessionScriptRef.current = selectedScript;
        await notifyPage('🔴 녹음 시작 — 지금 Typecast 재생 버튼을 누르세요.', 'rec');
      }
    } finally {
      setCountdown(null);
      setIsStarting(false);
    }
  };

  const stopRecording = async () => {
    await notifyPage('녹음을 종료하고 저장합니다.', 'warn');
    await onStopRecord();
  };

  // ── 렌더 ───────────────────────────────────────────────
  return (
    <div className="space-y-4 pb-6">
      {/* 대본 선택 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg space-y-3">
        <div className="flex items-center justify-between gap-3">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <FileText className="w-4 h-4 text-emerald-400" />
            녹음할 대본
          </span>
          <button
            onClick={onGoToLibrary}
            className="text-[11px] font-semibold text-emerald-400 hover:text-emerald-300 flex items-center gap-1"
          >
            대본 관리로 이동
            <ChevronRight className="w-3 h-3" />
          </button>
        </div>

        {scripts.length === 0 ? (
          <div className="text-xs text-slate-500 py-4 text-center rounded-xl border border-dashed border-slate-800">
            저장된 대본이 없습니다. "대본 관리"에서 먼저 대본을 만들어 주세요.
          </div>
        ) : (
          <>
            <select
              value={selectedScript?.id ?? ''}
              onChange={(e) => onSelectScript(e.target.value || null)}
              disabled={isActive}
              className="w-full px-3 py-2.5 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 focus:outline-none focus:border-emerald-600 disabled:opacity-50"
            >
              <option value="">— 대본을 선택하세요 —</option>
              {scripts.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.title} ({s.char_count.toLocaleString()}자 · ≈
                  {formatDuration(s.estimated_secs)})
                </option>
              ))}
            </select>

            {selectedScript && (
              <div className="rounded-xl bg-slate-950 border border-slate-800 p-3">
                <div className="flex items-center gap-2 text-[10px] font-mono text-slate-500 mb-2">
                  <span>{selectedScript.char_count.toLocaleString()}자</span>
                  <span>·</span>
                  <span>{selectedScript.line_count}줄</span>
                  <span>·</span>
                  <span className="text-emerald-500">
                    예상 낭독 ≈{formatDuration(selectedScript.estimated_secs)}
                  </span>
                </div>
                <p className="text-[11px] text-slate-400 leading-relaxed line-clamp-3 whitespace-pre-wrap">
                  {selectedScript.content}
                </p>
              </div>
            )}
          </>
        )}
      </div>

      {/* 1단계: 로그인 & 접속 (자동 일괄 녹음 화면과 같은 공용 카드) */}
      <TypecastSessionCard
        settings={settings}
        onUpdateSettings={onUpdateSettings}
        onNotice={flash}
        onError={setErrorMsg}
      />

      <TypecastDiagnosticsLog onCopy={flash} />


      {/* 2단계: 대본 보내기 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg space-y-3">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <span className="w-5 h-5 rounded-lg bg-purple-600 text-white text-[10px] font-black flex items-center justify-center">
              2
            </span>
            대본을 Typecast 편집기로 보내기
          </span>
          <div className="flex items-center gap-1.5">
            <span className="text-[11px] text-slate-400 font-semibold">분할</span>
            <select
              value={chunkLimit}
              onChange={(e) => setChunkLimit(Number(e.target.value))}
              className="px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-800 text-[11px] text-slate-200 focus:outline-none focus:border-purple-600"
            >
              {CHUNK_PRESETS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </div>
        </div>

        <p className="text-[11px] text-slate-400 leading-relaxed">
          대본을 클립보드에 복사하고 편집기 자동 입력을 시도합니다. Typecast 화면 구조가 바뀌어 자동 입력이
          실패하더라도 편집기를 클릭한 뒤 <b className="text-slate-300">⌘V / Ctrl+V</b>로 바로 붙여넣을 수
          있습니다. 요금제 글자 수 제한이 있다면 분할 옵션으로 나눠서 조각별로 녹음하세요.
        </p>

        {!selectedScript ? (
          <div className="text-xs text-slate-500 py-3 text-center rounded-xl border border-dashed border-slate-800">
            대본을 먼저 선택하세요.
          </div>
        ) : chunkLimit === 0 ? (
          <div className="flex flex-wrap items-center gap-2">
            <button
              onClick={() => pushChunk(0)}
              className="flex items-center gap-1.5 px-4 py-2.5 rounded-xl bg-purple-600 hover:bg-purple-500 text-white text-xs font-bold shadow-lg shadow-purple-600/25 transition active:scale-95"
            >
              <Wand2 className="w-4 h-4" />
              <span>대본 전체 보내기</span>
            </button>
            <button
              onClick={() => copyChunk(0)}
              className="flex items-center gap-1.5 px-3.5 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition"
            >
              <ClipboardCopy className="w-3.5 h-3.5 text-purple-400" />
              <span>클립보드에 복사만</span>
            </button>
          </div>
        ) : (
          <div className="space-y-2 max-h-56 overflow-y-auto pr-1">
            {chunks.map((chunk, i) => (
              <div
                key={i}
                className={`rounded-xl border p-3 flex items-start gap-3 ${
                  i === chunkIndex
                    ? 'bg-purple-950/30 border-purple-700/60'
                    : 'bg-slate-950 border-slate-800'
                }`}
              >
                <span className="shrink-0 w-6 h-6 rounded-lg bg-slate-800 text-slate-300 text-[10px] font-bold flex items-center justify-center">
                  {i + 1}
                </span>
                <div className="min-w-0 flex-1">
                  <p className="text-[11px] text-slate-400 line-clamp-2 whitespace-pre-wrap">{chunk}</p>
                  <span className="text-[10px] text-slate-600 font-mono">{chunk.length}자</span>
                </div>
                <div className="flex items-center gap-1 shrink-0">
                  <button
                    title="Typecast로 보내기"
                    onClick={() => pushChunk(i)}
                    className="p-2 rounded-lg bg-purple-950/60 hover:bg-purple-900 text-purple-300 border border-purple-800/50 transition"
                  >
                    <Wand2 className="w-3.5 h-3.5" />
                  </button>
                  <button
                    title="클립보드 복사"
                    onClick={() => copyChunk(i)}
                    className="p-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-700 transition"
                  >
                    <ClipboardCopy className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* 3단계: 녹음 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg space-y-3">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <span className="w-5 h-5 rounded-lg bg-indigo-600 text-white text-[10px] font-black flex items-center justify-center">
              3
            </span>
            TTS 재생을 시스템 사운드로 녹음
          </span>
          <span className="text-[10px] font-mono font-bold px-2.5 py-1 rounded-full bg-indigo-950/60 border border-indigo-800/40 text-indigo-300 uppercase">
            {settings.audio_format} • {settings.audio_bitrate} kbps
          </span>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          <label className="rounded-xl bg-slate-950 border border-slate-800 p-3 flex items-center justify-between gap-2">
            <span className="text-[11px] font-semibold text-slate-300 flex items-center gap-1.5">
              <Timer className="w-3.5 h-3.5 text-indigo-400" />
              준비 카운트다운
            </span>
            <select
              value={settings.tts_countdown_secs}
              onChange={(e) => onUpdateSettings({ tts_countdown_secs: Number(e.target.value) })}
              disabled={isActive}
              className="px-2 py-1 rounded-lg bg-slate-900 border border-slate-800 text-[11px] text-slate-200 focus:outline-none disabled:opacity-50"
            >
              {[0, 3, 5, 10].map((v) => (
                <option key={v} value={v}>
                  {v === 0 ? '없음' : `${v}초`}
                </option>
              ))}
            </select>
          </label>

          <label className="rounded-xl bg-slate-950 border border-slate-800 p-3 flex items-center justify-between gap-2 cursor-pointer">
            <span className="text-[11px] font-semibold text-slate-300 flex items-center gap-1.5">
              <Mic className="w-3.5 h-3.5 text-indigo-400" />
              마이크도 함께 녹음
            </span>
            <input
              type="checkbox"
              checked={settings.tts_mic_enabled}
              onChange={(e) => onUpdateSettings({ tts_mic_enabled: e.target.checked })}
              disabled={isActive}
              className="w-4 h-4 accent-indigo-500"
            />
          </label>

          <div className="rounded-xl bg-slate-950 border border-slate-800 p-3 flex items-center justify-between gap-2">
            <span className="text-[11px] font-semibold text-slate-300 flex items-center gap-1.5">
              <Square className="w-3.5 h-3.5 text-indigo-400" />
              낭독 종료 판정 무음
            </span>
            <select
              value={settings.tts_auto_stop_seconds}
              onChange={(e) => onUpdateSettings({ tts_auto_stop_seconds: Number(e.target.value) })}
              disabled={isActive}
              className="px-2 py-1 rounded-lg bg-slate-900 border border-slate-800 text-[11px] text-slate-200 focus:outline-none disabled:opacity-50"
            >
              {[3, 4, 5, 8, 10].map((v) => (
                <option key={v} value={v}>
                  {v}초
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="rounded-xl bg-amber-950/25 border border-amber-900/40 p-3 flex gap-2.5">
            <AlertCircle className="w-4 h-4 text-amber-400 shrink-0 mt-0.5" />
            <p className="text-[11px] text-slate-300 leading-relaxed">
            무음이 {settings.tts_auto_stop_seconds}초 이어지면 녹음이 자동 종료됩니다. 녹음 시작 후 재생
            버튼을 그보다 늦게 누르면 낭독 전에 멈출 수 있으니, 카운트다운이 끝나면 바로 재생하세요.
          </p>
        </div>

        <AudioVisualizer
          sysLevelDb={recordingStatus.sys_vu_level}
          micLevelDb={recordingStatus.mic_vu_level}
          isRecording={isActive}
          systemAudioEnabled
          micAudioEnabled={settings.tts_mic_enabled}
        />

        <div className="flex flex-col md:flex-row items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <div
              className={`w-12 h-12 rounded-2xl flex items-center justify-center border ${
                isRecording
                  ? 'bg-red-950/60 border-red-500/50 text-red-400 animate-pulse'
                  : isPaused
                    ? 'bg-amber-950/60 border-amber-500/50 text-amber-400'
                    : 'bg-slate-950 border-slate-800 text-slate-500'
              }`}
            >
              {countdown !== null ? (
                <span className="text-xl font-black">{countdown}</span>
              ) : (
                <Timer className="w-6 h-6" />
              )}
            </div>
            <div>
              <div className="text-2xl font-mono font-extrabold tracking-wider text-white">
                {formatTimer(recordingStatus.duration_secs)}
              </div>
              <p className="text-[11px] text-slate-400">
                {countdown !== null
                  ? `${countdown}초 후 시작 — Typecast 재생 버튼 준비`
                  : isRecording
                    ? 'TTS 낭독 녹음 중 (미니 컨트롤러로도 종료 가능)'
                    : isPaused
                      ? '일시정지됨'
                      : '대기 중'}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            {!isActive ? (
              <button
                onClick={startRecording}
                disabled={!selectedScript || isStarting || isStopping}
                className="flex items-center gap-2 px-7 py-3.5 rounded-xl bg-gradient-to-r from-indigo-600 to-cyan-600 hover:from-indigo-500 hover:to-cyan-500 disabled:from-slate-800 disabled:to-slate-800 disabled:text-slate-500 text-white font-bold text-sm shadow-xl shadow-indigo-600/25 active:scale-95 transition"
              >
                <Play className="w-5 h-5 fill-current" />
                <span>{isStarting ? '준비 중...' : '녹음 시작'}</span>
              </button>
            ) : (
              <>
                {isRecording ? (
                  <button
                    onClick={onPauseRecord}
                    className="flex items-center gap-2 px-4 py-3.5 rounded-xl bg-amber-600 hover:bg-amber-500 text-white font-bold text-xs shadow-lg transition"
                  >
                    <Pause className="w-4 h-4 fill-current" />
                    <span>일시정지</span>
                  </button>
                ) : (
                  <button
                    onClick={onResumeRecord}
                    className="flex items-center gap-2 px-4 py-3.5 rounded-xl bg-emerald-600 hover:bg-emerald-500 text-white font-bold text-xs shadow-lg transition"
                  >
                    <Play className="w-4 h-4 fill-current" />
                    <span>재개</span>
                  </button>
                )}
                <button
                  onClick={stopRecording}
                  disabled={isStopping}
                  className="flex items-center gap-2 px-6 py-3.5 rounded-xl bg-red-600 hover:bg-red-500 text-white font-bold text-xs shadow-xl shadow-red-600/25 active:scale-95 transition"
                >
                  <Square className="w-4 h-4 fill-current" />
                  <span>{isStopping ? '저장 중...' : '녹음 종료 및 저장'}</span>
                </button>
              </>
            )}
          </div>
        </div>
      </div>

      {/* 4단계: 결과 */}
      {result && (
        <div className="bg-emerald-950/25 border border-emerald-800/50 rounded-2xl p-4 shadow-lg space-y-3">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="w-5 h-5 rounded-lg bg-emerald-600 text-white text-[10px] font-black flex items-center justify-center">
              4
            </span>
            <span className="text-sm font-bold text-emerald-200">녹음 완료</span>
            <span className="text-[11px] font-semibold px-2 py-0.5 rounded-lg bg-slate-900 border border-slate-700 text-slate-300 max-w-full truncate">
              대본: {result.scriptTitle}
            </span>
            {result.scriptId !== null && selectedScript?.id !== result.scriptId && (
              <span className="text-[11px] font-semibold px-2 py-0.5 rounded-lg bg-amber-950/60 border border-amber-700/50 text-amber-300">
                현재 선택된 대본과 다릅니다 — 아래 동작은 녹음한 대본을 씁니다
              </span>
            )}
          </div>
          <p className="text-[11px] text-slate-300 font-mono break-all">{result.path}</p>
          <div className="flex flex-wrap items-center gap-2">
            <button
              onClick={() => onSendToSubtitle(result.path, result.scriptContent)}
              disabled={!result.scriptContent.trim()}
              title={`"${result.scriptTitle}" 본문으로 자막을 만듭니다`}
              className="flex items-center gap-1.5 px-4 py-2.5 rounded-xl bg-amber-600 hover:bg-amber-500 disabled:bg-slate-800 disabled:text-slate-500 text-white text-xs font-bold shadow-lg shadow-amber-600/20 transition active:scale-95"
            >
              <FileText className="w-3.5 h-3.5" />
              <span>이 대본으로 자막 만들기</span>
            </button>
            <button
              onClick={() => onOpenDefaultPlayer(result.path)}
              className="flex items-center gap-1.5 px-3.5 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition"
            >
              <ExternalLink className="w-3.5 h-3.5 text-emerald-400" />
              <span>재생</span>
            </button>
            <button
              onClick={() => onOpenExplorer(result.path)}
              className="flex items-center gap-1.5 px-3.5 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition"
            >
              <FolderOpen className="w-3.5 h-3.5 text-emerald-400" />
              <span>폴더 열기</span>
            </button>
          </div>
        </div>
      )}

      {(feedback || errorMsg) && (
        <div
          className={`rounded-xl px-3.5 py-2.5 text-xs font-semibold flex items-center gap-2 border ${
            errorMsg
              ? 'bg-red-950/50 border-red-800/60 text-red-300'
              : 'bg-emerald-950/50 border-emerald-800/60 text-emerald-300'
          }`}
        >
          {errorMsg ? <AlertCircle className="w-3.5 h-3.5" /> : <Check className="w-3.5 h-3.5" />}
          <span>{errorMsg || feedback}</span>
        </div>
      )}
    </div>
  );
};
