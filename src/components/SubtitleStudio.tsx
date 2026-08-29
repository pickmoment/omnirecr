import React, { useCallback, useEffect, useState } from 'react';
import { FileText, ListChecks } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import type { ScriptItem, Settings } from '../types';
import { SubtitleGenerator } from './SubtitleGenerator';
import { SubtitleBatchRunner } from './SubtitleBatchRunner';

type SubtitleView = 'editor' | 'batch';

interface SubtitleStudioProps {
  settings: Settings;
  initialAudioPath?: string | null;
  initialScriptText?: string | null;
  onOpenExplorer: (path: string) => Promise<void>;
  onSettingsChange: (partial: Partial<Settings>) => Promise<void>;
  onOpenSettings: () => void;
}

/**
 * 자막 관련 화면을 한 탭에 모은 컨테이너.
 * - 편집기: 파일 하나를 열어 자막을 만들고 타임코드를 손보는 화면
 * - 일괄 생성: 대본에 연결된 녹음 파일들의 자막을 한 번에 생성
 */
export const SubtitleStudio: React.FC<SubtitleStudioProps> = ({
  settings,
  initialAudioPath,
  initialScriptText,
  onOpenExplorer,
  onSettingsChange,
  onOpenSettings,
}) => {
  const [view, setView] = useState<SubtitleView>('editor');
  const [scripts, setScripts] = useState<ScriptItem[]>([]);

  const refreshScripts = useCallback(async () => {
    try {
      setScripts(await invoke<ScriptItem[]>('list_scripts'));
    } catch (err) {
      console.error('대본 목록을 불러오지 못했습니다:', err);
    }
  }, []);

  useEffect(() => {
    refreshScripts();
  }, [refreshScripts]);

  // 자막 생성기로 파일이 전달되면 편집기 화면으로 돌아온다.
  useEffect(() => {
    if (initialAudioPath) setView('editor');
  }, [initialAudioPath]);

  const tabs: { key: SubtitleView; label: string; icon: React.ReactNode }[] = [
    { key: 'editor', label: '자막 편집기', icon: <FileText className="w-4 h-4" /> },
    { key: 'batch', label: '대본 일괄 생성', icon: <ListChecks className="w-4 h-4" /> },
  ];

  return (
    <div className="h-full min-h-0 flex flex-col">
      <div className="px-6 pt-5 shrink-0">
        <div className="flex items-center gap-1.5 bg-slate-950/60 p-1 rounded-xl border border-slate-800 self-start w-fit">
          {tabs.map((tab) => (
            <button
              key={tab.key}
              onClick={() => {
                setView(tab.key);
                if (tab.key === 'batch') refreshScripts();
              }}
              className={`flex items-center gap-2 px-4 py-2 rounded-lg text-xs font-semibold transition-all duration-200 ${
                view === tab.key
                  ? 'bg-amber-600 text-white shadow-md shadow-amber-600/30'
                  : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
              }`}
            >
              {tab.icon}
              <span>{tab.label}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        {view === 'editor' ? (
          <SubtitleGenerator
            settings={settings}
            initialAudioPath={initialAudioPath}
            initialScriptText={initialScriptText}
            onOpenExplorer={onOpenExplorer}
            onSettingsChange={onSettingsChange}
          />
        ) : (
          <div className="p-6 pt-4 max-w-5xl mx-auto w-full">
            <SubtitleBatchRunner
              settings={settings}
              scripts={scripts}
              onUpdateSettings={onSettingsChange}
              onOpenExplorer={onOpenExplorer}
              onOpenSettings={onOpenSettings}
            />
          </div>
        )}
      </div>
    </div>
  );
};
