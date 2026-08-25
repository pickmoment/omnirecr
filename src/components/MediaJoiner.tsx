import React, { useState, useEffect } from 'react';
import {
  Merge,
  UploadCloud,
  ArrowUp,
  ArrowDown,
  Trash2,
  Zap,
  RefreshCw,
  FolderOpen,
  CheckCircle2,
  AlertCircle,
  Plus,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import type { MediaProbeInfo, MergeProgressPayload, Settings } from '../types';

interface MediaJoinerProps {
  settings: Settings;
  initialFiles: string[];
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenDefaultPlayer: (path: string) => Promise<void>;
}

export const MediaJoiner: React.FC<MediaJoinerProps> = ({
  settings,
  initialFiles,
  onOpenExplorer,
  onOpenDefaultPlayer,
}) => {
  const [filePaths, setFilePaths] = useState<string[]>(initialFiles);
  const [probes, setProbes] = useState<MediaProbeInfo[]>([]);
  const [isProbing, setIsProbing] = useState(false);
  const [isMerging, setIsMerging] = useState(false);
  const [progress, setProgress] = useState<MergeProgressPayload | null>(null);
  const [outputFileName, setOutputFileName] = useState('');
  const [outputFormat, setOutputFormat] = useState<'mp4' | 'm4a' | 'mp3'>('mp4');
  const [mergedFilePath, setMergedFilePath] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Sync initialFiles from history or other triggers
  useEffect(() => {
    if (initialFiles.length > 0) {
      setFilePaths((prev) => Array.from(new Set([...prev, ...initialFiles])));
    }
  }, [initialFiles]);

  // Probe media files when list changes
  useEffect(() => {
    if (filePaths.length === 0) {
      setProbes([]);
      return;
    }

    let isMounted = true;
    setIsProbing(true);
    invoke<MediaProbeInfo[]>('probe_media_files', { files: filePaths })
      .then((res) => {
        if (isMounted) {
          setProbes(res);
          // Auto detect format from first file
          if (res.length > 0) {
            const first = res[0];
            if (first.file_type === 'video') {
              setOutputFormat('mp4');
            } else {
              setOutputFormat(first.format_name.includes('mp3') ? 'mp3' : 'm4a');
            }
          }
        }
      })
      .catch((err) => console.error('Probe error:', err))
      .finally(() => {
        if (isMounted) setIsProbing(false);
      });

    return () => {
      isMounted = false;
    };
  }, [filePaths]);

  // Listen to merge progress events
  useEffect(() => {
    const unlistenPromise = listen<MergeProgressPayload>('merge_progress', (event) => {
      setProgress(event.payload);
      if (event.payload.finished) {
        setIsMerging(false);
      }
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // Determine if direct copy is available
  const isDirectCopy =
    probes.length >= 2 &&
    probes.every((p) => {
      const first = probes[0];
      return (
        p.file_type === first.file_type &&
        p.video_codec === first.video_codec &&
        p.audio_codec === first.audio_codec &&
        p.width === first.width &&
        p.height === first.height &&
        p.sample_rate === first.sample_rate
      );
    });

  const totalDuration = probes.reduce((acc, p) => acc + p.duration_secs, 0);

  const moveUp = (index: number) => {
    if (index <= 0) return;
    const next = [...filePaths];
    const temp = next[index - 1];
    next[index - 1] = next[index];
    next[index] = temp;
    setFilePaths(next);
  };

  const moveDown = (index: number) => {
    if (index >= filePaths.length - 1) return;
    const next = [...filePaths];
    const temp = next[index + 1];
    next[index + 1] = next[index];
    next[index] = temp;
    setFilePaths(next);
  };

  const removeFile = (index: number) => {
    setFilePaths(filePaths.filter((_, i) => i !== index));
  };

  const handleAddFiles = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [
          {
            name: 'Media Files',
            extensions: ['mp4', 'mp3', 'm4a', 'wav', 'mov', 'mkv', 'webm'],
          },
        ],
      });

      if (selected) {
        const paths = Array.isArray(selected) ? selected : [selected];
        setFilePaths((prev) => Array.from(new Set([...prev, ...paths])));
      }
    } catch (err) {
      console.error(err);
    }
  };

  const handleStartMerge = async () => {
    if (filePaths.length < 2) {
      setErrorMsg('병합하려면 최소 2개 이상의 미디어 파일이 필요합니다.');
      return;
    }

    setErrorMsg(null);
    setIsMerging(true);
    setProgress(null);
    setMergedFilePath(null);

    const timestamp = new Date().toISOString().replace(/[-:T.]/g, '').slice(0, 14);
    const cleanName = outputFileName.trim().replace(/\.(mp4|m4a|mp3)$/i, '');
    const fname = cleanName
      ? `${cleanName}.${outputFormat}`
      : `Merged_${timestamp}.${outputFormat}`;

    const baseDir = settings.output_dir.trim().replace(/[\\/]+$/, '');
    const outputPath = baseDir ? `${baseDir}/${fname}` : fname;

    try {
      const resultPath = await invoke<string>('merge_media_files', {
        task: {
          input_files: filePaths,
          output_path: outputPath,
          output_format: outputFormat,
        },
      });

      setMergedFilePath(resultPath);
    } catch (err) {
      setErrorMsg(typeof err === 'string' ? err : '파일 병합 중 오류가 발생했습니다.');
    } finally {
      setIsMerging(false);
    }
  };

  const handleCancelMerge = async () => {
    try {
      await invoke('cancel_merge');
      setIsMerging(false);
      setProgress(null);
    } catch (err) {
      console.error(err);
    }
  };

  const formatDuration = (seconds: number) => {
    const s = Math.round(seconds);
    const hrs = Math.floor(s / 3600);
    const mins = Math.floor((s % 3600) / 60);
    const secs = s % 60;
    if (hrs > 0) {
      return `${String(hrs).padStart(2, '0')}:${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
    }
    return `${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
  };

  return (
    <div className="h-full flex flex-col p-6 space-y-5 max-w-5xl mx-auto overflow-hidden">
      {/* Top Banner & Mode Detection */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg shrink-0 space-y-3">
        <div className="flex flex-col md:flex-row md:items-center justify-between gap-3">
          <div>
            <h2 className="text-base font-bold text-slate-100 flex items-center gap-2">
              <Merge className="w-5 h-5 text-purple-400" />
              미디어 파일 연결 & 병합 (File Joiner & Merger)
            </h2>
            <p className="text-xs text-slate-400 mt-0.5">
              여러 개의 녹음/녹화 파일을 순서대로 결합하여 하나의 완성된 미디어 파일로 생성합니다.
            </p>
          </div>

          <button
            onClick={handleAddFiles}
            disabled={isMerging}
            className="flex items-center gap-2 px-4 py-2 bg-purple-600 hover:bg-purple-500 text-white rounded-xl text-xs font-bold shadow-lg shadow-purple-600/30 transition self-start md:self-auto"
          >
            <Plus className="w-4 h-4" />
            <span>파일 추가하기</span>
          </button>
        </div>

        {/* Intelligent Mode Badge */}
        {filePaths.length >= 2 && (
          <div
            className={`p-3 rounded-xl border flex items-center justify-between text-xs transition-all ${
              isDirectCopy
                ? 'bg-emerald-950/40 border-emerald-500/50 text-emerald-300'
                : 'bg-indigo-950/40 border-indigo-500/50 text-indigo-300'
            }`}
          >
            <div className="flex items-center gap-2 font-semibold">
              {isDirectCopy ? (
                <>
                  <Zap className="w-4 h-4 text-emerald-400" />
                  <span>⚡ 무손실 초고속 다이렉트 복사 (Direct Copy) 모드 활성화</span>
                </>
              ) : (
                <>
                  <RefreshCw className="w-4 h-4 text-indigo-400" />
                  <span>🔄 스마트 재인코딩 폴백 (Smart Re-encode) 모드 활성화</span>
                </>
              )}
            </div>
            <span className="text-[11px] font-mono opacity-80">
              {isDirectCopy
                ? '동일 코덱/포맷 규격 감지 (1~2초 완료)'
                : '서로 다른 해상도/코덱 자동 표준화 결합'}
            </span>
          </div>
        )}
      </div>

      {/* File Queue List */}
      <div className="flex-1 overflow-y-auto space-y-2 pr-1">
        {filePaths.length === 0 ? (
          <div
            onClick={handleAddFiles}
            className="h-56 flex flex-col items-center justify-center border-2 border-dashed border-slate-800 rounded-2xl bg-slate-900/30 hover:border-purple-500/50 hover:bg-slate-900/50 transition cursor-pointer p-6 text-center"
          >
            <UploadCloud className="w-12 h-12 text-slate-500 mb-3" />
            <p className="text-sm font-semibold text-slate-300">
              병합할 오디오 또는 동영상 파일을 이곳에 추가하세요.
            </p>
            <p className="text-xs text-slate-500 mt-1">
              [파일 추가하기] 버튼을 누르거나 히스토리 탭에서 [🔗 파일 연결]을 전송할 수 있습니다.
            </p>
          </div>
        ) : (
          filePaths.map((path, index) => {
            const probe = probes.find((p) => p.path === path);
            const fileName = path.split(/[\\/]/).pop() || path;

            return (
              <div
                key={`${path}-${index}`}
                className="p-3.5 rounded-xl bg-slate-900/80 border border-slate-800 flex items-center justify-between gap-3 shadow"
              >
                <div className="flex items-center gap-3 min-w-0">
                  <div className="w-7 h-7 rounded-lg bg-slate-950 border border-slate-800 flex items-center justify-center text-xs font-mono font-bold text-purple-400 shrink-0">
                    {index + 1}
                  </div>
                  <div className="min-w-0">
                    <div className="font-semibold text-xs text-slate-100 truncate" title={fileName}>
                      {fileName}
                    </div>
                    <div className="flex items-center gap-2 text-[11px] font-mono text-slate-400 mt-0.5">
                      {probe ? (
                        <>
                          <span className="text-purple-300 font-bold uppercase">{probe.format_name}</span>
                          <span>•</span>
                          <span>{formatDuration(probe.duration_secs)}</span>
                          {probe.width && (
                            <>
                              <span>•</span>
                              <span className="text-cyan-400">{probe.width}×{probe.height}</span>
                            </>
                          )}
                        </>
                      ) : (
                        <span>분석 중...</span>
                      )}
                    </div>
                  </div>
                </div>

                {/* Queue Controls */}
                <div className="flex items-center gap-1.5 shrink-0">
                  <button
                    onClick={() => moveUp(index)}
                    disabled={index === 0 || isMerging}
                    title="위로 이동"
                    className="p-1.5 rounded-lg bg-slate-950 border border-slate-800 text-slate-400 hover:text-white disabled:opacity-30"
                  >
                    <ArrowUp className="w-3.5 h-3.5" />
                  </button>
                  <button
                    onClick={() => moveDown(index)}
                    disabled={index === filePaths.length - 1 || isMerging}
                    title="아래로 이동"
                    className="p-1.5 rounded-lg bg-slate-950 border border-slate-800 text-slate-400 hover:text-white disabled:opacity-30"
                  >
                    <ArrowDown className="w-3.5 h-3.5" />
                  </button>
                  <button
                    onClick={() => removeFile(index)}
                    disabled={isMerging}
                    title="제거"
                    className="p-1.5 rounded-lg bg-slate-950 border border-slate-800 text-slate-500 hover:text-red-400 disabled:opacity-30"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Error Banner */}
      {errorMsg && (
        <div className="p-3 rounded-xl bg-red-950/60 border border-red-500/50 text-red-300 text-xs flex items-center gap-2">
          <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}

      {/* Progress & Merge Options Bar */}
      <div className="bg-slate-900/90 border border-slate-800 rounded-2xl p-5 shadow-2xl space-y-4 shrink-0">
        {/* Real-time Progress Bar */}
        {isMerging && progress && (
          <div className="space-y-2 p-3.5 bg-slate-950 rounded-xl border border-slate-800">
            <div className="flex items-center justify-between text-xs">
              <span className="font-semibold text-purple-300 flex items-center gap-1.5">
                <RefreshCw className="w-3.5 h-3.5 animate-spin" />
                미디어 파일 병합 인코딩 중... ({progress.percent.toFixed(1)}%)
              </span>
              <span className="font-mono text-slate-400">
                {formatDuration(progress.current_time_secs)} / {formatDuration(progress.total_time_secs)} (배속: {progress.speed})
              </span>
            </div>
            <div className="w-full h-2.5 bg-slate-900 rounded-full overflow-hidden border border-slate-800">
              <div
                className="h-full bg-gradient-to-r from-purple-500 to-cyan-400 transition-all duration-150"
                style={{ width: `${progress.percent}%` }}
              />
            </div>
          </div>
        )}

        {/* Finished Success Banner */}
        {mergedFilePath && !isMerging && (
          <div className="p-3.5 rounded-xl bg-emerald-950/60 border border-emerald-500/50 text-emerald-200 flex items-center justify-between text-xs">
            <div className="flex items-center gap-2 font-semibold">
              <CheckCircle2 className="w-4 h-4 text-emerald-400" />
              <span>병합이 성공적으로 완료되었습니다!</span>
            </div>
            <div className="flex items-center gap-2">
              <button
                onClick={() => onOpenDefaultPlayer(mergedFilePath)}
                className="px-3 py-1 bg-emerald-600 hover:bg-emerald-500 text-white rounded-lg font-bold shadow"
              >
                결과 재생
              </button>
              <button
                onClick={() => onOpenExplorer(mergedFilePath)}
                className="p-1 rounded-lg bg-emerald-950 border border-emerald-800 text-emerald-300"
                title="위치 열기"
              >
                <FolderOpen className="w-4 h-4" />
              </button>
            </div>
          </div>
        )}

        {/* Output Filename & Format Selection */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3 pt-1 border-t border-slate-800/80">
          <div className="space-y-1">
            <label className="text-xs text-slate-300 font-semibold">출력 파일명 (선택 사항)</label>
            <input
              type="text"
              placeholder="예: Final_Video (비워두면 자동 생성)"
              value={outputFileName}
              onChange={(e) => setOutputFileName(e.target.value)}
              className="w-full px-3 py-1.5 bg-slate-950 border border-slate-800 rounded-xl text-xs text-slate-200 focus:outline-none focus:border-purple-500 font-mono"
            />
          </div>

          <div className="space-y-1">
            <label className="text-xs text-slate-300 font-semibold">출력 포맷 (Container Format)</label>
            <div className="flex items-center gap-1.5">
              {(['mp4', 'm4a', 'mp3'] as const).map((fmt) => (
                <button
                  key={fmt}
                  type="button"
                  onClick={() => setOutputFormat(fmt)}
                  className={`flex-1 py-1.5 rounded-xl border text-xs font-bold uppercase transition ${
                    outputFormat === fmt
                      ? 'bg-purple-600 border-purple-500 text-white shadow'
                      : 'bg-slate-950 border-slate-800 text-slate-400 hover:text-slate-200'
                  }`}
                >
                  {fmt}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Bottom Options and Merge Trigger */}
        <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
          <div className="flex items-center gap-4 text-xs font-mono text-slate-400">
            <span>총 파일: <b className="text-white">{filePaths.length}</b>개</span>
            <span>•</span>
            <span>총 재생 시간: <b className="text-purple-400">{formatDuration(totalDuration)}</b></span>
          </div>

          <div className="flex items-center gap-3">
            {isMerging ? (
              <button
                onClick={handleCancelMerge}
                className="px-6 py-3 rounded-xl bg-red-600 hover:bg-red-500 text-white text-xs font-bold shadow"
              >
                병합 취소
              </button>
            ) : (
              <button
                disabled={filePaths.length < 2 || isProbing}
                onClick={handleStartMerge}
                className="flex items-center gap-2 px-8 py-3.5 rounded-xl bg-gradient-to-r from-purple-600 to-indigo-600 hover:from-purple-500 hover:to-indigo-500 text-white text-xs font-bold shadow-xl shadow-purple-600/30 active:scale-95 disabled:opacity-50 disabled:cursor-not-allowed transition-all"
              >
                <Merge className="w-4 h-4" />
                <span>미디어 병합 시작</span>
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
