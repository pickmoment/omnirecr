import React, { useState, useEffect } from 'react';
import {
  X,
  Folder,
  CheckCircle2,
  Sliders,
  Sparkles,
  Timer,
  Terminal,
  VolumeX,
  Waves,
  PauseCircle,
  StopCircle,
} from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import type { Settings } from '../types';

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  settings: Settings;
  onSave: (settings: Settings) => Promise<void>;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  onClose,
  settings,
  onSave,
}) => {
  const [localSettings, setLocalSettings] = useState<Settings>(settings);
  const [isSaved, setIsSaved] = useState(false);
  const [ffmpegTestMsg, setFfmpegTestMsg] = useState<string | null>(null);

  // Sync props to local state when modal opens
  useEffect(() => {
    if (isOpen) {
      setLocalSettings(settings);
      setFfmpegTestMsg(null);
    }
  }, [isOpen, settings]);

  if (!isOpen) return null;

  const handleSelectFolder = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        defaultPath: localSettings.output_dir,
      });

      if (selected && typeof selected === 'string') {
        const updated = { ...localSettings, output_dir: selected };
        setLocalSettings(updated);
        onSave(updated);
        showSavedToast();
      }
    } catch (err) {
      console.error(err);
    }
  };

  const handleChange = (partial: Partial<Settings>) => {
    const updated = { ...localSettings, ...partial };
    setLocalSettings(updated);
    onSave(updated);
    showSavedToast();
  };

  const showSavedToast = () => {
    setIsSaved(true);
    setTimeout(() => setIsSaved(false), 2000);
  };

  const handleTestFfmpeg = async () => {
    try {
      const res = await invoke<string>('check_ffmpeg_status', {
        customFfmpegPath: localSettings.custom_ffmpeg_path || null,
      });
      setFfmpegTestMsg(`✅ 감지 성공: ${res}`);
    } catch (err) {
      setFfmpegTestMsg(`❌ 감지 실패: ${err}`);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/70 backdrop-blur-sm flex items-center justify-center p-4">
      <div className="bg-slate-900 border border-slate-800 rounded-3xl w-full max-w-2xl shadow-2xl overflow-hidden flex flex-col max-h-[90vh]">
        {/* Header */}
        <div className="px-6 py-4 border-b border-slate-800 flex items-center justify-between bg-slate-950/60 shrink-0">
          <div className="flex items-center gap-2.5">
            <div className="w-8 h-8 rounded-xl bg-blue-600/20 border border-blue-500/30 text-blue-400 flex items-center justify-center">
              <Sliders className="w-4 h-4" />
            </div>
            <div>
              <h2 className="font-bold text-sm text-white">환경 설정 (Settings)</h2>
              <p className="text-[11px] text-slate-400">화면 녹화 및 오디오 녹음 전체에 공통 적용됩니다 (~/.omnirec/settings.json)</p>
            </div>
          </div>

          <div className="flex items-center gap-3">
            {isSaved && (
              <span className="flex items-center gap-1 text-xs text-emerald-400 font-medium animate-fade-in">
                <CheckCircle2 className="w-3.5 h-3.5" />
                자동 저장됨
              </span>
            )}
            <button
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800 transition"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Form Body */}
        <div className="p-6 space-y-6 overflow-y-auto flex-1">
          {/* Section 1: Smart Noise Filters */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <span className="text-xs font-bold text-slate-200 flex items-center gap-2">
                <Sparkles className="w-4 h-4 text-emerald-400" />
                <span>🛡️ 스마트 노이즈 필터 (녹화 & 녹음 공통 적용)</span>
              </span>
              <span className="text-[11px] text-emerald-400/80 font-mono">Real-time DSP</span>
            </div>

            <div className="space-y-2.5">
              {/* Windows Notification Sound Auto Mute */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 flex items-center justify-between hover:bg-slate-950/90 transition">
                <div className="flex items-center gap-2.5">
                  <div className="w-8 h-8 rounded-lg bg-amber-500/10 border border-amber-500/20 text-amber-400 flex items-center justify-center shrink-0">
                    <VolumeX className="w-4 h-4" />
                  </div>
                  <div>
                    <div className="text-xs font-bold text-slate-200">Windows 시스템 알림음 자동 음소거</div>
                    <div className="text-[11px] text-slate-400">녹화/녹음 중 윈도우 팝업/경고 알림음을 백그라운드 자동 차단 및 종료 시 자동 복구</div>
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={localSettings.mute_notifications}
                  onChange={(e) => handleChange({ mute_notifications: e.target.checked })}
                  className="w-4 h-4 rounded bg-slate-800 border-slate-700 text-emerald-500 focus:ring-0 cursor-pointer"
                />
              </div>

              {/* Smart Noise Gate */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2.5">
                    <div className="w-8 h-8 rounded-lg bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 flex items-center justify-center shrink-0">
                      <Sparkles className="w-4 h-4" />
                    </div>
                    <div>
                      <div className="text-xs font-bold text-slate-200">스마트 노이즈 게이트 (Noise Gate)</div>
                      <div className="text-[11px] text-slate-400">미세한 PC 팬 소음 및 화이트 노이즈 실시간 컷오프</div>
                    </div>
                  </div>
                  <input
                    type="checkbox"
                    checked={localSettings.noise_gate_enabled}
                    onChange={(e) => handleChange({ noise_gate_enabled: e.target.checked })}
                    className="w-4 h-4 rounded bg-slate-800 border-slate-700 text-emerald-500 focus:ring-0 cursor-pointer"
                  />
                </div>

                {localSettings.noise_gate_enabled && (
                  <div className="pt-2 border-t border-slate-900 space-y-1.5">
                    <div className="flex items-center justify-between text-xs">
                      <span className="text-slate-400">노이즈 감도 임계값 (Threshold)</span>
                      <span className="font-mono font-bold text-emerald-400">
                        {localSettings.noise_gate_threshold_db} dB
                      </span>
                    </div>
                    <input
                      type="range"
                      min="-60"
                      max="-20"
                      step="1"
                      value={localSettings.noise_gate_threshold_db}
                      onChange={(e) => handleChange({ noise_gate_threshold_db: parseFloat(e.target.value) })}
                      className="w-full h-1.5 bg-slate-800 rounded-lg appearance-none cursor-pointer accent-emerald-400"
                    />
                    <p className="text-[10px] text-slate-500">
                      -45dB 권장 (낮을수록 작은 소리 통과, 높을수록 강하게 잡음 차단)
                    </p>
                  </div>
                )}
              </div>

              {/* 80Hz Low-cut Filter */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 flex items-center justify-between hover:bg-slate-950/90 transition">
                <div className="flex items-center gap-2.5">
                  <div className="w-8 h-8 rounded-lg bg-teal-500/10 border border-teal-500/20 text-teal-400 flex items-center justify-center shrink-0">
                    <Waves className="w-4 h-4" />
                  </div>
                  <div>
                    <div className="text-xs font-bold text-slate-200">80Hz 하이패스 (Low-cut) 필터</div>
                    <div className="text-[11px] text-slate-400">책상 진동 및 웅웅거리는 저음역 잡음(Butterworth 2차 IIR) 제거</div>
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={localSettings.highpass_filter_enabled}
                  onChange={(e) => handleChange({ highpass_filter_enabled: e.target.checked })}
                  className="w-4 h-4 rounded bg-slate-800 border-slate-700 text-teal-500 focus:ring-0 cursor-pointer"
                />
              </div>
            </div>
          </div>

          {/* Section 2: Silence Automation & Smart Control */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <span className="text-xs font-bold text-slate-200 flex items-center gap-2">
                <Timer className="w-4 h-4 text-cyan-400" />
                <span>⏱️ 무음 자동화 & 스마트 제어 (Silence Automation)</span>
              </span>
              <span className="text-[11px] text-cyan-400/80 font-mono">Auto-Engine</span>
            </div>

            <div className="space-y-2.5">
              {/* Auto Pause */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2.5">
                    <div className="w-8 h-8 rounded-lg bg-blue-500/10 border border-blue-500/20 text-blue-400 flex items-center justify-center shrink-0">
                      <PauseCircle className="w-4 h-4" />
                    </div>
                    <div>
                      <div className="text-xs font-bold text-slate-200">무음 구간 자동 일시정지 & 재개 (Auto-Pause / Resume)</div>
                      <div className="text-[11px] text-slate-400">말이 없으면 인코딩을 일시 중지하고, 소리가 나면 즉시 자동 재개</div>
                    </div>
                  </div>
                  <input
                    type="checkbox"
                    checked={localSettings.auto_pause_enabled}
                    onChange={(e) => handleChange({ auto_pause_enabled: e.target.checked })}
                    className="w-4 h-4 rounded bg-slate-800 border-slate-700 text-blue-500 focus:ring-0 cursor-pointer"
                  />
                </div>

                {localSettings.auto_pause_enabled && (
                  <div className="pt-2 border-t border-slate-900 space-y-1.5">
                    <div className="flex items-center justify-between text-xs">
                      <span className="text-slate-400">일시정지 대기 시간</span>
                      <span className="font-mono font-bold text-blue-400">
                        {localSettings.auto_pause_seconds.toFixed(1)}초 (기본 1.0초)
                      </span>
                    </div>
                    <input
                      type="range"
                      min="0.5"
                      max="5.0"
                      step="0.5"
                      value={localSettings.auto_pause_seconds}
                      onChange={(e) => handleChange({ auto_pause_seconds: parseFloat(e.target.value) })}
                      className="w-full h-1.5 bg-slate-800 rounded-lg appearance-none cursor-pointer accent-blue-400"
                    />
                  </div>
                )}
              </div>

              {/* Auto Stop */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2.5">
                    <div className="w-8 h-8 rounded-lg bg-red-500/10 border border-red-500/20 text-red-400 flex items-center justify-center shrink-0">
                      <StopCircle className="w-4 h-4" />
                    </div>
                    <div>
                      <div className="text-xs font-bold text-slate-200">무음 지속 시 자동 종료 및 저장 (Auto-Stop)</div>
                      <div className="text-[11px] text-slate-400">설정된 시간 동안 무음이 지속되면 안전하게 파일을 저장하고 녹화 자동 종료</div>
                    </div>
                  </div>
                  <input
                    type="checkbox"
                    checked={localSettings.auto_stop_enabled}
                    onChange={(e) => handleChange({ auto_stop_enabled: e.target.checked })}
                    className="w-4 h-4 rounded bg-slate-800 border-slate-700 text-red-500 focus:ring-0 cursor-pointer"
                  />
                </div>

                {localSettings.auto_stop_enabled && (
                  <div className="pt-2 border-t border-slate-900 space-y-1.5">
                    <div className="flex items-center justify-between text-xs">
                      <span className="text-slate-400">자동 종료 대기 시간</span>
                      <span className="font-mono font-bold text-red-400">
                        {localSettings.auto_stop_seconds.toFixed(1)}초 (기본 5.0초)
                      </span>
                    </div>
                    <input
                      type="range"
                      min="2.0"
                      max="30.0"
                      step="1.0"
                      value={localSettings.auto_stop_seconds}
                      onChange={(e) => handleChange({ auto_stop_seconds: parseFloat(e.target.value) })}
                      className="w-full h-1.5 bg-slate-800 rounded-lg appearance-none cursor-pointer accent-red-400"
                    />
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Section 3: Storage & FFmpeg System */}
          <div className="space-y-3">
            <span className="text-xs font-bold text-slate-200 flex items-center gap-2">
              <Folder className="w-4 h-4 text-amber-400" />
              <span>📁 저장 경로 & FFmpeg 엔진</span>
            </span>

            <div className="space-y-3">
              {/* Storage Directory */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 space-y-2">
                <label className="text-xs font-semibold text-slate-300">기본 저장 폴더</label>
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    readOnly
                    value={localSettings.output_dir}
                    className="flex-1 px-3.5 py-2 bg-slate-950 border border-slate-800 rounded-xl text-xs font-mono text-slate-300 focus:outline-none"
                  />
                  <button
                    onClick={handleSelectFolder}
                    className="px-4 py-2 bg-blue-600 hover:bg-blue-500 text-white text-xs font-bold rounded-xl shadow transition"
                  >
                    폴더 변경
                  </button>
                </div>
              </div>

              {/* Custom FFmpeg binary path */}
              <div className="p-3.5 rounded-2xl bg-slate-950/60 border border-slate-800/80 space-y-2">
                <label className="text-xs font-semibold text-slate-300 flex items-center gap-1.5">
                  <Terminal className="w-3.5 h-3.5 text-indigo-400" />
                  <span>사용자 지정 FFmpeg 경로 (선택 사항)</span>
                </label>
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    placeholder="자동 감지 사용 (비워두면 자동 탐색)"
                    value={localSettings.custom_ffmpeg_path || ''}
                    onChange={(e) => handleChange({ custom_ffmpeg_path: e.target.value.trim() || null })}
                    className="flex-1 px-3.5 py-2 bg-slate-950 border border-slate-800 rounded-xl text-xs font-mono text-slate-300 focus:outline-none"
                  />
                  <button
                    onClick={handleTestFfmpeg}
                    className="px-3.5 py-2 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-bold rounded-xl transition"
                  >
                    경로 테스트
                  </button>
                </div>
                {ffmpegTestMsg && <p className="text-[11px] font-mono text-slate-400">{ffmpegTestMsg}</p>}
              </div>
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="px-6 py-4 border-t border-slate-800 bg-slate-950/60 flex items-center justify-between shrink-0">
          <span className="text-[11px] text-slate-400 font-mono">
            변경 사항은 실시간으로 저장되며 녹음/녹화 시 즉시 반영됩니다.
          </span>
          <button
            onClick={onClose}
            className="px-6 py-2 bg-blue-600 hover:bg-blue-500 text-white text-xs font-bold rounded-xl shadow transition"
          >
            확인 및 닫기
          </button>
        </div>
      </div>
    </div>
  );
};
