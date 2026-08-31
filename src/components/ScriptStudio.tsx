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
  onStopRecord: (options?: { silent?: boolean }) => Promise<void>;
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
      // 목록이 갱신되면 사라진 대본을 가리키던 선택을 반드시 정리한다.
      // 이걸 빼면 삭제된 대본이 "보이지 않는 선택"으로 남아
      // "1개 선택됨"인데 체크된 항목은 하나도 없고 시작 버튼만 활성인 상태가 되고,
      // 그 상태로 일괄 녹음을 돌리면 존재하지 않는 id 로 백엔드를 때린다.
      const liveIds = new Set(items.map((s) => s.id));
      setBatchIds((prev) => {
        const next = prev.filter((id) => liveIds.has(id));
        return next.length === prev.length ? prev : next;
      });
      setSelectedId((prev) => (prev !== null && liveIds.has(prev) ? prev : null));
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

  // 화면에 내려보내는 일괄 선택은 항상 "현재 존재하는 대본"만 담는다.
  // 개수 표시·시작 버튼 활성 조건이 전부 이 값에서 파생되므로,
  // 삭제 반영이 한 박자 늦더라도 유령 선택이 UI 로 새어 나가지 않는다.
  const validBatchIds = useMemo(() => {
    const liveIds = new Set(scripts.map((s) => s.id));
    return batchIds.filter((id) => liveIds.has(id));
  }, [scripts, batchIds]);

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
            selectedIds={validBatchIds}
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
            batchIds={validBatchIds}
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
