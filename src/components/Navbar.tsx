import React from 'react';
import {
  Activity,
  AlertTriangle,
  BookText,
  CheckCircle,
  FileText,
  FolderOpen,
  Mic,
  Settings as SettingsIcon,
} from 'lucide-react';
import type { TabType, RecordingStatus } from '../types';
import { formatTimer } from '../utils/format';
import { TabBar, type TabBarItem } from './TabBar';

interface NavbarProps {
  currentTab: TabType;
  onSelectTab: (tab: TabType) => void;
  recordingStatus: RecordingStatus;
  ffmpegDetected: boolean;
}

// 작업 흐름 단위로 묶은 상단 탭. 세부 화면은 각 탭 안의 서브 탭으로 나뉜다.
const TABS: TabBarItem<TabType>[] = [
  {
    key: 'record',
    label: '녹음 & 녹화',
    icon: <Mic className="w-4 h-4" />,
    accent: 'bg-indigo-600 shadow-indigo-600/30',
  },
  {
    key: 'script',
    label: '대본 & TTS',
    icon: <BookText className="w-4 h-4" />,
    accent: 'bg-emerald-600 shadow-emerald-600/30',
  },
  {
    key: 'subtitle',
    label: '자막',
    icon: <FileText className="w-4 h-4" />,
    accent: 'bg-amber-600 shadow-amber-600/30',
  },
  {
    key: 'files',
    label: '파일',
    icon: <FolderOpen className="w-4 h-4" />,
    accent: 'bg-cyan-600 shadow-cyan-600/30',
  },
  {
    key: 'settings',
    label: '환경 설정',
    icon: <SettingsIcon className="w-4 h-4" />,
    accent: 'bg-slate-700 shadow-slate-700/30',
  },
];

export const Navbar: React.FC<NavbarProps> = ({
  currentTab,
  onSelectTab,
  recordingStatus,
  ffmpegDetected,
}) => {
  const isRecording = recordingStatus.status === 'recording' || recordingStatus.status === 'paused';

  return (
    <header className="h-16 bg-slate-900/90 backdrop-blur-md border-b border-slate-800/80 px-5 flex items-center justify-between z-30 shrink-0 select-none">
      {/* Brand Logo & Name */}
      <div className="flex items-center gap-3 shrink-0">
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

      <TabBar items={TABS} current={currentTab} onSelect={onSelectTab} />

      {/* Status Badges */}
      <div className="flex items-center gap-3 shrink-0">
        {isRecording && (
          <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-red-950/80 border border-red-500/40 text-red-300 text-xs font-semibold animate-pulse">
            <div className="w-2.5 h-2.5 rounded-full bg-red-500 animate-ping" />
            <span>
              {recordingStatus.mode === 'screen' ? '화면 녹화 중' : '오디오 녹음 중'} (
              {recordingStatus.status === 'paused'
                ? '일시정지'
                : formatTimer(recordingStatus.duration_secs)}
              )
            </span>
          </div>
        )}

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
      </div>
    </header>
  );
};
