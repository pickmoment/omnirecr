import React from 'react';
import {
  Music,
  Play,
  Pause,
  Square,
  Timer,
  Sliders,
  Settings as SettingsIcon,
  HardDrive,
} from 'lucide-react';
import type { Settings, RecordingStatus } from '../types';
import { AudioVisualizer } from './AudioVisualizer';

interface AudioRecorderProps {
  settings: Settings;
  recordingStatus: RecordingStatus;
  onOpenSettings: () => void;
  onStartRecord: () => Promise<void>;
  onPauseRecord: () => Promise<void>;
  onResumeRecord: () => Promise<void>;
  onStopRecord: () => Promise<void>;
}

export const AudioRecorder: React.FC<AudioRecorderProps> = ({
  settings,
  recordingStatus,
  onOpenSettings,
  onStartRecord,
  onPauseRecord,
  onResumeRecord,
  onStopRecord,
}) => {
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
    // Real-time smooth size estimation based on audio bitrate (kbps -> bytes/sec)
    const estimatedBytes = Math.floor(((settings.audio_bitrate * 1000) / 8) * recordingStatus.duration_secs);
    return Math.max(recordingStatus.size_bytes, estimatedBytes);
  };

  return (
    <div className="h-full flex flex-col p-6 space-y-5 overflow-y-auto max-w-5xl mx-auto">
      {/* Audio Recording Profile Card */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-3">
        <div className="flex items-center justify-between">
          <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
            <Music className="w-4 h-4 text-indigo-400" />
            고음질 오디오 녹음 스튜디오 (Audio Recording Studio)
          </span>
          <div className="flex items-center gap-2">
            <span className="text-xs font-mono font-bold px-2.5 py-0.5 rounded-full bg-indigo-950/60 border border-indigo-800/40 text-indigo-300 uppercase">
              {settings.audio_format} • {settings.audio_bitrate} kbps • {settings.audio_sample_rate / 1000} kHz
            </span>
          </div>
        </div>

        <p className="text-xs text-slate-400">
          시스템 사운드와 마이크 입력을 고성능 DSP 필터(노이즈 게이트, 80Hz Low-cut)와 함께 실시간 믹싱하여 스튜디오급 사운드로 녹음합니다.
        </p>
      </div>

      {/* Centralized Audio & Filter Info Banner */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg flex flex-col md:flex-row items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="w-8 h-8 rounded-xl bg-indigo-600/20 border border-indigo-500/30 text-indigo-400 flex items-center justify-center shrink-0">
            <Sliders className="w-4 h-4" />
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
          <SettingsIcon className="w-3.5 h-3.5 text-indigo-400" />
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
                ? 'bg-indigo-950/60 border-indigo-500/50 text-indigo-400 glow-primary animate-pulse'
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
                <span className="flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-indigo-950/80 border border-indigo-700/50 text-indigo-300 font-mono text-xs font-bold animate-fade-in shadow-inner">
                  <HardDrive className="w-3.5 h-3.5 text-indigo-400" />
                  <span>{formatFileSize(getLiveSize())}</span>
                </span>
              )}
            </div>
            <div className="text-xs text-slate-400 font-medium flex items-center gap-1.5 mt-0.5">
              <span
                className={`w-2 h-2 rounded-full ${
                  isRecording ? 'bg-indigo-500 animate-ping' : isPaused ? 'bg-amber-500' : 'bg-slate-600'
                }`}
              />
              <span>
                {isRecording
                  ? `고음질 ${settings.audio_format.toUpperCase()} (${settings.audio_bitrate} kbps) 녹음 중...`
                  : isPaused
                  ? '오디오 녹음 일시정지 됨'
                  : '오디오 녹음 대기 중'}
              </span>
            </div>
          </div>
        </div>

        {/* Big Action Buttons */}
        <div className="flex items-center gap-3">
          {!isRecording && !isPaused ? (
            <button
              disabled={isBusy}
              onClick={onStartRecord}
              className="flex items-center gap-2.5 px-8 py-4 rounded-xl bg-gradient-to-r from-indigo-600 to-cyan-600 hover:from-indigo-500 hover:to-cyan-500 text-white font-bold text-sm shadow-xl shadow-indigo-600/30 active:scale-95 transition-all duration-150"
            >
              <Play className="w-5 h-5 fill-current" />
              <span>오디오 녹음 시작</span>
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
                  <span>녹음 재개</span>
                </button>
              )}

              <button
                disabled={isBusy}
                onClick={onStopRecord}
                className="flex items-center gap-2 px-7 py-3.5 rounded-xl bg-red-600 hover:bg-red-500 text-white font-bold text-xs shadow-xl shadow-red-600/30 active:scale-95 transition"
              >
                <Square className="w-4 h-4 fill-current" />
                <span>{isBusy ? '저장 중...' : '녹음 종료 및 저장'}</span>
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
};
