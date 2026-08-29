import React from 'react';
import { FolderOpen, Merge, RefreshCw } from 'lucide-react';
import type { FilesView, HistoryItem, Settings } from '../types';
import { HistoryList } from './HistoryList';
import { MediaJoiner } from './MediaJoiner';
import { AudioConverter } from './AudioConverter';
import { TabBar, type TabBarItem } from './TabBar';

interface FileStudioProps {
  settings: Settings;
  historyItems: HistoryItem[];
  isHistoryLoading: boolean;
  joinerInitialFiles: string[];
  converterInitialFiles: string[];
  onRefreshHistory: () => void;
  onDeleteFile: (path: string) => Promise<void>;
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenDefaultPlayer: (path: string) => Promise<void>;
  onSendToMerger: (paths: string[]) => void;
  onSendToConverter: (paths: string[]) => void;
  onSendToSubtitle: (audioPath: string) => void;
  /** 병합 · 변환 화면으로 전환하라는 외부 요청 */
  requestedView: FilesView | null;
  onViewHandled: () => void;
}

const TABS: TabBarItem<FilesView>[] = [
  {
    key: 'history',
    label: '히스토리',
    icon: <FolderOpen className="w-4 h-4" />,
    accent: 'bg-cyan-600 shadow-cyan-600/30',
  },
  {
    key: 'merger',
    label: '파일 연결 & 병합',
    icon: <Merge className="w-4 h-4" />,
    accent: 'bg-purple-600 shadow-purple-600/30',
  },
  {
    key: 'converter',
    label: '오디오 변환',
    icon: <RefreshCw className="w-4 h-4" />,
    accent: 'bg-teal-600 shadow-teal-600/30',
  },
];

/** 결과 파일을 다루는 화면들(히스토리 · 병합 · 변환)을 한 탭으로 묶는다. */
export const FileStudio: React.FC<FileStudioProps> = ({
  settings,
  historyItems,
  isHistoryLoading,
  joinerInitialFiles,
  converterInitialFiles,
  onRefreshHistory,
  onDeleteFile,
  onOpenExplorer,
  onOpenDefaultPlayer,
  onSendToMerger,
  onSendToConverter,
  onSendToSubtitle,
  requestedView,
  onViewHandled,
}) => {
  const [view, setView] = React.useState<FilesView>('history');

  // 히스토리에서 "병합으로 보내기" 같은 요청이 오면 해당 화면으로 전환한다.
  React.useEffect(() => {
    if (requestedView) {
      setView(requestedView);
      onViewHandled();
    }
  }, [requestedView, onViewHandled]);

  return (
    <div className="h-full min-h-0 flex flex-col">
      <div className="px-6 pt-5 shrink-0">
        <TabBar items={TABS} current={view} onSelect={setView} className="w-fit" />
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        {view === 'history' && (
          <HistoryList
            items={historyItems}
            isLoading={isHistoryLoading}
            onRefresh={onRefreshHistory}
            onDeleteFile={onDeleteFile}
            onOpenExplorer={onOpenExplorer}
            onOpenDefaultPlayer={onOpenDefaultPlayer}
            onSendToMerger={onSendToMerger}
            onSendToConverter={onSendToConverter}
            onSendToSubtitle={onSendToSubtitle}
          />
        )}

        {view === 'merger' && (
          <MediaJoiner
            settings={settings}
            initialFiles={joinerInitialFiles}
            onOpenExplorer={onOpenExplorer}
            onOpenDefaultPlayer={onOpenDefaultPlayer}
          />
        )}

        {view === 'converter' && (
          <AudioConverter
            settings={settings}
            initialFiles={converterInitialFiles}
            onOpenExplorer={onOpenExplorer}
            onOpenDefaultPlayer={onOpenDefaultPlayer}
            onNavigateToHistory={() => {
              onRefreshHistory();
              setView('history');
            }}
            onSendToSubtitle={onSendToSubtitle}
          />
        )}
      </div>
    </div>
  );
};
