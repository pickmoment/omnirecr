import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Square, Pause, Play, GripVertical, HardDrive } from 'lucide-react';
import type { RecordingStatus, AudioVUMeterPayload } from '../types';
import { formatFileSize, formatTimer } from '../utils/format';

export const MiniController: React.FC = () => {
  const [status, setStatus] = useState<RecordingStatus>({
    status: 'recording',
    mode: 'screen',
    duration_secs: 0,
    size_bytes: 0,
    is_auto_paused: false,
    output_file: null,
    sys_vu_level: -60,
    mic_vu_level: -60,
  });

  useEffect(() => {
    // Initial fetch
    invoke<RecordingStatus>('get_recording_status')
      .then((s) => setStatus(s))
      .catch((err) => console.error(err));

    const unlistenStatus = listen<RecordingStatus>('recording_status_change', (event) => {
      setStatus(event.payload);
    });

    const unlistenVu = listen<AudioVUMeterPayload>('audio_vu_meter', (event) => {
      setStatus((prev) => ({
        ...prev,
        sys_vu_level: event.payload.sys_level_db,
        mic_vu_level: event.payload.mic_level_db,
        duration_secs: event.payload.duration_secs,
        size_bytes: event.payload.size_bytes,
      }));
    });

    return () => {
      unlistenStatus.then((u) => u());
      unlistenVu.then((u) => u());
    };
  }, []);

  const getLiveSize = () => {
    if (status.status !== 'recording' && status.status !== 'paused') return 0;
    const estimatedBitrateBps = 3500000 / 8;
    const estimatedBytes = Math.floor(estimatedBitrateBps * status.duration_secs);
    return Math.max(status.size_bytes, estimatedBytes);
  };

  const isPaused = status.status === 'paused';
  const isAudio = status.mode === 'audio';
  const actionNoun = isAudio ? '녹음' : '녹화';

  const handleTogglePause = async () => {
    try {
      await invoke('toggle_pause_record');
    } catch (err) {
      console.error(err);
    }
  };

  const handleStop = async () => {
    try {
      await invoke('stop_record');
    } catch (err) {
      console.error(err);
    }
  };

  return (
    <div
      data-tauri-drag-region
      className="fixed inset-0 flex items-center justify-between px-3.5 py-2 bg-slate-900/95 border border-slate-700/80 rounded-2xl shadow-2xl backdrop-blur-lg select-none cursor-move text-slate-100"
      style={{ width: '100%', height: '100%' }}
    >
      {/* Left: Drag Grip & Pulsing Record Dot */}
      <div data-tauri-drag-region className="flex items-center gap-2">
        <GripVertical className="w-4 h-4 text-slate-500 hover:text-slate-300 transition" />
        <div className="flex items-center gap-1.5">
          <div
            className={`w-2.5 h-2.5 rounded-full ${
              isPaused ? 'bg-amber-500' : 'bg-red-500 animate-ping'
            }`}
          />
          <span className="font-mono text-sm font-extrabold tracking-wider text-white">
            {formatTimer(status.duration_secs)}
          </span>
        </div>
      </div>

      {/* Center: Status Badge & Hotkey Hint */}
      <div data-tauri-drag-region className="flex items-center gap-2 text-[10px] text-slate-400 font-mono text-center">
        {getLiveSize() > 0 && (
          <span className="flex items-center gap-1 px-1.5 py-0.5 rounded bg-slate-800/90 border border-slate-700 text-slate-300 font-bold">
            <HardDrive className="w-2.5 h-2.5 text-slate-400" />
            {formatFileSize(getLiveSize())}
          </span>
        )}
        {isPaused ? (
          <span className="text-amber-400 font-bold">일시정지됨</span>
        ) : (
          <span className="text-slate-300">REC (F9: 종료)</span>
        )}
      </div>

      {/* Right: Pause & Stop Action Buttons */}
      <div className="flex items-center gap-1.5 shrink-0">
        {/* Pause / Resume Button */}
        <button
          onClick={handleTogglePause}
          title={isPaused ? `${actionNoun} 재개 (F10)` : '일시정지 (F10)'}
          className={`p-2 rounded-xl border text-xs font-bold transition ${
            isPaused
              ? 'bg-emerald-600 border-emerald-500 text-white shadow-md'
              : 'bg-slate-800 hover:bg-slate-700 border-slate-700 text-amber-400'
          }`}
        >
          {isPaused ? <Play className="w-3.5 h-3.5 fill-current" /> : <Pause className="w-3.5 h-3.5 fill-current" />}
        </button>

        {/* Big Stop Button */}
        <button
          onClick={handleStop}
          title={`${actionNoun} 종료 및 저장 (F9)`}
          className="flex items-center gap-1.5 px-3 py-2 bg-red-600 hover:bg-red-500 text-white text-xs font-bold rounded-xl shadow-lg shadow-red-600/40 active:scale-95 transition"
        >
          <Square className="w-3.5 h-3.5 fill-current" />
          <span>{actionNoun} 종료</span>
        </button>
      </div>
    </div>
  );
};
