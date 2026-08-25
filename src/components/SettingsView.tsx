import React, { useState, useEffect } from 'react';
import {
  Sliders,
  Folder,
  Terminal,
  Volume2,
  Mic,
  Sparkles,
  VolumeX,
  Waves,
  PauseCircle,
  StopCircle,
  CheckCircle2,
  Monitor,
  Music,
  Radio,
  Layers,
} from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import type { Settings, AudioFormat } from '../types';

interface SettingsViewProps {
  settings: Settings;
  onUpdateSettings: (settings: Partial<Settings>) => Promise<void>;
  ffmpegDetected: boolean;
  onRefreshFfmpeg: (customPath?: string | null) => void;
}

export const SettingsView: React.FC<SettingsViewProps> = ({
  settings,
  onUpdateSettings,
  ffmpegDetected,
  onRefreshFfmpeg,
}) => {
  const [localSettings, setLocalSettings] = useState<Settings>(settings);
  const [isSaved, setIsSaved] = useState(false);
  const [ffmpegTestMsg, setFfmpegTestMsg] = useState<string | null>(null);

  useEffect(() => {
    setLocalSettings(settings);
  }, [settings]);

  const showSavedToast = () => {
    setIsSaved(true);
    setTimeout(() => setIsSaved(false), 2000);
  };

  const handleChange = (partial: Partial<Settings>) => {
    const updated = { ...localSettings, ...partial };
    setLocalSettings(updated);
    onUpdateSettings(partial);
    showSavedToast();
  };

  const handleSelectFolder = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        defaultPath: localSettings.output_dir,
      });

      if (selected && typeof selected === 'string') {
        handleChange({ output_dir: selected });
      }
    } catch (err) {
      console.error(err);
    }
  };

  const handleTestFfmpeg = async () => {
    try {
      const res = await invoke<string>('check_ffmpeg_status', {
        customFfmpegPath: localSettings.custom_ffmpeg_path || null,
      });
      setFfmpegTestMsg(`✅ 감지 성공: ${res}`);
      onRefreshFfmpeg(localSettings.custom_ffmpeg_path);
    } catch (err) {
      setFfmpegTestMsg(`❌ 감지 실패: ${err}`);
      onRefreshFfmpeg(localSettings.custom_ffmpeg_path);
    }
  };

  return (
    <div className="min-h-full flex flex-col p-6 space-y-6 max-w-5xl mx-auto pb-10">
      {/* Header Banner */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg flex items-center justify-between">
        <div className="flex items-center gap-3.5">
          <div className="w-12 h-12 rounded-2xl bg-blue-600/20 border border-blue-500/30 text-blue-400 flex items-center justify-center shadow-lg shadow-blue-500/10">
            <Sliders className="w-6 h-6" />
          </div>
          <div>
            <h1 className="text-lg font-extrabold text-white flex items-center gap-2">
              통합 환경 설정 (Integrated Settings)
            </h1>
            <p className="text-xs text-slate-400 mt-0.5">
              화면 녹화, 오디오 녹음 및 미디어 처리에 적용되는 모든 공동 설정을 한곳에서 제어합니다.
            </p>
          </div>
        </div>

        {isSaved && (
          <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-emerald-950/60 border border-emerald-500/40 text-emerald-300 text-xs font-semibold animate-fade-in">
            <CheckCircle2 className="w-4 h-4" />
            <span>자동 저장됨</span>
          </div>
        )}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
        {/* Section 1: 공통 오디오 믹서 (시스템 & 마이크) */}
        <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-4">
          <div className="flex items-center justify-between">
            <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
              <Volume2 className="w-4 h-4 text-cyan-400" />
              공통 오디오 믹서 (Mixer & Volume)
            </span>
            <span className="text-xs text-cyan-400 font-mono">0% ~ 200% Gain</span>
          </div>

          <div className="space-y-4">
            {/* System Audio */}
            <div className="p-3.5 bg-slate-950/60 rounded-xl border border-slate-800 space-y-2">
              <div className="flex items-center justify-between">
                <label className="flex items-center gap-2 text-xs font-semibold text-slate-300 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={localSettings.system_audio_enabled}
                    onChange={(e) => handleChange({ system_audio_enabled: e.target.checked })}
                    className="rounded bg-slate-800 border-slate-700 text-blue-500 focus:ring-0 cursor-pointer"
                  />
                  <Volume2 className="w-4 h-4 text-blue-400" />
                  <span>시스템 사운드 (WASAPI / CoreAudio)</span>
                </label>
                <span className="text-xs font-mono font-bold text-blue-400">
                  {Math.round(localSettings.system_audio_volume * 100)}%
                </span>
              </div>
              <input
                type="range"
                min="0"
                max="2"
                step="0.05"
                value={localSettings.system_audio_volume}
                onChange={(e) => handleChange({ system_audio_volume: parseFloat(e.target.value) })}
                className="w-full h-1.5 bg-slate-800 rounded-lg appearance-none cursor-pointer accent-blue-500"
              />
            </div>

            {/* Mic Audio */}
            <div className="p-3.5 bg-slate-950/60 rounded-xl border border-slate-800 space-y-2">
              <div className="flex items-center justify-between">
                <label className="flex items-center gap-2 text-xs font-semibold text-slate-300 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={localSettings.mic_audio_enabled}
                    onChange={(e) => handleChange({ mic_audio_enabled: e.target.checked })}
                    className="rounded bg-slate-800 border-slate-700 text-indigo-500 focus:ring-0 cursor-pointer"
                  />
                  <Mic className="w-4 h-4 text-indigo-400" />
                  <span>마이크 입력 (Microphone)</span>
                </label>
                <span className="text-xs font-mono font-bold text-indigo-400">
                  {Math.round(localSettings.mic_audio_volume * 100)}%
                </span>
              </div>
              <input
                type="range"
                min="0"
                max="2"
                step="0.05"
                value={localSettings.mic_audio_volume}
                onChange={(e) => handleChange({ mic_audio_volume: parseFloat(e.target.value) })}
                className="w-full h-1.5 bg-slate-800 rounded-lg appearance-none cursor-pointer accent-indigo-500"
              />
            </div>

            {/* Sample Rate */}
            <div className="p-3.5 bg-slate-950/60 rounded-xl border border-slate-800 flex items-center justify-between">
              <span className="text-xs font-semibold text-slate-300">오디오 샘플레이트 (Sample Rate)</span>
              <div className="flex items-center gap-1.5">
                {[44100, 48000].map((sr) => (
                  <button
                    key={sr}
                    type="button"
                    onClick={() => handleChange({ audio_sample_rate: sr })}
                    className={`px-3 py-1 rounded-lg border text-xs font-mono font-bold transition ${
                      localSettings.audio_sample_rate === sr
                        ? 'bg-blue-600 border-blue-500 text-white'
                        : 'bg-slate-900 border-slate-800 text-slate-400 hover:text-slate-200'
                    }`}
                  >
                    {sr / 1000} kHz
                  </button>
                ))}
              </div>
            </div>
          </div>
        </div>

        {/* Section 2: 스마트 노이즈 DSP 필터 */}
        <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-4">
          <div className="flex items-center justify-between">
            <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
              <Sparkles className="w-4 h-4 text-emerald-400" />
              스마트 노이즈 필터 (Real-time DSP)
            </span>
            <span className="text-xs text-emerald-400 font-mono">IIR & Gate</span>
          </div>

          <div className="space-y-3">
            {/* Windows Notification Mute */}
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <div className="w-7 h-7 rounded-lg bg-amber-500/10 border border-amber-500/20 text-amber-400 flex items-center justify-center shrink-0">
                  <VolumeX className="w-3.5 h-3.5" />
                </div>
                <div>
                  <div className="text-xs font-bold text-slate-200">시스템 알림음 자동 차단 (Auto-Mute Notifications)</div>
                  <div className="text-[10px] text-slate-400">녹화/녹음 중 시스템 팝업 및 경고 알림음 자동 음소거 및 종료 후 복구</div>
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
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2.5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-7 h-7 rounded-lg bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 flex items-center justify-center shrink-0">
                    <Sparkles className="w-3.5 h-3.5" />
                  </div>
                  <div>
                    <div className="text-xs font-bold text-slate-200">스마트 노이즈 게이트 (Noise Gate)</div>
                    <div className="text-[10px] text-slate-400">팬 소음 및 화이트 노이즈 실시간 컷오프</div>
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
                <div className="pt-2 border-t border-slate-900 space-y-1">
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-slate-400 text-[11px]">노이즈 감도 임계값</span>
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
                </div>
              )}
            </div>

            {/* 80Hz Low-cut Filter */}
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <div className="w-7 h-7 rounded-lg bg-teal-500/10 border border-teal-500/20 text-teal-400 flex items-center justify-center shrink-0">
                  <Waves className="w-3.5 h-3.5" />
                </div>
                <div>
                  <div className="text-xs font-bold text-slate-200">80Hz 하이패스 (Low-cut) 필터</div>
                  <div className="text-[10px] text-slate-400">책상 진동 및 웅웅거리는 저음역 잡음(2차 IIR) 제거</div>
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

        {/* Section 3: 무음 감지 스마트 자동화 */}
        <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-4">
          <div className="flex items-center justify-between">
            <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
              <PauseCircle className="w-4 h-4 text-blue-400" />
              무음 자동화 제어 (Silence Automation)
            </span>
            <span className="text-xs text-blue-400 font-mono">Auto Control</span>
          </div>

          <div className="space-y-3">
            {/* Auto Pause */}
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2.5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-7 h-7 rounded-lg bg-blue-500/10 border border-blue-500/20 text-blue-400 flex items-center justify-center shrink-0">
                    <PauseCircle className="w-3.5 h-3.5" />
                  </div>
                  <div>
                    <div className="text-xs font-bold text-slate-200">무음 구간 자동 일시정지 & 재개</div>
                    <div className="text-[10px] text-slate-400">말이 없으면 일시중지, 소리가 나면 자동 재개</div>
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
                <div className="pt-2 border-t border-slate-900 space-y-1">
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-slate-400 text-[11px]">일시정지 대기 시간</span>
                    <span className="font-mono font-bold text-blue-400">
                      {localSettings.auto_pause_seconds.toFixed(1)}초
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
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2.5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-7 h-7 rounded-lg bg-red-500/10 border border-red-500/20 text-red-400 flex items-center justify-center shrink-0">
                    <StopCircle className="w-3.5 h-3.5" />
                  </div>
                  <div>
                    <div className="text-xs font-bold text-slate-200">무음 지속 시 자동 종료 & 저장</div>
                    <div className="text-[10px] text-slate-400">설정 시간 동안 무음 시 녹화 자동 종료</div>
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
                <div className="pt-2 border-t border-slate-900 space-y-1">
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-slate-400 text-[11px]">자동 종료 대기 시간</span>
                    <span className="font-mono font-bold text-red-400">
                      {localSettings.auto_stop_seconds.toFixed(1)}초
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

        {/* Section 4: 기본 저장 경로 & FFmpeg 엔진 */}
        <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-4">
          <div className="flex items-center justify-between">
            <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
              <Folder className="w-4 h-4 text-amber-400" />
              저장 경로 & FFmpeg 엔진
            </span>
            <span
              className={`text-[11px] font-mono px-2 py-0.5 rounded-full border ${
                ffmpegDetected
                  ? 'bg-emerald-950/60 border-emerald-800/60 text-emerald-400'
                  : 'bg-amber-950/60 border-amber-800/60 text-amber-400'
              }`}
            >
              {ffmpegDetected ? 'FFmpeg Ready' : 'FFmpeg Needed'}
            </span>
          </div>

          <div className="space-y-3">
            {/* Save Directory */}
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2">
              <label className="text-xs font-semibold text-slate-300">기본 파일 저장 폴더</label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  readOnly
                  value={localSettings.output_dir}
                  className="flex-1 px-3 py-1.5 bg-slate-950 border border-slate-800 rounded-xl text-xs font-mono text-slate-300 focus:outline-none"
                />
                <button
                  type="button"
                  onClick={handleSelectFolder}
                  className="px-3.5 py-1.5 bg-blue-600 hover:bg-blue-500 text-white text-xs font-bold rounded-xl shadow transition"
                >
                  폴더 변경
                </button>
              </div>
            </div>

            {/* Custom FFmpeg binary path */}
            <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2">
              <label className="text-xs font-semibold text-slate-300 flex items-center gap-1.5">
                <Terminal className="w-3.5 h-3.5 text-indigo-400" />
                <span>사용자 지정 FFmpeg 경로 (선택)</span>
              </label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  placeholder="자동 감지 사용 (비워두면 자동 탐색)"
                  value={localSettings.custom_ffmpeg_path || ''}
                  onChange={(e) => {
                    const val = e.target.value.trim() || null;
                    setLocalSettings((prev) => ({ ...prev, custom_ffmpeg_path: val }));
                  }}
                  onBlur={() => handleChange({ custom_ffmpeg_path: localSettings.custom_ffmpeg_path })}
                  className="flex-1 px-3 py-1.5 bg-slate-950 border border-slate-800 rounded-xl text-xs font-mono text-slate-300 focus:outline-none"
                />
                <button
                  type="button"
                  onClick={handleTestFfmpeg}
                  className="px-3 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-bold rounded-xl transition"
                >
                  테스트
                </button>
              </div>
              {ffmpegTestMsg && <p className="text-[11px] font-mono text-slate-400">{ffmpegTestMsg}</p>}
            </div>
          </div>
        </div>
      </div>

      {/* Section 5: 기본 녹화 & 녹음 규격 프리셋 */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-4">
        <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
          <Layers className="w-4 h-4 text-purple-400" />
          기본 미디어 규격 프리셋 (Default Media Quality)
        </span>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          {/* Default FPS */}
          <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2">
            <div className="text-xs font-semibold text-slate-300 flex items-center gap-1.5">
              <Monitor className="w-3.5 h-3.5 text-blue-400" />
              <span>화면 녹화 프레임 (FPS)</span>
            </div>
            <div className="grid grid-cols-2 gap-2">
              {[30, 60].map((fps) => (
                <button
                  key={fps}
                  type="button"
                  onClick={() => handleChange({ video_fps: fps })}
                  className={`py-2 rounded-xl border text-xs font-bold transition ${
                    localSettings.video_fps === fps
                      ? 'bg-blue-600 border-blue-500 text-white shadow'
                      : 'bg-slate-900 border-slate-800 text-slate-400 hover:text-slate-200'
                  }`}
                >
                  {fps} FPS
                </button>
              ))}
            </div>
          </div>

          {/* Default Audio Format */}
          <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2">
            <div className="text-xs font-semibold text-slate-300 flex items-center gap-1.5">
              <Music className="w-3.5 h-3.5 text-indigo-400" />
              <span>오디오 녹음 기본 포맷</span>
            </div>
            <div className="grid grid-cols-3 gap-1.5">
              {(['m4a', 'mp3', 'wav'] as const).map((fmt) => (
                <button
                  key={fmt}
                  type="button"
                  onClick={() => handleChange({ audio_format: fmt as AudioFormat })}
                  className={`py-2 rounded-xl border text-xs font-bold uppercase transition ${
                    localSettings.audio_format === fmt
                      ? 'bg-indigo-600 border-indigo-500 text-white shadow'
                      : 'bg-slate-900 border-slate-800 text-slate-400 hover:text-slate-200'
                  }`}
                >
                  {fmt}
                </button>
              ))}
            </div>
          </div>

          {/* Default Audio Bitrate */}
          <div className="p-3.5 rounded-xl bg-slate-950/60 border border-slate-800 space-y-2">
            <div className="text-xs font-semibold text-slate-300 flex items-center gap-1.5">
              <Radio className="w-3.5 h-3.5 text-cyan-400" />
              <span>오디오 기본 비트레이트</span>
            </div>
            <div className="grid grid-cols-4 gap-1.5">
              {[128, 192, 256, 320].map((b) => (
                <button
                  key={b}
                  type="button"
                  onClick={() => handleChange({ audio_bitrate: b })}
                  className={`py-2 rounded-xl border text-[11px] font-bold font-mono transition ${
                    localSettings.audio_bitrate === b
                      ? 'bg-cyan-600 border-cyan-500 text-white shadow'
                      : 'bg-slate-900 border-slate-800 text-slate-400 hover:text-slate-200'
                  }`}
                >
                  {b}k
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
