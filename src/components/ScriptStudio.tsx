import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { BookText, ListChecks, Mic } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import type {
  RecordingStatus,
  ScriptItem,
  ScriptStudioView,
  Settings,
} from '../types';
import { ScriptLibrary } from './ScriptLibrary';
import { TtsRecorder, type StartTtsRecordOptions } from './TtsRecorder';
import { TtsBatchRunner } from './TtsBatchRunner';
import { TabBar, type TabBarItem } from './TabBar';

interface ScriptStudioProps {
  settings: Settings;
  recordingStatus: RecordingStatus;
  onUpdateSettings: (partial: Partial<Settings>) => Promise<void>;
  onStartRecord: (options: StartTtsRecordOptions) => Promise<string | null>;
  onPauseRecord: () => Promise<void>;
  onResumeRecord: () => Promise<void>;
  onStopRecord: () => Promise<void>;
  onSendToSubtitle: (audioPath: string, scriptText: string) => void;
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenDefaultPlayer: (path: string) => Promise<void>;
  onOpenSettings: () => void;
}

export const ScriptStudio: React.FC<ScriptStudioProps> = ({
  settings,
  recordingStatus,
  onUpdateSettings,
  onStartRecord,
  onPauseRecord,
  onResumeRecord,
  onStopRecord,
  onSendToSubtitle,
  onOpenExplorer,
  onOpenDefaultPlayer,
  onOpenSettings,
}) => {
  const [view, setView] = useState<ScriptStudioView>('batch');
  const [scripts, setScripts] = useState<ScriptItem[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // 일괄 자동 처리 대상으로 체크한 대본들
  const [batchIds, setBatchIds] = useState<string[]>([]);

  const refreshScripts = useCallback(async () => {
    setIsLoading(true);
    try {
      const items = await invoke<ScriptItem[]>('list_scripts');
      setScripts(items);
    } catch (err) {
      console.error('대본 목록을 불러오지 못했습니다:', err);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    refreshScripts();
  }, [refreshScripts]);

  const selectedScript = useMemo(
    () => scripts.find((s) => s.id === selectedId) ?? null,
    [scripts, selectedId],
  );

  const tabs: TabBarItem<ScriptStudioView>[] = [
    {
      key: 'batch',
      label: '자동 일괄 녹음',
      icon: <ListChecks className="w-4 h-4" />,
      accent: 'bg-indigo-600 shadow-indigo-600/30',
    },
    {
      key: 'library',
      label: '대본 관리',
      icon: <BookText className="w-4 h-4" />,
      accent: 'bg-emerald-600 shadow-emerald-600/30',
    },
    {
      key: 'manual',
      label: '수동 녹음',
      icon: <Mic className="w-4 h-4" />,
      accent: 'bg-slate-700 shadow-slate-700/30',
    },
  ];

  const toggleBatchId = (id: string) =>
    setBatchIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  return (
    <div className="h-full min-h-0 flex flex-col p-6 pt-5 max-w-6xl mx-auto w-full">
      {/* 서브 탭 */}
      <TabBar items={tabs} current={view} onSelect={setView} className="self-start mb-4 shrink-0" />

      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className={view === 'batch' ? '' : 'hidden'}>
          <TtsBatchRunner
            settings={settings}
            scripts={scripts}
            selectedIds={batchIds}
            onToggleSelect={toggleBatchId}
            onSelectAll={setBatchIds}
            recordingStatus={recordingStatus}
            onUpdateSettings={onUpdateSettings}
            onStartRecord={onStartRecord}
            onStopRecord={onStopRecord}
            onRefreshScripts={refreshScripts}
            onOpenExplorer={onOpenExplorer}
            onGoToLibrary={() => setView('library')}
            onOpenSettings={onOpenSettings}
          />
        </div>

        <div className={view === 'library' ? '' : 'hidden'}>
          <ScriptLibrary
            scripts={scripts}
            isLoading={isLoading}
            selectedId={selectedId}
            onSelect={setSelectedId}
            onRefresh={refreshScripts}
            batchIds={batchIds}
            onToggleBatch={toggleBatchId}
            onSendToTts={(script) => {
              setSelectedId(script.id);
              setView('manual');
            }}
            onOpenExplorer={onOpenExplorer}
          />
        </div>

        <div className={view === 'manual' ? '' : 'hidden'}>
          <TtsRecorder
            settings={settings}
            scripts={scripts}
            selectedScript={selectedScript}
            onSelectScript={setSelectedId}
            recordingStatus={recordingStatus}
            onUpdateSettings={onUpdateSettings}
            onStartRecord={onStartRecord}
            onPauseRecord={onPauseRecord}
            onResumeRecord={onResumeRecord}
            onStopRecord={onStopRecord}
            onRefreshScripts={refreshScripts}
            onSendToSubtitle={onSendToSubtitle}
            onOpenExplorer={onOpenExplorer}
            onOpenDefaultPlayer={onOpenDefaultPlayer}
            onGoToLibrary={() => setView('library')}
          />
        </div>
      </div>
    </div>
  );
};
