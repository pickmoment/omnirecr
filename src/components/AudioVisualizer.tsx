import React from 'react';
import { Volume2, Mic } from 'lucide-react';

interface AudioVisualizerProps {
  sysLevelDb: number; // -60 to 0
  micLevelDb: number; // -60 to 0
  isRecording: boolean;
  systemAudioEnabled: boolean;
  micAudioEnabled: boolean;
}

export const AudioVisualizer: React.FC<AudioVisualizerProps> = ({
  sysLevelDb,
  micLevelDb,
  isRecording,
  systemAudioEnabled,
  micAudioEnabled,
}) => {
  // Convert dB (-60 to 0) to percentage (0 to 100)
  const dbToPercent = (db: number, enabled: boolean) => {
    if (!enabled || !isRecording) return 0;
    if (db <= -60) return 0;
    if (db >= 0) return 100;
    return Math.round(((db + 60) / 60) * 100);
  };

  const sysPercent = dbToPercent(sysLevelDb, systemAudioEnabled);
  const micPercent = dbToPercent(micLevelDb, micAudioEnabled);

  return (
    <div className="bg-slate-900/70 border border-slate-800 rounded-xl p-4 space-y-3.5">
      <div className="flex items-center justify-between">
        <span className="text-xs font-semibold text-slate-300 tracking-wide flex items-center gap-1.5">
          <span className={`w-2 h-2 rounded-full ${isRecording ? 'bg-emerald-400 animate-pulse' : 'bg-slate-600'}`} />
          실시간 오디오 레벨 미터 (Real-time VU Meter)
        </span>
        <span className="text-[11px] text-slate-400 font-mono">dBFS Scale (-60 ~ 0 dB)</span>
      </div>

      {/* System Sound Meter */}
      <div className="space-y-1.5">
        <div className="flex items-center justify-between text-xs">
          <span className="flex items-center gap-1.5 text-slate-300 font-medium">
            <Volume2 className={`w-3.5 h-3.5 ${systemAudioEnabled ? 'text-blue-400' : 'text-slate-600'}`} />
            <span>시스템 사운드 (WASAPI)</span>
          </span>
          <span className="text-[11px] font-mono text-slate-400">
            {systemAudioEnabled && isRecording ? `${sysLevelDb.toFixed(1)} dB` : 'OFF'}
          </span>
        </div>
        <div className="h-3 w-full bg-slate-950 rounded-full overflow-hidden p-0.5 border border-slate-800 flex items-center">
          <div
            className="h-full rounded-full transition-all duration-75 ease-out bg-gradient-to-r from-blue-500 via-emerald-400 to-red-500"
            style={{ width: `${sysPercent}%` }}
          />
        </div>
      </div>

      {/* Microphone Sound Meter */}
      <div className="space-y-1.5">
        <div className="flex items-center justify-between text-xs">
          <span className="flex items-center gap-1.5 text-slate-300 font-medium">
            <Mic className={`w-3.5 h-3.5 ${micAudioEnabled ? 'text-indigo-400' : 'text-slate-600'}`} />
            <span>마이크 입력 (Microphone)</span>
          </span>
          <span className="text-[11px] font-mono text-slate-400">
            {micAudioEnabled && isRecording ? `${micLevelDb.toFixed(1)} dB` : 'OFF'}
          </span>
        </div>
        <div className="h-3 w-full bg-slate-950 rounded-full overflow-hidden p-0.5 border border-slate-800 flex items-center">
          <div
            className="h-full rounded-full transition-all duration-75 ease-out bg-gradient-to-r from-indigo-500 via-emerald-400 to-red-500"
            style={{ width: `${micPercent}%` }}
          />
        </div>
      </div>
    </div>
  );
};
