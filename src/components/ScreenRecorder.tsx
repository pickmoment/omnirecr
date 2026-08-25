import React, { useState } from 'react';
import {
  Monitor,
  Crop,
  Volume2,
  Play,
  Pause,
  Square,
  Timer,
  Settings as SettingsIcon,
  HardDrive,
} from 'lucide-react';
import type { RectRegion, Settings, RecordingStatus } from '../types';
import { AudioVisualizer } from './AudioVisualizer';

interface ScreenRecorderProps {
  settings: Settings;
  recordingStatus: RecordingStatus;
  selectedRegion: RectRegion | null;
  onClearRegion: () => void;
  onOpenSelectionOverlay: () => void;
  onOpenSettings: () => void;
  onStartRecord: (region: RectRegion | null) => Promise<void>;
  onPauseRecord: () => Promise<void>;
  onResumeRecord: () => Promise<void>;
  onStopRecord: () => Promise<void>;
}

export const ScreenRecorder: React.FC<ScreenRecorderProps> = ({
  settings,
  recordingStatus,
  selectedRegion,
  onClearRegion,
  onOpenSelectionOverlay,
  onOpenSettings,
  onStartRecord,
  onPauseRecord,
  onResumeRecord,
  onStopRecord,
}) => {
  const [screenMode, setScreenMode] = useState<'fullscreen' | 'region'>(
    selectedRegion ? 'region' : 'fullscreen'
  );

  const isRecording = recordingStatus.status === 'recording';
  const isPaused = recordingStatus.status === 'paused';
  const isBusy = recordingStatus.status === 'stopping';

  const formatTimer = (seconds: number) => {
    const s = Math.floor(seconds);
    const hrs = Math.floor(s / 3600);
    const mins = Math.floor((s % 3600) / 60);
    const secs = s % 60;
    if (hrs > 0) {
      return `${String(hrs).padStart(2, '0')}:${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
    }
    return `${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
  };

  const formatFileSize = (bytes: number) => {
    if (bytes <= 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${(bytes / Math.pow(k, i)).toFixed(i > 1 ? 2 : 1)} ${sizes[i]}`;
  };

  const getLiveSize = () => {
    if (!isRecording && !isPaused) return 0;
    // Real-time smooth size estimation based on video fps & average h264 bitrate
    const estimatedBitrateBps = (settings.video_fps >= 60 ? 5000000 : 3000000) / 8;
    const estimatedBytes = Math.floor(estimatedBitrateBps * recordingStatus.duration_secs);
    return Math.max(recordingStatus.size_bytes, estimatedBytes);
  };

  const handleStart = () => {
    const region = screenMode === 'region' ? selectedRegion : null;
    onStartRecord(region);
  };

  return (
    <div className="h-full flex flex-col p-6 space-y-5 overflow-y-auto max-w-5xl mx-auto">
      {/* Mode Selector Card */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-4">
        <div className="flex items-center justify-between">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <Monitor className="w-4 h-4 text-blue-400" />
            녹화 대상 영역 선택 (Screen Recording Target)
          </span>
          <div className="flex items-center gap-2">
            <span className="text-xs text-blue-400 font-mono font-medium px-2.5 py-0.5 rounded-full bg-blue-950/60 border border-blue-800/40">
              {settings.video_fps} FPS • H.264 MP4
            </span>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <button
            disabled={isRecording || isPaused}
            onClick={() => {
              setScreenMode('fullscreen');
              onClearRegion();
            }}
            className={`flex flex-col items-center justify-center p-4 rounded-xl border transition-all duration-200 ${
              screenMode === 'fullscreen'
                ? 'bg-blue-600/20 border-blue-500 text-blue-300 shadow-md shadow-blue-500/10'
                : 'bg-slate-950/50 border-slate-800 text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'
            } ${isRecording || isPaused ? 'opacity-60 cursor-not-allowed' : ''}`}
          >
            <Monitor className="w-6 h-6 mb-2" />
            <span className="text-sm font-bold">전체 화면 녹화</span>
            <span className="text-xs text-slate-500 mt-0.5">디스플레이 전체 영역 실시간 캡처</span>
          </button>

          <button
            disabled={isRecording || isPaused}
            onClick={() => {
              setScreenMode('region');
              onOpenSelectionOverlay();
            }}
            className={`flex flex-col items-center justify-center p-4 rounded-xl border transition-all duration-200 ${
              screenMode === 'region'
                ? 'bg-blue-600/20 border-blue-500 text-blue-300 shadow-md shadow-blue-500/10'
                : 'bg-slate-950/50 border-slate-800 text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'
            } ${isRecording || isPaused ? 'opacity-60 cursor-not-allowed' : ''}`}
          >
            <Crop className="w-6 h-6 mb-2" />
            <span className="text-sm font-bold">영역 드래그 지정</span>
            <span className="text-xs text-slate-500 mt-0.5">원하는 특정 윈도우 또는 사각형 영역</span>
          </button>
        </div>

        {/* Region Status Tag */}
        {screenMode === 'region' && (
          <div className="flex items-center justify-between p-3 rounded-xl bg-blue-950/40 border border-blue-500/30 text-xs">
            <div className="flex items-center gap-2 text-blue-300 font-mono">
              <Crop className="w-4 h-4 text-blue-400" />
              <span>
                {selectedRegion
                  ? `지정된 캡처 영역: ${selectedRegion.width} × ${selectedRegion.height} px (X: ${selectedRegion.x}, Y: ${selectedRegion.y})`
                  : '영역이 선택되지 않았습니다. [영역 지정하기] 버튼을 누르세요.'}
              </span>
            </div>
            <button
              disabled={isRecording || isPaused}
              onClick={onOpenSelectionOverlay}
              className="text-xs font-bold text-blue-400 hover:text-blue-200 underline"
            >
              다시 지정하기
            </button>
          </div>
        )}
      </div>

      {/* Centralized Audio & Filter Info Banner */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg flex flex-col md:flex-row items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="w-8 h-8 rounded-xl bg-cyan-600/20 border border-cyan-500/30 text-cyan-400 flex items-center justify-center shrink-0">
            <Volume2 className="w-4 h-4" />
          </div>
          <div>
            <div className="text-xs font-bold text-slate-200 flex items-center gap-2">
              <span>오디오 믹서 & 스마트 필터 설정</span>
              <span className="text-[10px] text-slate-400 font-mono">
                (시스템 {settings.system_audio_enabled ? `${Math.round(settings.system_audio_volume * 100)}%` : 'OFF'} • 마이크 {settings.mic_audio_enabled ? `${Math.round(settings.mic_audio_volume * 100)}%` : 'OFF'})
              </span>
            </div>
            <div className="text-[11px] text-slate-400 flex items-center gap-2 mt-0.5">
              <span>알림 차단: <b className="text-slate-300">{settings.mute_notifications ? 'ON' : 'OFF'}</b></span>
              <span>•</span>
              <span>노이즈 게이트: <b className="text-slate-300">{settings.noise_gate_enabled ? `${settings.noise_gate_threshold_db}dB` : 'OFF'}</b></span>
              <span>•</span>
              <span>80Hz Low-cut: <b className="text-slate-300">{settings.highpass_filter_enabled ? 'ON' : 'OFF'}</b></span>
            </div>
          </div>
        </div>

        <button
          onClick={onOpenSettings}
          className="flex items-center gap-1.5 px-3.5 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition shrink-0 self-end md:self-auto"
        >
          <SettingsIcon className="w-3.5 h-3.5 text-cyan-400" />
          <span>환경 설정에서 변경</span>
        </button>
      </div>

      {/* Real-time VU Meter Visualizer */}
      <AudioVisualizer
        sysLevelDb={recordingStatus.sys_vu_level}
        micLevelDb={recordingStatus.mic_vu_level}
        isRecording={isRecording || isPaused}
        systemAudioEnabled={settings.system_audio_enabled}
        micAudioEnabled={settings.mic_audio_enabled}
      />

      {/* Bottom Main Action & Control Bar */}
      <div className="bg-slate-900/90 border border-slate-800 rounded-2xl p-6 shadow-2xl flex flex-col md:flex-row items-center justify-between gap-6 shrink-0">
        {/* Timer & Live Status */}
        <div className="flex items-center gap-4">
          <div
            className={`w-14 h-14 rounded-2xl flex items-center justify-center border ${
              isRecording
                ? 'bg-red-950/60 border-red-500/50 text-red-400 glow-red animate-pulse'
                : isPaused
                ? 'bg-amber-950/60 border-amber-500/50 text-amber-400'
                : 'bg-slate-950 border-slate-800 text-slate-500'
            }`}
          >
            <Timer className="w-7 h-7" />
          </div>
          <div>
            <div className="flex items-baseline gap-3">
              <span className="text-3xl font-mono font-extrabold tracking-wider text-white">
                {formatTimer(recordingStatus.duration_secs)}
              </span>
              {(isRecording || isPaused) && (
                <span className="flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-red-950/80 border border-red-700/50 text-red-300 font-mono text-xs font-bold animate-fade-in shadow-inner">
                  <HardDrive className="w-3.5 h-3.5 text-red-400" />
                  <span>{formatFileSize(getLiveSize())}</span>
                </span>
              )}
            </div>
            <div className="text-xs text-slate-400 font-medium flex items-center gap-1.5 mt-0.5">
              <span
                className={`w-2 h-2 rounded-full ${
                  isRecording ? 'bg-red-500 animate-ping' : isPaused ? 'bg-amber-500' : 'bg-slate-600'
                }`}
              />
              <span>
                {isRecording
                  ? `화면 녹화 진행 중... (${settings.video_fps} FPS)`
                  : isPaused
                  ? '녹화 일시정지 됨'
                  : '화면 녹화 대기 중'}
              </span>
            </div>
          </div>
        </div>

        {/* Big Action Buttons */}
        <div className="flex items-center gap-3">
          {!isRecording && !isPaused ? (
            <button
              disabled={isBusy}
              onClick={handleStart}
              className="flex items-center gap-2.5 px-8 py-4 rounded-xl bg-gradient-to-r from-blue-600 to-indigo-600 hover:from-blue-500 hover:to-indigo-500 text-white font-bold text-sm shadow-xl shadow-blue-600/30 active:scale-95 transition-all duration-150"
            >
              <Play className="w-5 h-5 fill-current" />
              <span>화면 녹화 시작</span>
            </button>
          ) : (
            <>
              {isRecording ? (
                <button
                  disabled={isBusy}
                  onClick={onPauseRecord}
                  className="flex items-center gap-2 px-5 py-3.5 rounded-xl bg-amber-600 hover:bg-amber-500 text-white font-bold text-xs shadow-lg shadow-amber-600/20 active:scale-95 transition"
                >
                  <Pause className="w-4 h-4 fill-current" />
                  <span>일시정지</span>
                </button>
              ) : (
                <button
                  disabled={isBusy}
                  onClick={onResumeRecord}
                  className="flex items-center gap-2 px-5 py-3.5 rounded-xl bg-emerald-600 hover:bg-emerald-500 text-white font-bold text-xs shadow-lg shadow-emerald-600/20 active:scale-95 transition"
                >
                  <Play className="w-4 h-4 fill-current" />
                  <span>녹화 재개</span>
                </button>
              )}

              <button
                disabled={isBusy}
                onClick={onStopRecord}
                className="flex items-center gap-2 px-7 py-3.5 rounded-xl bg-red-600 hover:bg-red-500 text-white font-bold text-xs shadow-xl shadow-red-600/30 active:scale-95 transition"
              >
                <Square className="w-4 h-4 fill-current" />
                <span>{isBusy ? '저장 중...' : '녹화 종료 및 저장'}</span>
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
};
