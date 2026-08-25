import React from 'react';
import { Video, Mic, FolderOpen, Merge, RefreshCw, Settings as SettingsIcon, Activity, CheckCircle, AlertTriangle, FileText } from 'lucide-react';
import type { TabType, RecordingStatus } from '../types';

interface NavbarProps {
  currentTab: TabType;
  onSelectTab: (tab: TabType) => void;
  recordingStatus: RecordingStatus;
  ffmpegDetected: boolean;
}

export const Navbar: React.FC<NavbarProps> = ({
  currentTab,
  onSelectTab,
  recordingStatus,
  ffmpegDetected,
}) => {
  const isRecording = recordingStatus.status === 'recording' || recordingStatus.status === 'paused';

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

  return (
    <header className="h-16 bg-slate-900/90 backdrop-blur-md border-b border-slate-800/80 px-5 flex items-center justify-between z-30 shrink-0 select-none">
      {/* Brand Logo & Name */}
      <div className="flex items-center gap-3">
        <div className="w-10 h-10 rounded-xl bg-gradient-to-br from-indigo-500 via-blue-600 to-cyan-400 flex items-center justify-center shadow-lg shadow-blue-500/25">
          <Activity className="w-6 h-6 text-white" />
        </div>
        <div>
          <div className="flex items-center gap-2">
            <span className="font-bold text-lg tracking-tight bg-gradient-to-r from-white via-slate-100 to-slate-400 bg-clip-text text-transparent">
              OmniRec
            </span>
            <span className="text-[10px] uppercase font-semibold tracking-wider px-1.5 py-0.5 rounded bg-blue-500/20 text-blue-400 border border-blue-500/30">
              Studio
            </span>
          </div>
          <p className="text-[11px] text-slate-400 font-medium">Screen & Audio Studio</p>
        </div>
      </div>

      {/* Navigation Tabs */}
      <nav className="flex items-center gap-1.5 bg-slate-950/60 p-1 rounded-xl border border-slate-800">
        <button
          onClick={() => onSelectTab('screen')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'screen'
              ? 'bg-blue-600 text-white shadow-md shadow-blue-600/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <Video className="w-4 h-4" />
          <span>화면 녹화</span>
        </button>

        <button
          onClick={() => onSelectTab('audio')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'audio'
              ? 'bg-indigo-600 text-white shadow-md shadow-indigo-600/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <Mic className="w-4 h-4" />
          <span>오디오 녹음</span>
        </button>

        <button
          onClick={() => onSelectTab('history')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'history'
              ? 'bg-cyan-600 text-white shadow-md shadow-cyan-600/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <FolderOpen className="w-4 h-4" />
          <span>히스토리</span>
        </button>

        <button
          onClick={() => onSelectTab('merger')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'merger'
              ? 'bg-purple-600 text-white shadow-md shadow-purple-600/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <Merge className="w-4 h-4" />
          <span>파일 연결 & 병합</span>
        </button>

        <button
          onClick={() => onSelectTab('converter')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'converter'
              ? 'bg-teal-600 text-white shadow-md shadow-teal-600/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <RefreshCw className="w-4 h-4" />
          <span>오디오 변환</span>
        </button>

        <button
          onClick={() => onSelectTab('subtitle')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'subtitle'
              ? 'bg-amber-600 text-white shadow-md shadow-amber-600/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <FileText className="w-4 h-4" />
          <span>자막 생성기</span>
        </button>

        <button
          onClick={() => onSelectTab('settings')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
            currentTab === 'settings'
              ? 'bg-slate-700 text-white shadow-md shadow-slate-700/30'
              : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
          }`}
        >
          <SettingsIcon className="w-4 h-4" />
          <span>환경 설정</span>
        </button>
      </nav>

      {/* Status Badges & Settings */}
      <div className="flex items-center gap-3">
        {/* Live Recording Badge */}
        {isRecording && (
          <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-red-950/80 border border-red-500/40 text-red-300 text-xs font-semibold animate-pulse">
            <div className="w-2.5 h-2.5 rounded-full bg-red-500 animate-ping" />
            <span>
              {recordingStatus.mode === 'screen' ? '화면 녹화 중' : '오디오 녹음 중'} (
              {recordingStatus.status === 'paused' ? '일시정지' : formatTimer(recordingStatus.duration_secs)})
            </span>
          </div>
        )}

        {/* FFmpeg status */}
        <div
          title={ffmpegDetected ? 'FFmpeg 엔진 준비 완료' : 'FFmpeg 감지 안됨 - 설정에서 확인 필요'}
          className={`flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[11px] font-medium border ${
            ffmpegDetected
              ? 'bg-emerald-950/50 border-emerald-800/60 text-emerald-400'
              : 'bg-amber-950/50 border-amber-800/60 text-amber-400'
          }`}
        >
          {ffmpegDetected ? (
            <>
              <CheckCircle className="w-3.5 h-3.5" />
              <span>FFmpeg OK</span>
            </>
          ) : (
            <>
              <AlertTriangle className="w-3.5 h-3.5" />
              <span>FFmpeg 필요</span>
            </>
          )}
        </div>

        {/* Settings quick button */}
        <button
          onClick={() => onSelectTab('settings')}
          title="환경 설정으로 이동"
          className={`p-2 rounded-lg border transition-all duration-150 ${
            currentTab === 'settings'
              ? 'bg-slate-800 text-white border-slate-700'
              : 'text-slate-400 hover:text-white hover:bg-slate-800 border-slate-800/80'
          }`}
        >
          <SettingsIcon className="w-4 h-4" />
        </button>
      </div>
    </header>
  );
};
