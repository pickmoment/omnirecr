import React, { useState, useEffect } from 'react';
import {
  FileAudio,
  UploadCloud,
  Trash2,
  RefreshCw,
  FolderOpen,
  CheckCircle2,
  AlertCircle,
  Plus,
  Play,
  Settings2,
  ArrowRight,
  Sparkles,
  Music2,
  Check,
  Loader2,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import type {
  MediaProbeInfo,
  AudioConvertProgressPayload,
  AudioConvertTaskPayload,
  Settings,
} from '../types';

interface AudioConverterProps {
  settings: Settings;
  initialFiles: string[];
  onOpenExplorer: (path: string) => Promise<void>;
  onOpenDefaultPlayer: (path: string) => Promise<void>;
  onNavigateToHistory: () => void;
}

export const AudioConverter: React.FC<AudioConverterProps> = ({
  settings,
  initialFiles,
  onOpenExplorer,
  onOpenDefaultPlayer,
  onNavigateToHistory,
}) => {
  const [filePaths, setFilePaths] = useState<string[]>(initialFiles);
  const [probes, setProbes] = useState<MediaProbeInfo[]>([]);
  const [isProbing, setIsProbing] = useState(false);
  const [isConverting, setIsConverting] = useState(false);
  const [progress, setProgress] = useState<AudioConvertProgressPayload | null>(null);

  // Conversion options
  const [targetFormat, setTargetFormat] = useState<'mp3' | 'm4a'>('mp3');
  const [bitrate, setBitrate] = useState<number>(settings.audio_bitrate || 256);
  const [sampleRate, setSampleRate] = useState<number | 0>(0); // 0 = Keep Original
  const [channels, setChannels] = useState<number | 0>(0); // 0 = Keep Original
  const [outputLocationMode, setOutputLocationMode] = useState<'same' | 'default' | 'custom'>('same');
  const [customOutputDir, setCustomOutputDir] = useState<string>('');

  // Results
  const [convertedPaths, setConvertedPaths] = useState<string[]>([]);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Sync initialFiles from external triggers (e.g. from history)
  useEffect(() => {
    if (initialFiles.length > 0) {
      setFilePaths((prev) => Array.from(new Set([...prev, ...initialFiles])));
    }
  }, [initialFiles]);

  // Probe media files whenever list changes
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

  // Listen to conversion progress events
  useEffect(() => {
    const unlistenPromise = listen<AudioConvertProgressPayload>('conversion_progress', (event) => {
      setProgress(event.payload);
      if (event.payload.finished) {
        setIsConverting(false);
      }
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  const handleSelectFiles = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [
          {
            name: 'WAV & 오디오 파일',
            extensions: ['wav', 'wave', 'm4a', 'mp3', 'flac', 'aac', 'ogg', 'wma', 'aiff'],
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

  const handleSelectCustomDir = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        defaultPath: settings.output_dir || undefined,
      });

      if (selected && typeof selected === 'string') {
        setCustomOutputDir(selected);
      }
    } catch (err) {
      console.error(err);
    }
  };

  const removeFile = (index: number) => {
    setFilePaths((prev) => prev.filter((_, i) => i !== index));
  };

  const clearAll = () => {
    setFilePaths([]);
    setProbes([]);
    setProgress(null);
    setConvertedPaths([]);
    setErrorMsg(null);
  };

  const handleStartConvert = async () => {
    if (filePaths.length === 0) return;

    setIsConverting(true);
    setErrorMsg(null);
    setProgress(null);
    setConvertedPaths([]);

    let outDir: string | null = null;
    if (outputLocationMode === 'default') {
      outDir = settings.output_dir || null;
    } else if (outputLocationMode === 'custom') {
      outDir = customOutputDir || null;
    }

    const payload: AudioConvertTaskPayload = {
      input_files: filePaths,
      target_format: targetFormat,
      bitrate,
      sample_rate: sampleRate > 0 ? sampleRate : null,
      channels: channels > 0 ? channels : null,
      output_dir: outDir,
    };

    try {
      const results = await invoke<string[]>('convert_audio_files', { task: payload });
      setConvertedPaths(results);
    } catch (err) {
      console.error('Convert failed:', err);
      setErrorMsg(typeof err === 'string' ? err : '오디오 변환 중 오류가 발생했습니다.');
    } finally {
      setIsConverting(false);
    }
  };

  const handleCancelConvert = async () => {
    try {
      await invoke('cancel_conversion');
    } catch (err) {
      console.error('Cancel error:', err);
    }
  };

  const formatDuration = (secs: number) => {
    if (!secs || isNaN(secs)) return '0:00';
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${String(s).padStart(2, '0')}`;
  };

  const formatBytes = (bytes: number) => {
    if (bytes <= 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${(bytes / Math.pow(k, i)).toFixed(i > 1 ? 2 : 1)} ${sizes[i]}`;
  };

  return (
    <div className="h-full flex flex-col p-6 space-y-6 overflow-y-auto max-w-5xl mx-auto">
      {/* Header Banner */}
      <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg flex flex-col md:flex-row items-start md:items-center justify-between gap-4">
        <div className="flex items-center gap-3.5">
          <div className="w-12 h-12 rounded-2xl bg-gradient-to-br from-emerald-500 via-teal-600 to-cyan-500 flex items-center justify-center text-white shadow-lg shadow-teal-500/20 shrink-0">
            <RefreshCw className="w-6 h-6" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h1 className="text-lg font-extrabold text-white">
                WAV 오디오 포맷 변환기 (Audio Converter)
              </h1>
              <span className="text-[10px] uppercase font-semibold px-2 py-0.5 rounded-full bg-teal-500/20 text-teal-300 border border-teal-500/30">
                WAV ➔ MP3 / M4A
              </span>
            </div>
            <p className="text-xs text-slate-400 mt-0.5">
              WAV 무손실 오디오 파일을 고품질 MP3 또는 고효율 M4A(AAC)로 빠르고 손실 없이 일괄 변환합니다.
            </p>
          </div>
        </div>

        <button
          onClick={handleSelectFiles}
          disabled={isConverting}
          className="flex items-center gap-2 px-4 py-2.5 rounded-xl bg-gradient-to-r from-teal-500 to-emerald-600 hover:from-teal-400 hover:to-emerald-500 text-white text-xs font-bold shadow-md shadow-teal-500/25 transition shrink-0 disabled:opacity-50"
        >
          <Plus className="w-4 h-4" />
          <span>WAV / 오디오 파일 추가</span>
        </button>
      </div>

      {/* Main Grid: File List & Conversion Options */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-6">
        {/* Left Column: File Queue (7 cols) */}
        <div className="lg:col-span-7 flex flex-col space-y-4">
          <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg flex flex-col flex-1">
            <div className="flex items-center justify-between pb-3 border-b border-slate-800">
              <div className="flex items-center gap-2">
                <FileAudio className="w-4 h-4 text-teal-400" />
                <span className="text-xs font-bold text-slate-200">
                  변환 대상 파일 목록 ({filePaths.length}개)
                </span>
                {isProbing && (
                  <Loader2 className="w-3.5 h-3.5 animate-spin text-teal-400" />
                )}
              </div>
              {filePaths.length > 0 && (
                <button
                  onClick={clearAll}
                  disabled={isConverting}
                  className="text-xs text-rose-400 hover:text-rose-300 transition flex items-center gap-1 disabled:opacity-50"
                >
                  <Trash2 className="w-3.5 h-3.5" />
                  <span>목록 비우기</span>
                </button>
              )}
            </div>

            {/* Empty State */}
            {filePaths.length === 0 ? (
              <div
                onClick={handleSelectFiles}
                className="my-6 border-2 border-dashed border-slate-700/80 hover:border-teal-500/60 rounded-2xl p-8 flex flex-col items-center justify-center text-center cursor-pointer transition bg-slate-950/40 hover:bg-teal-950/10 group"
              >
                <div className="w-14 h-14 rounded-2xl bg-slate-800/80 group-hover:bg-teal-600/20 text-slate-400 group-hover:text-teal-400 flex items-center justify-center transition mb-3">
                  <UploadCloud className="w-7 h-7" />
                </div>
                <h3 className="text-sm font-bold text-slate-300 group-hover:text-white transition">
                  변환할 WAV 또는 오디오 파일을 선택하세요
                </h3>
                <p className="text-xs text-slate-500 mt-1 max-w-sm">
                  클릭하여 컴퓨터의 .wav 파일을 선택하거나 여러 개의 파일을 한 번에 추가할 수 있습니다.
                </p>
                <span className="mt-3 text-[11px] font-semibold text-teal-400 bg-teal-950/60 border border-teal-800/40 px-3 py-1 rounded-full">
                  .wav, .m4a, .mp3, .flac, .aac 지원
                </span>
              </div>
            ) : (
              <div className="space-y-2.5 mt-3 max-h-[380px] overflow-y-auto pr-1">
                {filePaths.map((filePath, index) => {
                  const probe = probes.find((p) => p.path === filePath);
                  const fileName = filePath.split(/[/\\]/).pop() || filePath;
                  const ext = fileName.split('.').pop()?.toUpperCase() || 'AUDIO';
                  const isWav = ext === 'WAV' || ext === 'WAVE';

                  return (
                    <div
                      key={filePath}
                      className="bg-slate-950/60 border border-slate-800/80 hover:border-slate-700 rounded-xl p-3 flex items-center justify-between gap-3 transition"
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <span className="text-xs font-mono font-bold text-slate-500 w-5 text-right shrink-0">
                          {index + 1}
                        </span>
                        <div
                          className={`w-9 h-9 rounded-lg flex items-center justify-center shrink-0 font-bold text-xs ${
                            isWav
                              ? 'bg-amber-500/20 text-amber-300 border border-amber-500/30'
                              : 'bg-teal-500/20 text-teal-300 border border-teal-500/30'
                          }`}
                        >
                          {ext}
                        </div>
                        <div className="min-w-0">
                          <p className="text-xs font-semibold text-slate-200 truncate" title={filePath}>
                            {fileName}
                          </p>
                          <div className="flex items-center gap-2 text-[11px] text-slate-400 mt-0.5">
                            {probe ? (
                              <>
                                <span>{formatDuration(probe.duration_secs)}</span>
                                <span>•</span>
                                <span>{formatBytes(probe.size_bytes)}</span>
                                {probe.sample_rate && (
                                  <>
                                    <span>•</span>
                                    <span>{(probe.sample_rate / 1000).toFixed(1)} kHz</span>
                                  </>
                                )}
                                {probe.channels && (
                                  <>
                                    <span>•</span>
                                    <span>{probe.channels === 1 ? 'Mono' : 'Stereo'}</span>
                                  </>
                                )}
                              </>
                            ) : (
                              <span className="text-slate-500">분석 중...</span>
                            )}
                          </div>
                        </div>
                      </div>

                      <div className="flex items-center gap-1.5 shrink-0">
                        <button
                          onClick={() => removeFile(index)}
                          disabled={isConverting}
                          title="삭제"
                          className="p-1.5 rounded-lg text-slate-400 hover:text-rose-400 hover:bg-slate-800/60 transition disabled:opacity-50"
                        >
                          <Trash2 className="w-4 h-4" />
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>

        {/* Right Column: Settings & Actions (5 cols) */}
        <div className="lg:col-span-5 flex flex-col space-y-4">
          <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-5 shadow-lg space-y-5">
            <div className="flex items-center gap-2 pb-3 border-b border-slate-800">
              <Settings2 className="w-4 h-4 text-teal-400" />
              <h2 className="text-xs font-bold text-slate-200">변환 인코딩 설정 (Encoding Settings)</h2>
            </div>

            {/* 1. Target Format Selection */}
            <div className="space-y-2">
              <label className="text-xs font-bold text-slate-300 flex items-center justify-between">
                <span>출력 포맷 (Target Format)</span>
                <span className="text-[10px] text-teal-400 font-mono">인코더 설정</span>
              </label>

              <div className="grid grid-cols-2 gap-3">
                {/* MP3 Option */}
                <button
                  type="button"
                  onClick={() => setTargetFormat('mp3')}
                  className={`p-3.5 rounded-xl border text-left transition relative ${
                    targetFormat === 'mp3'
                      ? 'bg-teal-950/40 border-teal-500 text-teal-200 shadow-md shadow-teal-950/50'
                      : 'bg-slate-950/40 border-slate-800 text-slate-400 hover:border-slate-700'
                  }`}
                >
                  {targetFormat === 'mp3' && (
                    <div className="absolute top-2 right-2 w-4 h-4 rounded-full bg-teal-500 text-slate-950 flex items-center justify-center">
                      <Check className="w-3 h-3 stroke-[3]" />
                    </div>
                  )}
                  <div className="font-bold text-sm text-slate-200">MP3</div>
                  <div className="text-[11px] text-slate-400 mt-1">
                    libmp3lame • 범용 호환성 최고 (모든 기기/플레이어)
                  </div>
                </button>

                {/* M4A Option */}
                <button
                  type="button"
                  onClick={() => setTargetFormat('m4a')}
                  className={`p-3.5 rounded-xl border text-left transition relative ${
                    targetFormat === 'm4a'
                      ? 'bg-teal-950/40 border-teal-500 text-teal-200 shadow-md shadow-teal-950/50'
                      : 'bg-slate-950/40 border-slate-800 text-slate-400 hover:border-slate-700'
                  }`}
                >
                  {targetFormat === 'm4a' && (
                    <div className="absolute top-2 right-2 w-4 h-4 rounded-full bg-teal-500 text-slate-950 flex items-center justify-center">
                      <Check className="w-3 h-3 stroke-[3]" />
                    </div>
                  )}
                  <div className="font-bold text-sm text-slate-200">M4A (AAC)</div>
                  <div className="text-[11px] text-slate-400 mt-1">
                    AAC • 고효율 고음질 (Apple & 모던 웹 최적)
                  </div>
                </button>
              </div>
            </div>

            {/* 2. Bitrate Selection */}
            <div className="space-y-2">
              <label className="text-xs font-bold text-slate-300 flex items-center justify-between">
                <span>오디오 비트레이트 (Bitrate)</span>
                <span className="text-[10px] text-slate-400 font-mono">{bitrate} kbps</span>
              </label>

              <div className="grid grid-cols-4 gap-2">
                {[
                  { val: 128, label: '128k', desc: '표준' },
                  { val: 192, label: '192k', desc: '고음질' },
                  { val: 256, label: '256k', desc: '추천' },
                  { val: 320, label: '320k', desc: '최고' },
                ].map((b) => (
                  <button
                    key={b.val}
                    type="button"
                    onClick={() => setBitrate(b.val)}
                    className={`py-2 px-2 rounded-xl text-center border transition ${
                      bitrate === b.val
                        ? 'bg-teal-600 text-white font-bold border-teal-500 shadow-md shadow-teal-600/30'
                        : 'bg-slate-950/40 border-slate-800 text-slate-400 hover:text-slate-200 hover:border-slate-700'
                    }`}
                  >
                    <div className="text-xs font-mono">{b.label}</div>
                    <div className="text-[10px] opacity-80">{b.desc}</div>
                  </button>
                ))}
              </div>
            </div>

            {/* 3. Advanced (Sample Rate & Channels) */}
            <div className="grid grid-cols-2 gap-3 pt-1">
              <div className="space-y-1.5">
                <label className="text-xs font-bold text-slate-300">샘플링 레이트</label>
                <select
                  value={sampleRate}
                  onChange={(e) => setSampleRate(Number(e.target.value))}
                  className="w-full bg-slate-950/60 border border-slate-800 rounded-xl px-3 py-2 text-xs text-slate-200 focus:outline-none focus:border-teal-500"
                >
                  <option value={0}>원본 유지 (Auto)</option>
                  <option value={44100}>44.1 kHz (CD Audio)</option>
                  <option value={48000}>48.0 kHz (Studio)</option>
                </select>
              </div>

              <div className="space-y-1.5">
                <label className="text-xs font-bold text-slate-300">오디오 채널</label>
                <select
                  value={channels}
                  onChange={(e) => setChannels(Number(e.target.value))}
                  className="w-full bg-slate-950/60 border border-slate-800 rounded-xl px-3 py-2 text-xs text-slate-200 focus:outline-none focus:border-teal-500"
                >
                  <option value={0}>원본 유지 (Auto)</option>
                  <option value={2}>Stereo (2채널)</option>
                  <option value={1}>Mono (1채널)</option>
                </select>
              </div>
            </div>

            {/* 4. Output Location */}
            <div className="space-y-2 pt-1 border-t border-slate-800">
              <label className="text-xs font-bold text-slate-300">저장 위치</label>
              <div className="space-y-2">
                <div className="grid grid-cols-3 gap-2">
                  <button
                    type="button"
                    onClick={() => setOutputLocationMode('same')}
                    className={`py-1.5 px-2 rounded-lg text-center border text-[11px] transition ${
                      outputLocationMode === 'same'
                        ? 'bg-teal-950/60 border-teal-500 text-teal-300 font-bold'
                        : 'bg-slate-950/40 border-slate-800 text-slate-400 hover:border-slate-700'
                    }`}
                  >
                    원본과 같은 폴더
                  </button>
                  <button
                    type="button"
                    onClick={() => setOutputLocationMode('default')}
                    className={`py-1.5 px-2 rounded-lg text-center border text-[11px] transition ${
                      outputLocationMode === 'default'
                        ? 'bg-teal-950/60 border-teal-500 text-teal-300 font-bold'
                        : 'bg-slate-950/40 border-slate-800 text-slate-400 hover:border-slate-700'
                    }`}
                  >
                    녹음 기본 폴더
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setOutputLocationMode('custom');
                      if (!customOutputDir) handleSelectCustomDir();
                    }}
                    className={`py-1.5 px-2 rounded-lg text-center border text-[11px] transition ${
                      outputLocationMode === 'custom'
                        ? 'bg-teal-950/60 border-teal-500 text-teal-300 font-bold'
                        : 'bg-slate-950/40 border-slate-800 text-slate-400 hover:border-slate-700'
                    }`}
                  >
                    직접 폴더 지정
                  </button>
                </div>

                {outputLocationMode === 'custom' && (
                  <div className="flex items-center gap-2 mt-1">
                    <input
                      type="text"
                      readOnly
                      placeholder="저장할 폴더를 선택하세요..."
                      value={customOutputDir}
                      className="flex-1 bg-slate-950/60 border border-slate-800 rounded-xl px-3 py-1.5 text-xs text-slate-300 truncate"
                    />
                    <button
                      type="button"
                      onClick={handleSelectCustomDir}
                      className="px-3 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold shrink-0"
                    >
                      찾아보기
                    </button>
                  </div>
                )}
              </div>
            </div>

            {/* Convert Action Button */}
            <div className="pt-2">
              {!isConverting ? (
                <button
                  onClick={handleStartConvert}
                  disabled={filePaths.length === 0}
                  className="w-full py-3.5 rounded-xl bg-gradient-to-r from-teal-500 via-emerald-500 to-cyan-500 hover:from-teal-400 hover:to-cyan-400 text-white font-bold text-sm shadow-lg shadow-teal-500/25 transition duration-200 flex items-center justify-center gap-2 disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
                >
                  <Sparkles className="w-4 h-4" />
                  <span>
                    {filePaths.length > 0
                      ? `${filePaths.length}개 파일 ${targetFormat.toUpperCase()}로 변환 시작`
                      : '변환할 파일을 추가해주세요'}
                  </span>
                </button>
              ) : (
                <button
                  onClick={handleCancelConvert}
                  className="w-full py-3.5 rounded-xl bg-rose-600 hover:bg-rose-500 text-white font-bold text-sm shadow-lg shadow-rose-600/30 transition duration-200 flex items-center justify-center gap-2 cursor-pointer"
                >
                  <Trash2 className="w-4 h-4" />
                  <span>변환 작업 취소</span>
                </button>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* Progress & Status Card (Shown while converting) */}
      {isConverting && progress && (
        <div className="bg-slate-900/90 border border-teal-500/40 rounded-2xl p-5 shadow-xl space-y-3 animate-fade-in">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2.5">
              <div className="w-8 h-8 rounded-xl bg-teal-500/20 text-teal-400 flex items-center justify-center animate-spin">
                <RefreshCw className="w-4 h-4" />
              </div>
              <div>
                <h3 className="text-xs font-bold text-white flex items-center gap-2">
                  <span>
                    변환 진행 중 ({progress.file_index + 1} / {progress.total_files})
                  </span>
                  <span className="text-[11px] text-teal-400 font-mono">{progress.speed}</span>
                </h3>
                <p className="text-[11px] text-slate-400 truncate max-w-lg">
                  {progress.current_file_name} ➔ {targetFormat.toUpperCase()}
                </p>
              </div>
            </div>
            <div className="text-right">
              <span className="text-lg font-mono font-extrabold text-teal-300">
                {progress.overall_percent.toFixed(1)}%
              </span>
            </div>
          </div>

          {/* Progress Bar */}
          <div className="w-full bg-slate-950 rounded-full h-3 overflow-hidden p-0.5 border border-slate-800">
            <div
              className="bg-gradient-to-r from-teal-500 via-emerald-400 to-cyan-400 h-full rounded-full transition-all duration-200 shadow-sm shadow-teal-500"
              style={{ width: `${Math.max(2, progress.overall_percent)}%` }}
            />
          </div>
        </div>
      )}

      {/* Error Message */}
      {errorMsg && (
        <div className="bg-rose-950/60 border border-rose-500/40 rounded-2xl p-4 flex items-center gap-3 text-rose-300 text-xs">
          <AlertCircle className="w-5 h-5 shrink-0 text-rose-400" />
          <span>{errorMsg}</span>
        </div>
      )}

      {/* Success / Result Card */}
      {convertedPaths.length > 0 && !isConverting && (
        <div className="bg-emerald-950/40 border border-emerald-500/40 rounded-2xl p-5 shadow-xl space-y-4 animate-fade-in">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2.5">
              <div className="w-9 h-9 rounded-xl bg-emerald-500/20 text-emerald-400 flex items-center justify-center">
                <CheckCircle2 className="w-5 h-5" />
              </div>
              <div>
                <h3 className="text-sm font-bold text-white">
                  변환 완료! ({convertedPaths.length}개 파일)
                </h3>
                <p className="text-xs text-emerald-300/80">
                  모든 WAV 파일이 성공적으로 {targetFormat.toUpperCase()} 포맷으로 변환되었습니다.
                </p>
              </div>
            </div>

            <button
              onClick={onNavigateToHistory}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-emerald-800/40 hover:bg-emerald-700/60 text-emerald-200 text-xs font-semibold border border-emerald-600/40 transition"
            >
              <span>히스토리 보기</span>
              <ArrowRight className="w-3.5 h-3.5" />
            </button>
          </div>

          <div className="space-y-2 max-h-48 overflow-y-auto">
            {convertedPaths.map((cPath) => {
              const cName = cPath.split(/[/\\]/).pop() || cPath;
              return (
                <div
                  key={cPath}
                  className="bg-slate-950/70 border border-emerald-900/40 rounded-xl p-3 flex items-center justify-between gap-3"
                >
                  <div className="flex items-center gap-2.5 min-w-0">
                    <Music2 className="w-4 h-4 text-emerald-400 shrink-0" />
                    <span className="text-xs font-medium text-slate-200 truncate" title={cPath}>
                      {cName}
                    </span>
                  </div>

                  <div className="flex items-center gap-2 shrink-0">
                    <button
                      onClick={() => onOpenDefaultPlayer(cPath)}
                      className="flex items-center gap-1 px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-medium transition"
                    >
                      <Play className="w-3 h-3 text-emerald-400" />
                      <span>재생</span>
                    </button>
                    <button
                      onClick={() => onOpenExplorer(cPath)}
                      className="flex items-center gap-1 px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-medium transition"
                    >
                      <FolderOpen className="w-3 h-3 text-teal-400" />
                      <span>폴더</span>
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
};
