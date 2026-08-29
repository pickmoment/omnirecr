import React from 'react';
import { Mic, Video } from 'lucide-react';
import type { RecordView, RecordingStatus, RectRegion, Settings } from '../types';
import { ScreenRecorder } from './ScreenRecorder';
import { AudioRecorder } from './AudioRecorder';
import { TabBar, type TabBarItem } from './TabBar';

interface RecordStudioProps {
  settings: Settings;
  recordingStatus: RecordingStatus;
  selectedRegion: RectRegion | null;
  onClearRegion: () => void;
  onOpenSelectionOverlay: () => Promise<void>;
  onOpenSettings: () => void;
  onStartScreenRecord: (region: RectRegion | null) => Promise<void>;
  onStartAudioRecord: () => Promise<void>;
  onPauseRecord: () => Promise<void>;
  onResumeRecord: () => Promise<void>;
  onStopRecord: () => Promise<void>;
}

const TABS: TabBarItem<RecordView>[] = [
  {
    key: 'audio',
    label: '오디오 녹음',
    icon: <Mic className="w-4 h-4" />,
    accent: 'bg-indigo-600 shadow-indigo-600/30',
  },
  {
    key: 'screen',
    label: '화면 녹화',
    icon: <Video className="w-4 h-4" />,
    accent: 'bg-blue-600 shadow-blue-600/30',
  },
];

/** 오디오 녹음과 화면 녹화를 한 탭으로 묶는다. */
export const RecordStudio: React.FC<RecordStudioProps> = ({
  settings,
  recordingStatus,
  selectedRegion,
  onClearRegion,
  onOpenSelectionOverlay,
  onOpenSettings,
  onStartScreenRecord,
  onStartAudioRecord,
  onPauseRecord,
  onResumeRecord,
  onStopRecord,
}) => {
  const [view, setView] = React.useState<RecordView>('audio');

  return (
    <div className="h-full min-h-0 flex flex-col">
      <div className="px-6 pt-5 shrink-0">
        <TabBar items={TABS} current={view} onSelect={setView} className="w-fit" />
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        {view === 'audio' ? (
          <AudioRecorder
            settings={settings}
            recordingStatus={recordingStatus}
            onOpenSettings={onOpenSettings}
            onStartRecord={onStartAudioRecord}
            onPauseRecord={onPauseRecord}
            onResumeRecord={onResumeRecord}
            onStopRecord={onStopRecord}
          />
        ) : (
          <ScreenRecorder
            settings={settings}
            recordingStatus={recordingStatus}
            selectedRegion={selectedRegion}
            onClearRegion={onClearRegion}
            onOpenSelectionOverlay={onOpenSelectionOverlay}
            onOpenSettings={onOpenSettings}
            onStartRecord={onStartScreenRecord}
            onPauseRecord={onPauseRecord}
            onResumeRecord={onResumeRecord}
            onStopRecord={onStopRecord}
          />
        )}
      </div>
    </div>
  );
};
