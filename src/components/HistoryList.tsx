import React, { useState, useRef, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  FolderOpen,
  Music,
  Video,
  Play,
  Pause,
  ExternalLink,
  Trash2,
  Merge,
  Search,
  CheckSquare,
  Square,
  RefreshCw,
  Clock,
  HardDrive,
  FileVideo,
  FileAudio,
  Loader2,
} from 'lucide-react';
import type { HistoryItem } from '../types';

interface HistoryListProps {
  items: HistoryItem[];
  isLoading: boolean;
  onRefresh: () => void;
  onDeleteFile: (path: string) => Promise<void>;
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenDefaultPlayer: (path: string) => Promise<void>;
  onSendToMerger: (selectedPaths: string[]) => void;
  onSendToConverter?: (selectedPaths: string[]) => void;
}

export const HistoryList: React.FC<HistoryListProps> = ({
  items,
  isLoading,
  onRefresh,
  onDeleteFile,
  onOpenExplorer,
  onOpenDefaultPlayer,
  onSendToMerger,
  onSendToConverter,
}) => {
  const [filterType, setFilterType] = useState<'all' | 'audio' | 'video'>('all');
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // In-app Audio Preview Player State
  const [activePreview, setActivePreview] = useState<HistoryItem | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [isLoadingAudio, setIsLoadingAudio] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [previewDuration, setPreviewDuration] = useState(0);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const currentBlobUrlRef = useRef<string | null>(null);

  useEffect(() => {
    onRefresh();
  }, []);

  const filteredItems = items.filter((item) => {
    if (filterType !== 'all' && item.file_type !== filterType) {
      return false;
    }
    if (searchQuery.trim() !== '') {
      const q = searchQuery.toLowerCase();
      return item.file_name.toLowerCase().includes(q) || item.format.toLowerCase().includes(q);
    }
    return true;
  });

  const toggleSelect = (id: string) => {
    const next = new Set(selectedIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    setSelectedIds(next);
  };

  const toggleSelectAll = () => {
    if (selectedIds.size === filteredItems.length && filteredItems.length > 0) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(filteredItems.map((i) => i.id)));
    }
  };

  const handlePlayPreview = (item: HistoryItem) => {
    if (activePreview?.id === item.id) {
      if (isPlaying) {
        audioRef.current?.pause();
        setIsPlaying(false);
      } else {
        audioRef.current?.play().then(() => setIsPlaying(true)).catch(console.error);
      }
    } else {
      setActivePreview(item);
      setCurrentTime(0);
    }
  };

  useEffect(() => {
    let isCancelled = false;

    if (!activePreview) {
      if (currentBlobUrlRef.current) {
        URL.revokeObjectURL(currentBlobUrlRef.current);
        currentBlobUrlRef.current = null;
      }
      setIsPlaying(false);
      return;
    }

    setIsLoadingAudio(true);

    const ext = activePreview.format.toLowerCase();
    const mimeType = ext === 'mp3' ? 'audio/mpeg' : ext === 'wav' ? 'audio/wav' : 'audio/mp4';

    invoke<number[]>('read_audio_file', { path: activePreview.file_path })
      .then((bytes) => {
        if (isCancelled) return;

        if (currentBlobUrlRef.current) {
          URL.revokeObjectURL(currentBlobUrlRef.current);
        }

        const uint8Array = new Uint8Array(bytes);
        const blob = new Blob([uint8Array], { type: mimeType });
        const blobUrl = URL.createObjectURL(blob);
        currentBlobUrlRef.current = blobUrl;

        if (audioRef.current) {
          audioRef.current.src = blobUrl;
          audioRef.current.load();
          audioRef.current
            .play()
            .then(() => {
              if (!isCancelled) setIsPlaying(true);
            })
            .catch((err) => {
              console.warn('Auto-play blocked or error:', err);
              if (!isCancelled) setIsPlaying(false);
            });
        }
      })
      .catch((err) => {
        console.error('Failed to load audio for preview:', err);
        if (!isCancelled) setIsPlaying(false);
      })
      .finally(() => {
        if (!isCancelled) setIsLoadingAudio(false);
      });

    return () => {
      isCancelled = true;
    };
  }, [activePreview]);

  const formatTime = (secs: number) => {
    const s = Math.floor(secs);
    const m = Math.floor(s / 60);
    const sec = s % 60;
    return `${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
  };

  return (
    <div className="h-full flex flex-col p-6 space-y-4 max-w-6xl mx-auto overflow-hidden">
      {/* Top Filter & Action Bar */}
      <div className="flex flex-col md:flex-row md:items-center justify-between gap-4 bg-slate-900/80 border border-slate-800 p-4 rounded-2xl shadow-lg shrink-0">
        {/* Category Tabs */}
        <div className="flex items-center gap-1.5 bg-slate-950/70 p-1 rounded-xl border border-slate-800">
          <button
            onClick={() => setFilterType('all')}
            className={`px-3.5 py-1.5 rounded-lg text-xs font-semibold transition ${
              filterType === 'all'
                ? 'bg-blue-600 text-white shadow'
                : 'text-slate-400 hover:text-slate-200'
            }`}
          >
            전체 ({items.length})
          </button>
          <button
            onClick={() => setFilterType('audio')}
            className={`flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg text-xs font-semibold transition ${
              filterType === 'audio'
                ? 'bg-indigo-600 text-white shadow'
                : 'text-slate-400 hover:text-slate-200'
            }`}
          >
            <Music className="w-3.5 h-3.5" />
            <span>오디오 ({items.filter((i) => i.file_type === 'audio').length})</span>
          </button>
          <button
            onClick={() => setFilterType('video')}
            className={`flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg text-xs font-semibold transition ${
              filterType === 'video'
                ? 'bg-cyan-600 text-white shadow'
                : 'text-slate-400 hover:text-slate-200'
            }`}
          >
            <Video className="w-3.5 h-3.5" />
            <span>동영상 ({items.filter((i) => i.file_type === 'video').length})</span>
          </button>
        </div>

        {/* Search & Actions */}
        <div className="flex items-center gap-2.5">
          <div className="relative">
            <Search className="w-4 h-4 text-slate-400 absolute left-3 top-2.5" />
            <input
              type="text"
              placeholder="파일 검색..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9 pr-3 py-1.5 bg-slate-950 border border-slate-800 rounded-xl text-xs text-slate-200 focus:outline-none focus:border-blue-500 w-48"
            />
          </div>

          <button
            onClick={onRefresh}
            title="새로고침"
            className="p-2 rounded-xl bg-slate-950 border border-slate-800 text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition"
          >
            <RefreshCw className={`w-4 h-4 ${isLoading ? 'animate-spin text-blue-400' : ''}`} />
          </button>

          {/* Transfer Buttons */}
          {selectedIds.size > 0 && (
            <div className="flex items-center gap-2">
              {onSendToConverter && (
                <button
                  onClick={() => onSendToConverter(Array.from(selectedIds))}
                  className="flex items-center gap-1.5 px-3 py-2 rounded-xl bg-teal-600 hover:bg-teal-500 text-white text-xs font-bold shadow-lg shadow-teal-600/30 transition animate-fade-in"
                >
                  <RefreshCw className="w-3.5 h-3.5" />
                  <span>변환기로 전송 ({selectedIds.size})</span>
                </button>
              )}
              <button
                onClick={() => onSendToMerger(Array.from(selectedIds))}
                className="flex items-center gap-1.5 px-3 py-2 rounded-xl bg-purple-600 hover:bg-purple-500 text-white text-xs font-bold shadow-lg shadow-purple-600/30 transition animate-fade-in"
              >
                <Merge className="w-3.5 h-3.5" />
                <span>파일 연결 전송 ({selectedIds.size})</span>
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Select All Bar */}
      {filteredItems.length > 0 && (
        <div className="flex items-center justify-between px-2 text-xs text-slate-400 shrink-0">
          <button
            onClick={toggleSelectAll}
            className="flex items-center gap-1.5 hover:text-slate-200 transition font-medium"
          >
            {selectedIds.size === filteredItems.length ? (
              <CheckSquare className="w-4 h-4 text-blue-400" />
            ) : (
              <Square className="w-4 h-4 text-slate-600" />
            )}
            <span>전체 선택 ({selectedIds.size} / {filteredItems.length})</span>
          </button>
          <span>Shift / Ctrl 다중 선택 지원</span>
        </div>
      )}

      {/* History File Items List */}
      <div className="flex-1 overflow-y-auto space-y-2.5 pr-1">
        {filteredItems.length === 0 ? (
          <div className="h-64 flex flex-col items-center justify-center text-slate-500 border border-dashed border-slate-800 rounded-2xl bg-slate-900/30">
            <FolderOpen className="w-12 h-12 mb-3 opacity-40" />
            <p className="text-sm font-semibold">녹화 및 녹음된 미디어 파일이 없습니다.</p>
            <p className="text-xs text-slate-600 mt-1">상단 탭에서 화면 녹화 또는 오디오 녹음을 시작해 보세요.</p>
          </div>
        ) : (
          filteredItems.map((item) => {
            const isSelected = selectedIds.has(item.id);
            const isVideo = item.file_type === 'video';
            const isThisAudioPlaying = activePreview?.id === item.id && isPlaying;

            return (
              <div
                key={item.id}
                onClick={(e) => {
                  if ((e.target as HTMLElement).tagName !== 'BUTTON' && (e.target as HTMLElement).tagName !== 'INPUT') {
                    toggleSelect(item.id);
                  }
                }}
                className={`p-4 rounded-2xl border transition-all duration-150 flex flex-col md:flex-row md:items-center justify-between gap-4 cursor-pointer select-none ${
                  isSelected
                    ? 'bg-blue-950/30 border-blue-500/60 shadow-md shadow-blue-500/10'
                    : 'bg-slate-900/80 border-slate-800 hover:bg-slate-850 hover:border-slate-700'
                }`}
              >
                {/* Left: Checkbox + Icon + Details */}
                <div className="flex items-center gap-3.5 min-w-0">
                  <input
                    type="checkbox"
                    checked={isSelected}
                    onChange={() => toggleSelect(item.id)}
                    className="w-4 h-4 rounded bg-slate-800 border-slate-700 text-blue-500 focus:ring-0 cursor-pointer shrink-0"
                  />

                  <div
                    className={`w-11 h-11 rounded-xl flex items-center justify-center shrink-0 border ${
                      isVideo
                        ? 'bg-cyan-950/60 border-cyan-500/40 text-cyan-400'
                        : 'bg-indigo-950/60 border-indigo-500/40 text-indigo-400'
                    }`}
                  >
                    {isVideo ? <FileVideo className="w-5 h-5" /> : <FileAudio className="w-5 h-5" />}
                  </div>

                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-semibold text-sm text-slate-100 truncate" title={item.file_name}>
                        {item.file_name}
                      </span>
                      <span
                        className={`text-[10px] font-mono uppercase font-bold px-1.5 py-0.5 rounded border ${
                          isVideo
                            ? 'bg-cyan-950/80 border-cyan-800/60 text-cyan-300'
                            : 'bg-indigo-950/80 border-indigo-800/60 text-indigo-300'
                        }`}
                      >
                        {item.format}
                      </span>
                    </div>

                    <div className="flex items-center gap-3 text-xs text-slate-400 mt-1 font-mono">
                      <span className="flex items-center gap-1">
                        <Clock className="w-3.5 h-3.5 text-slate-500" />
                        {item.duration_formatted}
                      </span>
                      <span>•</span>
                      <span className="flex items-center gap-1">
                        <HardDrive className="w-3.5 h-3.5 text-slate-500" />
                        {item.size_formatted}
                      </span>
                      {item.resolution && (
                        <>
                          <span>•</span>
                          <span className="text-cyan-400 font-semibold">{item.resolution}</span>
                        </>
                      )}
                      <span>•</span>
                      <span className="text-slate-500 text-[11px]">{item.created_at}</span>
                    </div>
                  </div>
                </div>

                {/* Right: Actions */}
                <div className="flex items-center gap-2 shrink-0 self-end md:self-center">
                  {/* Audio Preview Button */}
                  {!isVideo && (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        handlePlayPreview(item);
                      }}
                      title="앱 내 미리듣기"
                      className={`p-2 rounded-xl border text-xs flex items-center gap-1.5 transition ${
                        activePreview?.id === item.id
                          ? 'bg-indigo-600 text-white border-indigo-500 shadow-md shadow-indigo-600/30'
                          : 'bg-slate-950 border-slate-800 text-indigo-400 hover:bg-slate-800'
                      }`}
                    >
                      {activePreview?.id === item.id && isLoadingAudio ? (
                        <Loader2 className="w-3.5 h-3.5 animate-spin" />
                      ) : isThisAudioPlaying ? (
                        <Pause className="w-3.5 h-3.5" />
                      ) : (
                        <Play className="w-3.5 h-3.5" />
                      )}
                      <span>{activePreview?.id === item.id && isPlaying ? '일시정지' : '미리듣기'}</span>
                    </button>
                  )}

                  {/* Send to Converter Button */}
                  {!isVideo && onSendToConverter && (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onSendToConverter([item.file_path]);
                      }}
                      title="MP3 / M4A로 변환"
                      className="p-2 rounded-xl bg-slate-950 border border-slate-800 text-teal-400 hover:text-white hover:bg-teal-900/40 hover:border-teal-700/60 transition flex items-center gap-1.5 text-xs"
                    >
                      <RefreshCw className="w-3.5 h-3.5 text-teal-400" />
                      <span>변환</span>
                    </button>
                  )}

                  {/* Open in Default Player */}
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenDefaultPlayer(item.file_path);
                    }}
                    title="외부 플레이어로 열기"
                    className="p-2 rounded-xl bg-slate-950 border border-slate-800 text-slate-300 hover:text-white hover:bg-slate-800 transition flex items-center gap-1.5 text-xs"
                  >
                    <ExternalLink className="w-3.5 h-3.5 text-blue-400" />
                    <span>열기</span>
                  </button>

                  {/* Show in Explorer */}
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenExplorer(item.file_path);
                    }}
                    title="탐색기에서 위치 열기"
                    className="p-2 rounded-xl bg-slate-950 border border-slate-800 text-slate-300 hover:text-white hover:bg-slate-800 transition"
                  >
                    <FolderOpen className="w-3.5 h-3.5 text-amber-400" />
                  </button>

                  {/* Delete */}
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      if (confirm(`'${item.file_name}' 파일을 삭제하시겠습니까?`)) {
                        onDeleteFile(item.file_path);
                      }
                    }}
                    title="파일 삭제"
                    className="p-2 rounded-xl bg-slate-950 border border-slate-800 text-slate-500 hover:text-red-400 hover:border-red-900/60 hover:bg-red-950/30 transition"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* In-app Audio Preview Player Bar */}
      {activePreview && (
        <div className="bg-indigo-950/80 border border-indigo-500/40 rounded-2xl p-3.5 shadow-2xl flex items-center justify-between gap-4 shrink-0 animate-fade-in">
          <audio
            ref={audioRef}
            onTimeUpdate={(e) => setCurrentTime((e.target as HTMLAudioElement).currentTime)}
            onLoadedMetadata={(e) => setPreviewDuration((e.target as HTMLAudioElement).duration)}
            onEnded={() => setIsPlaying(false)}
          />

          <div className="flex items-center gap-3 min-w-0">
            <button
              onClick={() => {
                if (isPlaying) {
                  audioRef.current?.pause();
                  setIsPlaying(false);
                } else {
                  audioRef.current?.play();
                  setIsPlaying(true);
                }
              }}
              className="w-10 h-10 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white flex items-center justify-center shadow-md shrink-0"
            >
              {isPlaying ? <Pause className="w-5 h-5 fill-current" /> : <Play className="w-5 h-5 fill-current" />}
            </button>

            <div className="min-w-0">
              <div className="font-semibold text-xs text-white truncate">{activePreview.file_name}</div>
              <div className="text-[11px] font-mono text-indigo-300">
                {formatTime(currentTime)} / {formatTime(previewDuration || activePreview.duration_secs)}
              </div>
            </div>
          </div>

          {/* Scrubber */}
          <input
            type="range"
            min="0"
            max={previewDuration || activePreview.duration_secs || 100}
            value={currentTime}
            onChange={(e) => {
              const t = parseFloat(e.target.value);
              setCurrentTime(t);
              if (audioRef.current) {
                audioRef.current.currentTime = t;
              }
            }}
            className="flex-1 h-1.5 bg-slate-900 rounded-lg appearance-none cursor-pointer accent-indigo-400"
          />

          <button
            onClick={() => {
              audioRef.current?.pause();
              setActivePreview(null);
              setIsPlaying(false);
            }}
            className="text-xs text-slate-400 hover:text-slate-200 px-2 py-1"
          >
            닫기
          </button>
        </div>
      )}
    </div>
  );
};
