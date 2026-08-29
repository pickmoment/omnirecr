import React, { useState, useRef, useEffect } from 'react';
import {
  FileText,
  Upload,
  Play,
  Pause,
  Download,
  Copy,
  FolderOpen,
  Check,
  AlertCircle,
  Plus,
  Trash2,
  Sparkles,
  Sliders,
  Volume2,
  VolumeX,
  Wand2,
  FileCheck,
  Search,
  Bot,
  Zap,
  Link2,
  Link2Off,
  Mic,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import type {
  Settings,
  SubtitleGenerateResult,
  SubtitleItem,
  SubtitleSplitMode,
} from '../types';
import { resetWhisperPipeline } from '../services/whisperService';
import {
  buildSrt,
  buildVtt,
  formatSrtTimestamp,
  generateSubtitles,
} from '../services/subtitleGeneration';

interface SubtitleGeneratorProps {
  settings: Settings;
  initialAudioPath?: string | null;
  /** 대본 스튜디오의 TTS 녹음에서 넘어온 원본 대본 (대본+AI 싱크 모드 자동 세팅) */
  initialScriptText?: string | null;
  onOpenExplorer: (path: string) => Promise<void>;
  onSettingsChange: (partial: Partial<Settings>) => void;
}

const SAMPLE_SCRIPT = `안녕하세요! OmniRec 스튜디오에 오신 것을 환영합니다.
오늘은 고화질 화면 녹화와 무손실 오디오 캡처 기능을 살펴보겠습니다.
시스템 사운드와 마이크 음성을 독립적으로 제어할 수 있어 매우 편리합니다.
자동 묵음 감지와 노이즈 게이트 필터로 깔끔한 음질을 제공합니다.
녹음이 끝나면 대본과 음성을 결합해 자막 파일을 손쉽게 만들어보세요!`;

export const SubtitleGenerator: React.FC<SubtitleGeneratorProps> = ({
  settings,
  initialAudioPath,
  initialScriptText,
  onOpenExplorer,
  onSettingsChange,
}) => {
  // Input State (initialized from persisted settings so choices survive tab switches / app restarts)
  const [generationWorkflow, setGenerationWorkflow] = useState<'with-script' | 'ai-only'>(settings.subtitle_generation_workflow);
  const [scriptText, setScriptText] = useState<string>('');
  const [audioPath, setAudioPath] = useState<string>(initialAudioPath || '');
  const [whisperLanguage, setWhisperLanguage] = useState<string>(settings.subtitle_whisper_language);

  // 자막 옵션은 settings 를 단일 출처로 삼는다.
  // 예전에는 이 화면이 값을 따로 들고 있어 자막 일괄 생성 화면과 어긋날 수 있었다.
  const syncEngine = settings.subtitle_sync_engine;
  const whisperModel = settings.subtitle_whisper_model;
  const splitMode = settings.subtitle_split_mode;
  const splitOnComma = settings.subtitle_split_on_comma;
  const maxChars = settings.subtitle_max_chars;
  const silenceThresholdDb = settings.subtitle_silence_threshold_db;
  const minSilenceDuration = settings.subtitle_min_silence_duration;

  const setSyncEngine = (value: 'ai-whisper' | 'vad') =>
    onSettingsChange({ subtitle_sync_engine: value });
  const setWhisperModel = (value: Settings['subtitle_whisper_model']) =>
    onSettingsChange({ subtitle_whisper_model: value });
  const setSplitMode = (value: SubtitleSplitMode) =>
    onSettingsChange({ subtitle_split_mode: value });
  const setSplitOnComma = (value: boolean) =>
    onSettingsChange({ subtitle_split_on_comma: value });
  const setMaxChars = (value: number) => onSettingsChange({ subtitle_max_chars: value });
  const setSilenceThresholdDb = (value: number) =>
    onSettingsChange({ subtitle_silence_threshold_db: value });
  const setMinSilenceDuration = (value: number) =>
    onSettingsChange({ subtitle_min_silence_duration: value });
  const [startOffsetSecs, setStartOffsetSecs] = useState<number>(settings.subtitle_start_offset_secs);
  const [autoSave, setAutoSave] = useState<boolean>(settings.subtitle_auto_save);

  // AI Progress State
  const [aiStatusMsg, setAiStatusMsg] = useState<string | null>(null);
  const [aiProgress, setAiProgress] = useState<number>(0);

  // Result & Editor State
  const [isGenerating, setIsGenerating] = useState<boolean>(false);
  const [generateResult, setGenerateResult] = useState<SubtitleGenerateResult | null>(null);
  const [subtitles, setSubtitles] = useState<SubtitleItem[]>([]);
  const [searchQuery, setSearchQuery] = useState<string>('');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [copyFeedback, setCopyFeedback] = useState<string | null>(null);
  const [savedPaths, setSavedPaths] = useState<{ srt?: string; vtt?: string }>({});

  // Audio Playback State
  const [audioBlobUrl, setAudioBlobUrl] = useState<string | null>(null);
  const [isPlaying, setIsPlaying] = useState<boolean>(false);
  const [currentTime, setCurrentTime] = useState<number>(0);
  const [duration, setDuration] = useState<number>(0);
  const [isMuted, setIsMuted] = useState<boolean>(false);
  const [highlightedIndex, setHighlightedIndex] = useState<number | null>(null);
  const [autoScroll, setAutoScroll] = useState<boolean>(settings.subtitle_auto_scroll);
  const [rippleEdit, setRippleEdit] = useState<boolean>(settings.subtitle_ripple_edit);

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const subtitleListRef = useRef<HTMLDivElement | null>(null);
  const activeItemRef = useRef<HTMLDivElement | null>(null);
  const segmentTimerRef = useRef<number | null>(null);
  const isFirstSettingsSyncRef = useRef(true);

  // 아직 settings 를 단일 출처로 옮기지 않은 항목만 되돌려 저장한다.
  // (자막 옵션은 위 setter 들이 즉시 settings 에 쓴다)
  useEffect(() => {
    if (isFirstSettingsSyncRef.current) {
      isFirstSettingsSyncRef.current = false;
      return;
    }
    const timer = window.setTimeout(() => {
      onSettingsChange({
        subtitle_generation_workflow: generationWorkflow,
        subtitle_whisper_language: whisperLanguage,
        subtitle_start_offset_secs: startOffsetSecs,
        subtitle_auto_save: autoSave,
        subtitle_auto_scroll: autoScroll,
        subtitle_ripple_edit: rippleEdit,
      });
    }, 500);
    return () => window.clearTimeout(timer);
  }, [
    generationWorkflow,
    whisperLanguage,
    startOffsetSecs,
    autoSave,
    autoScroll,
    rippleEdit,
  ]);

  // Global Spacebar shortcut for Play / Pause
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName?.toLowerCase();
      if (tag === 'input' || tag === 'textarea') {
        return;
      }

      if (e.code === 'Space') {
        e.preventDefault();
        togglePlay();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isPlaying]);

  // Sync initialAudioPath prop
  useEffect(() => {
    if (initialAudioPath) {
      setAudioPath(initialAudioPath);
      loadAudioFile(initialAudioPath);
    }
  }, [initialAudioPath]);

  // 대본 스튜디오에서 넘어온 대본을 그대로 채우고 "대본 + AI 싱크" 모드로 전환한다.
  useEffect(() => {
    if (initialScriptText) {
      setScriptText(initialScriptText);
      setGenerationWorkflow('with-script');
    }
  }, [initialScriptText]);

  // Load audio file into HTML5 Audio via blob
  const loadAudioFile = async (path: string) => {
    if (!path) return;
    try {
      if (audioBlobUrl) {
        URL.revokeObjectURL(audioBlobUrl);
        setAudioBlobUrl(null);
      }
      const rawBytes = await invoke<number[]>('read_audio_file', { path });
      const uint8 = new Uint8Array(rawBytes);

      let mime = 'audio/mpeg';
      const lower = path.toLowerCase();
      if (lower.endsWith('.wav')) mime = 'audio/wav';
      else if (lower.endsWith('.m4a') || lower.endsWith('.mp4') || lower.endsWith('.aac')) mime = 'audio/mp4';
      else if (lower.endsWith('.ogg')) mime = 'audio/ogg';
      else if (lower.endsWith('.webm')) mime = 'audio/webm';

      const blob = new Blob([uint8], { type: mime });
      const url = URL.createObjectURL(blob);
      setAudioBlobUrl(url);
    } catch (err) {
      console.warn('Failed to load audio for preview:', err);
    }
  };

  // Cleanup blob URL on unmount
  useEffect(() => {
    return () => {
      if (audioBlobUrl) {
        URL.revokeObjectURL(audioBlobUrl);
      }
    };
  }, [audioBlobUrl]);

  // Auto scroll to active subtitle during playback
  useEffect(() => {
    if (!autoScroll || !isPlaying || highlightedIndex === null) return;

    if (activeItemRef.current && subtitleListRef.current) {
      activeItemRef.current.scrollIntoView({
        behavior: 'smooth',
        block: 'nearest',
      });
    }
  }, [highlightedIndex, isPlaying, autoScroll]);

  // Handle select script file
  const handleSelectScriptFile = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [
          { name: '대본 및 텍스트 파일', extensions: ['txt', 'md', 'srt', 'vtt', 'csv', 'json'] },
          { name: '모든 파일', extensions: ['*'] },
        ],
      });
      if (selected && typeof selected === 'string') {
        const content = await invoke<string>('read_script_file', { path: selected });
        setScriptText(content);
      }
    } catch (err) {
      setErrorMsg(`대본 파일 열기 실패: ${err}`);
    }
  };

  // Handle select audio file
  const handleSelectAudioFile = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [
          {
            name: '음성 및 영상 미디어',
            extensions: ['mp3', 'wav', 'm4a', 'aac', 'flac', 'ogg', 'mp4', 'mkv', 'mov', 'webm'],
          },
          { name: '모든 파일', extensions: ['*'] },
        ],
      });
      if (selected && typeof selected === 'string') {
        setAudioPath(selected);
        await loadAudioFile(selected);
      }
    } catch (err) {
      setErrorMsg(`오디오 파일 열기 실패: ${err}`);
    }
  };

  // Generate Subtitles (Local AI Whisper or High-Precision VAD DP)
  // 실제 파이프라인은 services/subtitleGeneration.ts 에 있다.
  // 자막 일괄 생성과 동일한 코드를 쓰기 위해 여기서는 UI 상태만 다룬다.
  const handleGenerate = async () => {
    if (!audioPath.trim()) {
      setErrorMsg('음성 또는 영상 미디어 파일을 선택해 주세요.');
      return;
    }
    if (generationWorkflow === 'with-script' && !scriptText.trim()) {
      setErrorMsg('대본 내용을 입력하거나 파일을 불러와 주세요. (대본 없이 생성하려면 상단에서 [🤖 대본 없는 순수 AI 자막 생성] 모드를 선택하세요)');
      return;
    }

    // Stop existing playback & timer cleanly
    stopAudio();

    setErrorMsg(null);
    setIsGenerating(true);
    setAiStatusMsg(null);
    setAiProgress(0);

    try {
      const outcome = await generateSubtitles({
        audioPath,
        scriptText,
        workflow: generationWorkflow,
        syncEngine,
        whisperModel,
        whisperLanguage,
        splitMode,
        splitOnComma,
        maxChars,
        silenceThresholdDb,
        minSilenceDuration,
        startOffsetSecs,
        autoSave,
        outputDir: settings.output_dir,
        onProgress: (message, percent) => {
          setAiStatusMsg(message);
          if (percent) setAiProgress(percent);
        },
      });

      // 대본 없는 AI 전사 모드에서는 인식 결과를 대본 칸에 채워 준다.
      if (generationWorkflow === 'ai-only' && outcome.transcribedText && !scriptText.trim()) {
        setScriptText(outcome.transcribedText);
      }

      setSubtitles(outcome.subtitles);
      setGenerateResult({
        subtitles: outcome.subtitles,
        srt_content: outcome.srtContent,
        vtt_content: outcome.vttContent,
        srt_path: outcome.srtPath,
        vtt_path: outcome.vttPath,
        total_duration: outcome.totalDuration,
        speech_segments_detected: outcome.segmentsDetected,
        script_lines_count: outcome.subtitles.length,
      });
      setSavedPaths({
        srt: outcome.srtPath || undefined,
        vtt: outcome.vttPath || undefined,
      });

      if (!audioBlobUrl) {
        await loadAudioFile(audioPath);
      }
    } catch (err: any) {
      console.error('Subtitle generation failed:', err);
      setErrorMsg(`자막 생성 실패: ${err?.message || err}`);
    } finally {
      setIsGenerating(false);
      setAiStatusMsg(null);
    }
  };

  // Time format helper for seconds to HH:MM:SS,mmm or MM:SS.mmm
  const formatSecs = (secs: number) => {
    const s = Math.max(0, secs);
    const ms = Math.floor((s % 1) * 1000);
    const totalSecs = Math.floor(s);
    const mins = Math.floor(totalSecs / 60);
    const remSecs = totalSecs % 60;
    return `${String(mins).padStart(2, '0')}:${String(remSecs).padStart(2, '0')}.${String(ms).padStart(3, '0')}`;
  };

  // 자막 문자열 생성은 공용 서비스(services/subtitleGeneration.ts)를 그대로 쓴다.
  const buildCurrentSrt = (items: SubtitleItem[]) => buildSrt(items);
  const buildCurrentVtt = (items: SubtitleItem[]) => buildVtt(items);

  // Audio playback tracking & sync highlighting
  const handleTimeUpdate = () => {
    if (!audioRef.current) return;
    const t = audioRef.current.currentTime;
    setCurrentTime(t);

    const curIdx = subtitles.findIndex((sub) => t >= sub.start_secs && t <= sub.end_secs);
    setHighlightedIndex(curIdx !== -1 ? curIdx : null);
  };

  const togglePlay = () => {
    if (!audioRef.current) return;
    if (segmentTimerRef.current) {
      clearTimeout(segmentTimerRef.current);
      segmentTimerRef.current = null;
    }
    if (isPlaying) {
      audioRef.current.pause();
      setIsPlaying(false);
    } else {
      audioRef.current.play().catch(console.error);
      setIsPlaying(true);
    }
  };

  const stopAudio = () => {
    if (segmentTimerRef.current) {
      clearTimeout(segmentTimerRef.current);
      segmentTimerRef.current = null;
    }
    if (audioRef.current) {
      audioRef.current.pause();
    }
    setIsPlaying(false);
  };

  const handleToggleSubPlay = (sub: SubtitleItem) => {
    if (!audioRef.current) return;

    const isThisSubPlaying = isPlaying && highlightedIndex === sub.index - 1;

    if (isThisSubPlaying) {
      stopAudio();
    } else {
      if (segmentTimerRef.current) {
        clearTimeout(segmentTimerRef.current);
        segmentTimerRef.current = null;
      }

      audioRef.current.currentTime = Math.max(0, sub.start_secs - 0.05);
      audioRef.current
        .play()
        .then(() => setIsPlaying(true))
        .catch(console.error);

      const durMs = Math.max(0.3, sub.end_secs - sub.start_secs + 0.1) * 1000;
      segmentTimerRef.current = window.setTimeout(() => {
        if (audioRef.current) {
          audioRef.current.pause();
          setIsPlaying(false);
        }
      }, durMs);
    }
  };

  // Snap currently playing audio position to start / end of specific subtitle
  const handleSnapStart = (index: number) => {
    if (!audioRef.current) return;
    const t = audioRef.current.currentTime;
    handleStartSecsChange(index, t);
  };

  const handleSnapEnd = (index: number) => {
    if (!audioRef.current) return;
    const t = audioRef.current.currentTime;
    handleEndSecsChange(index, t);
  };

  const handleShiftSingle = (index: number, offsetSecs: number) => {
    setSubtitles((prev) => {
      const target = prev[index];
      if (!target) return prev;
      const newStart = Math.max(0, target.start_secs + offsetSecs);
      const newEnd = Math.max(newStart + 0.15, target.end_secs + offsetSecs);
      const delta = newStart - target.start_secs;

      return prev.map((item, i) => {
        // Previous items (guard overlap)
        if (i < index) {
          if (delta < 0 && i === index - 1 && item.end_secs > newStart - 0.05) {
            const adjEnd = Math.max(item.start_secs + 0.1, newStart - 0.05);
            return {
              ...item,
              end_secs: adjEnd,
              end_formatted: formatSrtTimestamp(adjEnd),
            };
          }
          return item;
        }

        // Current item
        if (i === index) {
          return {
            ...item,
            start_secs: newStart,
            end_secs: newEnd,
            start_formatted: formatSrtTimestamp(newStart),
            end_formatted: formatSrtTimestamp(newEnd),
          };
        }

        // Subsequent items
        if (rippleEdit) {
          const cascadeStart = Math.max(0, item.start_secs + delta);
          const cascadeEnd = Math.max(cascadeStart + 0.15, item.end_secs + delta);
          return {
            ...item,
            start_secs: cascadeStart,
            end_secs: cascadeEnd,
            start_formatted: formatSrtTimestamp(cascadeStart),
            end_formatted: formatSrtTimestamp(cascadeEnd),
          };
        } else if (i === index + 1 && item.start_secs < newEnd + 0.05) {
          // Even if rippleEdit is OFF, resolve immediate overlap with next item
          const adjNextStart = newEnd + 0.05;
          const adjNextEnd = Math.max(adjNextStart + 0.2, item.end_secs);
          return {
            ...item,
            start_secs: adjNextStart,
            end_secs: adjNextEnd,
            start_formatted: formatSrtTimestamp(adjNextStart),
            end_formatted: formatSrtTimestamp(adjNextEnd),
          };
        }

        return item;
      });
    });
  };

  // Subtitle Editing Operations
  const handleTextChange = (index: number, newText: string) => {
    setSubtitles((prev) =>
      prev.map((item, i) => (i === index ? { ...item, text: newText } : item))
    );
  };

  const handleStartSecsChange = (index: number, val: number) => {
    const newStart = Math.max(0, val);
    setSubtitles((prev) => {
      const target = prev[index];
      if (!target) return prev;
      const delta = newStart - target.start_secs;
      if (Math.abs(delta) < 0.001) return prev;

      return prev.map((item, i) => {
        // Previous items: auto resolve overlap when start is pulled earlier
        if (i < index) {
          if (i === index - 1 && item.end_secs > newStart - 0.05) {
            const adjEnd = Math.max(item.start_secs + 0.1, newStart - 0.05);
            return {
              ...item,
              end_secs: adjEnd,
              end_formatted: formatSrtTimestamp(adjEnd),
            };
          }
          return item;
        }

        // Current item
        if (i === index) {
          let itemEnd = rippleEdit ? Math.max(newStart + 0.15, target.end_secs + delta) : target.end_secs;
          if (itemEnd <= newStart + 0.1) {
            itemEnd = newStart + 0.3;
          }
          return {
            ...item,
            start_secs: newStart,
            end_secs: itemEnd,
            start_formatted: formatSrtTimestamp(newStart),
            end_formatted: formatSrtTimestamp(itemEnd),
          };
        }

        // Subsequent items
        if (rippleEdit) {
          const cascadeStart = Math.max(0, item.start_secs + delta);
          const cascadeEnd = Math.max(cascadeStart + 0.15, item.end_secs + delta);
          return {
            ...item,
            start_secs: cascadeStart,
            end_secs: cascadeEnd,
            start_formatted: formatSrtTimestamp(cascadeStart),
            end_formatted: formatSrtTimestamp(cascadeEnd),
          };
        } else if (i === index + 1) {
          // Resolve overlap with immediate next item if current end encroaches
          const currentEnd = Math.max(newStart + 0.15, target.end_secs);
          if (item.start_secs < currentEnd + 0.05) {
            const adjStart = currentEnd + 0.05;
            const adjEnd = Math.max(adjStart + 0.2, item.end_secs);
            return {
              ...item,
              start_secs: adjStart,
              end_secs: adjEnd,
              start_formatted: formatSrtTimestamp(adjStart),
              end_formatted: formatSrtTimestamp(adjEnd),
            };
          }
        }

        return item;
      });
    });
  };

  const handleEndSecsChange = (index: number, val: number) => {
    setSubtitles((prev) => {
      const target = prev[index];
      if (!target) return prev;
      const newEnd = Math.max(target.start_secs + 0.1, val);
      const delta = newEnd - target.end_secs;
      if (Math.abs(delta) < 0.001) return prev;

      return prev.map((item, i) => {
        if (i < index) return item;

        // Current item
        if (i === index) {
          return {
            ...item,
            end_secs: newEnd,
            end_formatted: formatSrtTimestamp(newEnd),
          };
        }

        // Subsequent items
        if (rippleEdit) {
          const cascadeStart = Math.max(0, item.start_secs + delta);
          const cascadeEnd = Math.max(cascadeStart + 0.15, item.end_secs + delta);
          return {
            ...item,
            start_secs: cascadeStart,
            end_secs: cascadeEnd,
            start_formatted: formatSrtTimestamp(cascadeStart),
            end_formatted: formatSrtTimestamp(cascadeEnd),
          };
        } else if (i === index + 1 && item.start_secs < newEnd + 0.05) {
          // Auto resolve overlap with next item: push next item start
          const adjNextStart = newEnd + 0.05;
          const adjNextEnd = Math.max(adjNextStart + 0.2, item.end_secs);
          return {
            ...item,
            start_secs: adjNextStart,
            end_secs: adjNextEnd,
            start_formatted: formatSrtTimestamp(adjNextStart),
            end_formatted: formatSrtTimestamp(adjNextEnd),
          };
        }

        return item;
      });
    });
  };

  const handleDeleteItem = (index: number) => {
    setSubtitles((prev) =>
      prev
        .filter((_, i) => i !== index)
        .map((item, i) => ({ ...item, index: i + 1 }))
    );
  };

  const handleInsertItemAfter = (index: number) => {
    setSubtitles((prev) => {
      const current = prev[index];
      const nextStart = current ? current.end_secs + 0.05 : 0;
      const nextEnd = nextStart + 2.0;

      const newItem: SubtitleItem = {
        index: index + 2,
        start_secs: nextStart,
        end_secs: nextEnd,
        start_formatted: formatSrtTimestamp(nextStart),
        end_formatted: formatSrtTimestamp(nextEnd),
        text: '새 자막 문장',
      };

      const updated = [...prev.slice(0, index + 1), newItem, ...prev.slice(index + 1)];
      return updated.map((it, idx) => ({ ...it, index: idx + 1 }));
    });
  };

  // Scale / Stretch all subtitles proportionally to target end time (eliminate drift)
  const handleScaleToFit = (targetEndSecs: number) => {
    if (subtitles.length === 0 || targetEndSecs <= 0.5) return;
    const firstStart = subtitles[0].start_secs;
    const lastEnd = subtitles[subtitles.length - 1].end_secs;
    const currentSpan = lastEnd - firstStart;
    const targetSpan = targetEndSecs - firstStart;

    if (currentSpan <= 0.1 || targetSpan <= 0.1) return;
    const ratio = targetSpan / currentSpan;

    setSubtitles((prev) =>
      prev.map((item) => {
        const newStart = firstStart + (item.start_secs - firstStart) * ratio;
        const newEnd = firstStart + (item.end_secs - firstStart) * ratio;
        return {
          ...item,
          start_secs: Math.max(0, newStart),
          end_secs: Math.max(newStart + 0.2, newEnd),
          start_formatted: formatSrtTimestamp(Math.max(0, newStart)),
          end_formatted: formatSrtTimestamp(Math.max(newStart + 0.2, newEnd)),
        };
      })
    );
  };

  // Global Time Shift
  const handleShiftAll = (offsetSecs: number) => {
    setSubtitles((prev) =>
      prev.map((item) => {
        const newStart = Math.max(0, item.start_secs + offsetSecs);
        const newEnd = Math.max(newStart + 0.2, item.end_secs + offsetSecs);
        return {
          ...item,
          start_secs: newStart,
          end_secs: newEnd,
          start_formatted: formatSrtTimestamp(newStart),
          end_formatted: formatSrtTimestamp(newEnd),
        };
      })
    );
  };

  // Text cleanup tools
  const handleCleanSpaces = () => {
    setScriptText((prev) =>
      prev
        .split('\n')
        .map((line) => line.trim().replace(/\s+/g, ' '))
        .filter((line) => line.length > 0)
        .join('\n')
    );
  };

  const handleRemoveTimestamps = () => {
    setScriptText((prev) =>
      prev
        .replace(/\[\d{1,2}:\d{2}(?:\.\d+)?\]/g, '')
        .replace(/\d{2}:\d{2}:\d{2}[,\.]\d{3}\s*-->\s*\d{2}:\d{2}:\d{2}[,\.]\d{3}/g, '')
        .split('\n')
        .map((l) => l.trim())
        .filter((l) => l.length > 0)
        .join('\n')
    );
  };

  // Copy to clipboard
  const handleCopy = async (type: 'srt' | 'vtt' | 'text') => {
    try {
      let content = '';
      if (type === 'srt') content = buildCurrentSrt(subtitles);
      else if (type === 'vtt') content = buildCurrentVtt(subtitles);
      else content = subtitles.map((s) => s.text).join('\n');

      await navigator.clipboard.writeText(content);
      setCopyFeedback(type);
      setTimeout(() => setCopyFeedback(null), 2000);
    } catch (err) {
      console.error('Clipboard copy error:', err);
    }
  };

  // Save As file dialog
  const handleSaveAs = async (format: 'srt' | 'vtt') => {
    try {
      const defaultName = audioPath
        ? audioPath.split(/[\\/]/).pop()?.replace(/\.[^/.]+$/, '') + `.${format}`
        : `subtitles.${format}`;

      const filePath = await save({
        defaultPath: defaultName,
        filters: [{ name: format.toUpperCase(), extensions: [format] }],
      });

      if (filePath) {
        const content = format === 'srt' ? buildCurrentSrt(subtitles) : buildCurrentVtt(subtitles);
        await invoke('save_subtitle_file', { path: filePath, content });
        setSavedPaths((prev) => ({ ...prev, [format]: filePath }));
        alert(`${format.toUpperCase()} 파일이 성공적으로 저장되었습니다:\n${filePath}`);
      }
    } catch (err) {
      setErrorMsg(`파일 저장 실패: ${err}`);
    }
  };

  // Filtered Subtitles
  const filteredSubtitles = subtitles.filter((sub) => {
    if (!searchQuery.trim()) return true;
    const q = searchQuery.toLowerCase();
    return sub.text.toLowerCase().includes(q) || String(sub.index).includes(q);
  });

  const getAudioFileName = () => {
    if (!audioPath) return '선택된 파일 없음';
    return audioPath.split(/[\\/]/).pop() || audioPath;
  };

  return (
    <div className="min-h-full flex flex-col bg-slate-950 text-slate-100 pb-10">
      {/* Hidden HTML5 Audio Element for Preview */}
      {audioBlobUrl && (
        <audio
          ref={audioRef}
          src={audioBlobUrl}
          onTimeUpdate={handleTimeUpdate}
          onLoadedMetadata={() => {
            if (audioRef.current) {
              setDuration(audioRef.current.duration);
            }
          }}
          onEnded={() => setIsPlaying(false)}
        />
      )}

      {/* Top Header */}
      <div className="p-6 border-b border-slate-800 bg-gradient-to-b from-slate-900/90 to-slate-950/80">
        <div className="max-w-7xl mx-auto flex flex-col md:flex-row items-start md:items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <div className="w-12 h-12 rounded-2xl bg-gradient-to-tr from-amber-500 via-orange-500 to-yellow-400 flex items-center justify-center shadow-lg shadow-orange-500/20">
              <FileText className="w-6 h-6 text-white" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h1 className="text-xl font-bold text-white tracking-tight">자막 생성기 (Script-to-Sub)</h1>
                <span className="text-xs font-semibold px-2 py-0.5 rounded-full bg-orange-500/20 text-orange-400 border border-orange-500/30">
                  Smart Sync
                </span>
              </div>
              <p className="text-xs text-slate-400 mt-0.5">
                대본 텍스트와 음성 파일을 결합하여 정확한 타임스탬프의 SRT / VTT 자막을 자동 생성합니다.
              </p>
            </div>
          </div>

          {/* Quick Action Badges */}
          <div className="flex items-center gap-2">
            <button
              onClick={() => {
                setScriptText(SAMPLE_SCRIPT);
              }}
              className="px-3 py-1.5 rounded-lg text-xs font-medium bg-slate-800/80 hover:bg-slate-700 text-slate-300 border border-slate-700/70 transition flex items-center gap-1.5"
            >
              <Sparkles className="w-3.5 h-3.5 text-amber-400" />
              <span>예시 대본 불러오기</span>
            </button>
          </div>
        </div>
      </div>

      {/* Main Container */}
      <div className="max-w-7xl mx-auto w-full p-6 space-y-6">
        {/* Workflow Mode Selector Tab */}
        <div className="bg-slate-900/90 border border-slate-800 p-1.5 rounded-2xl flex items-center gap-2 max-w-2xl shadow-lg">
          <button
            onClick={() => setGenerationWorkflow('with-script')}
            className={`flex-1 flex items-center justify-center gap-2 py-2.5 px-4 rounded-xl font-bold text-xs transition ${
              generationWorkflow === 'with-script'
                ? 'bg-gradient-to-r from-amber-600 to-orange-600 text-white shadow-md shadow-orange-600/30'
                : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/60'
            }`}
          >
            <FileText className="w-4 h-4" />
            <span>📝 대본 + AI 싱크 모드 (대본 원문 보존)</span>
          </button>

          <button
            onClick={() => {
              setGenerationWorkflow('ai-only');
              setSyncEngine('ai-whisper');
            }}
            className={`flex-1 flex items-center justify-center gap-2 py-2.5 px-4 rounded-xl font-bold text-xs transition ${
              generationWorkflow === 'ai-only'
                ? 'bg-gradient-to-r from-orange-600 to-amber-500 text-white shadow-md shadow-orange-500/30'
                : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/60'
            }`}
          >
            <Bot className="w-4 h-4 text-amber-300" />
            <span>🤖 대본 없는 순수 AI 자동 자막 모드</span>
          </button>
        </div>

        {/* Error Notification */}
        {errorMsg && (
          <div className="p-4 rounded-xl bg-red-950/60 border border-red-500/50 text-red-300 text-xs flex items-center justify-between gap-3 animate-shake">
            <div className="flex items-center gap-2">
              <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />
              <span>{errorMsg}</span>
            </div>
            <button
              onClick={() => setErrorMsg(null)}
              className="text-red-400 hover:text-red-200 text-xs px-2 py-1 rounded bg-red-900/40"
            >
              닫기
            </button>
          </div>
        )}

        {/* 2-Column Grid for Script Input & Audio Settings */}
        <div className="grid grid-cols-1 lg:grid-cols-12 gap-6">
          {/* Column 1: Script Editor (7 cols) */}
          <div className="lg:col-span-7 bg-slate-900/80 border border-slate-800/90 rounded-2xl p-5 flex flex-col gap-4 shadow-xl">
            {generationWorkflow === 'ai-only' ? (
              /* AI-Only Workflow View */
              <div className="flex-1 flex flex-col justify-between p-4 rounded-xl bg-slate-950/60 border border-slate-800/80 space-y-4">
                <div className="space-y-2">
                  <div className="flex items-center gap-2 text-sm font-bold text-orange-400">
                    <Bot className="w-5 h-5" />
                    <span>순수 AI 음성인식 전사 모드 안내</span>
                  </div>
                  <p className="text-xs text-slate-300 leading-relaxed">
                    대본을 따로 준비할 필요가 없습니다. 우측에서 <strong>음성 또는 영상 미디어 파일</strong>만 선택하고 하단의 <strong>[로컬 AI 자막 자동 생성]</strong> 버튼을 누르면, 온디바이스 AI Whisper가 실제 목소리를 직접 받아쓰고 적절한 길이로 자막을 자동 생성합니다.
                  </p>
                </div>

                <div className="p-3.5 rounded-xl bg-slate-900/90 border border-slate-800 space-y-2 text-xs">
                  <div className="flex items-center gap-2 text-slate-200 font-semibold">
                    <Mic className="w-4 h-4 text-emerald-400" />
                    <span>AI 전사 결과 텍스트 (생성 후 자동 표시)</span>
                  </div>
                  <textarea
                    value={scriptText}
                    readOnly
                    placeholder="AI 자막 생성을 시작하면 전사된 텍스트가 여기에 자동으로 채워집니다..."
                    className="w-full h-36 bg-slate-950 border border-slate-800 rounded-lg p-2.5 text-xs text-slate-300 font-mono focus:outline-none resize-none"
                  />
                </div>
              </div>
            ) : (
              /* With-Script Workflow View */
              <>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <FileText className="w-4 h-4 text-orange-400" />
                    <h2 className="text-sm font-semibold text-white">대본 (스크립트) 입력</h2>
                    <span className="text-[11px] text-slate-400 font-mono">
                      ({scriptText.length.toLocaleString()}자 /{' '}
                      {scriptText.split('\n').filter((l) => l.trim().length > 0).length}줄)
                    </span>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <button
                      onClick={handleSelectScriptFile}
                      title="대본 파일 열기 (.txt, .md, .srt, .vtt)"
                      className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700 flex items-center gap-1.5 transition"
                    >
                      <Upload className="w-3.5 h-3.5 text-orange-400" />
                      <span>파일 열기</span>
                    </button>
                    <button
                      onClick={handleCleanSpaces}
                      title="불필요한 공백 및 빈 줄 정리"
                      className="px-2 py-1.5 rounded-lg text-xs font-medium bg-slate-800/60 hover:bg-slate-700 text-slate-300 border border-slate-800 transition"
                    >
                      공백 정리
                    </button>
                    <button
                      onClick={handleRemoveTimestamps}
                      title="타임코드 태그 제거"
                      className="px-2 py-1.5 rounded-lg text-xs font-medium bg-slate-800/60 hover:bg-slate-700 text-slate-300 border border-slate-800 transition"
                    >
                      태그 제거
                    </button>
                    {scriptText && (
                      <button
                        onClick={() => setScriptText('')}
                        title="대본 지우기"
                        className="p-1.5 rounded-lg text-slate-400 hover:text-red-400 hover:bg-slate-800 transition"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    )}
                  </div>
                </div>

                {/* Script Textarea */}
                <div className="relative flex-1 min-h-[220px]">
                  <textarea
                    value={scriptText}
                    onChange={(e) => setScriptText(e.target.value)}
                    placeholder="여기에 대본이나 낭독 스크립트를 직접 입력하거나 붙여넣으세요...&#10;또는 [파일 열기] 버튼으로 텍스트(.txt, .md) 파일을 불러올 수 있습니다."
                    className="w-full h-full min-h-[220px] bg-slate-950/70 border border-slate-800 rounded-xl p-3.5 text-xs text-slate-200 placeholder:text-slate-500 font-sans leading-relaxed focus:outline-none focus:border-orange-500/50 focus:ring-1 focus:ring-orange-500/50 resize-y"
                  />
                </div>
              </>
            )}
          </div>

          {/* Column 2: Audio Source & Alignment Controls (5 cols) */}
          <div className="lg:col-span-5 bg-slate-900/80 border border-slate-800/90 rounded-2xl p-5 flex flex-col gap-4 shadow-xl">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Sliders className="w-4 h-4 text-orange-400" />
                <h2 className="text-sm font-semibold text-white">음성 파일 & 정렬 설정</h2>
              </div>
            </div>

            {/* Audio File Picker Card */}
            <div className="p-3 rounded-xl bg-slate-950/70 border border-slate-800 flex flex-col gap-2.5">
              <div className="flex items-center justify-between">
                <span className="text-[11px] font-medium text-slate-400">오디오 / 비디오 소스</span>
                <button
                  onClick={handleSelectAudioFile}
                  className="px-2.5 py-1 rounded-md text-xs font-semibold bg-orange-600 hover:bg-orange-500 text-white shadow-sm transition flex items-center gap-1.5"
                >
                  <FolderOpen className="w-3.5 h-3.5" />
                  <span>파일 선택</span>
                </button>
              </div>

              <div className="flex items-center gap-2.5 p-2 rounded-lg bg-slate-900/90 border border-slate-800/80">
                <div className="w-8 h-8 rounded-lg bg-orange-500/10 border border-orange-500/30 flex items-center justify-center shrink-0">
                  <Volume2 className="w-4 h-4 text-orange-400" />
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-xs font-medium text-slate-200 truncate">{getAudioFileName()}</p>
                  <p className="text-[10px] text-slate-400 truncate">
                    {audioPath ? audioPath : 'MP3, WAV, M4A, MP4 등 지원'}
                  </p>
                </div>
              </div>

              {/* In-app Audio Mini Controller */}
              {audioBlobUrl && (
                <div className="pt-2 border-t border-slate-800/80 flex items-center gap-2">
                  <button
                    onClick={togglePlay}
                    className="w-7 h-7 rounded-full bg-orange-600 hover:bg-orange-500 text-white flex items-center justify-center shadow transition shrink-0"
                  >
                    {isPlaying ? <Pause className="w-3.5 h-3.5" /> : <Play className="w-3.5 h-3.5 translate-x-0.5" />}
                  </button>

                  <div className="flex-1 flex flex-col gap-1 min-w-0">
                    <div className="flex items-center justify-between text-[10px] font-mono text-slate-400">
                      <span>{formatSecs(currentTime)}</span>
                      <span>{formatSecs(duration)}</span>
                    </div>
                    <input
                      type="range"
                      min={0}
                      max={duration || 100}
                      step={0.05}
                      value={currentTime}
                      onChange={(e) => {
                        const val = parseFloat(e.target.value);
                        setCurrentTime(val);
                        if (audioRef.current) audioRef.current.currentTime = val;
                      }}
                      className="w-full h-1 bg-slate-800 rounded-lg appearance-none cursor-pointer accent-orange-500"
                    />
                  </div>

                  <button
                    onClick={() => {
                      if (audioRef.current) {
                        audioRef.current.muted = !isMuted;
                        setIsMuted(!isMuted);
                      }
                    }}
                    title={isMuted ? '음소거 해제' : '음소거'}
                    className="p-1 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800 transition shrink-0"
                  >
                    {isMuted ? <VolumeX className="w-3.5 h-3.5 text-red-400" /> : <Volume2 className="w-3.5 h-3.5" />}
                  </button>
                </div>
              )}
            </div>

            {/* Sync Engine Selector */}
            <div className="p-3 rounded-xl bg-slate-950/80 border border-slate-800 space-y-2">
              <div className="flex items-center justify-between">
                <span className="text-[11px] font-bold text-slate-200 flex items-center gap-1.5">
                  <Bot className="w-3.5 h-3.5 text-orange-400" />
                  <span>자막 싱크 엔진 선택</span>
                </span>
                <span className="text-[10px] text-orange-400 font-medium">
                  {syncEngine === 'ai-whisper' ? '로컬 AI Whisper (단어 실측)' : '고속 음성 파형 VAD'}
                </span>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <button
                  onClick={() => setSyncEngine('ai-whisper')}
                  className={`p-2.5 rounded-xl border flex flex-col items-start gap-1 transition text-left ${
                    syncEngine === 'ai-whisper'
                      ? 'bg-orange-950/50 border-orange-500/80 shadow-md shadow-orange-500/10'
                      : 'bg-slate-900/60 border-slate-800 hover:border-slate-700 text-slate-400'
                  }`}
                >
                  <div className="flex items-center gap-1.5 text-xs font-bold text-slate-100">
                    <Bot className="w-4 h-4 text-orange-400" />
                    <span>로컬 AI Whisper</span>
                    <span className="text-[9px] bg-orange-500/20 text-orange-400 px-1 py-0.2 rounded border border-orange-500/30">추천</span>
                  </div>
                  <p className="text-[10px] text-slate-400 leading-tight">
                    AI가 실제 음성 단어 타임코드를 실측 분석하여 대본과 100% 밀착 정렬
                  </p>
                </button>

                <button
                  onClick={() => setSyncEngine('vad')}
                  className={`p-2.5 rounded-xl border flex flex-col items-start gap-1 transition text-left ${
                    syncEngine === 'vad'
                      ? 'bg-cyan-950/50 border-cyan-500/80 shadow-md shadow-cyan-500/10'
                      : 'bg-slate-900/60 border-slate-800 hover:border-slate-700 text-slate-400'
                  }`}
                >
                  <div className="flex items-center gap-1.5 text-xs font-bold text-slate-100">
                    <Zap className="w-4 h-4 text-cyan-400" />
                    <span>고속 음성 파형 VAD</span>
                  </div>
                  <p className="text-[10px] text-slate-400 leading-tight">
                    FFmpeg 16kHz PCM 음성 에너지 및 DP 기반 초고속 정렬
                  </p>
                </button>
              </div>

              {syncEngine === 'ai-whisper' && (
                <div className="pt-2 border-t border-slate-800/80 space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[11px] text-slate-300">AI 모델:</span>
                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => {
                          resetWhisperPipeline();
                          setWhisperModel('Xenova/whisper-tiny');
                        }}
                        className={`px-2 py-1 rounded-lg text-[10px] font-semibold transition ${
                          whisperModel === 'Xenova/whisper-tiny'
                            ? 'bg-orange-600 text-white shadow-sm'
                            : 'bg-slate-800 text-slate-400 hover:text-slate-200'
                        }`}
                      >
                        Tiny (39MB • 초고속)
                      </button>
                      <button
                        onClick={() => {
                          resetWhisperPipeline();
                          setWhisperModel('Xenova/whisper-base');
                        }}
                        className={`px-2 py-1 rounded-lg text-[10px] font-semibold transition ${
                          whisperModel === 'Xenova/whisper-base'
                            ? 'bg-orange-600 text-white shadow-sm'
                            : 'bg-slate-800 text-slate-400 hover:text-slate-200'
                        }`}
                      >
                        Base (73MB • 표준 권장)
                      </button>
                      <button
                        onClick={() => {
                          resetWhisperPipeline();
                          setWhisperModel('Xenova/whisper-small');
                        }}
                        className={`px-2 py-1 rounded-lg text-[10px] font-semibold transition ${
                          whisperModel === 'Xenova/whisper-small'
                            ? 'bg-orange-600 text-white shadow-sm'
                            : 'bg-slate-800 text-slate-400 hover:text-slate-200'
                        }`}
                      >
                        Small (240MB • 고정밀)
                      </button>
                    </div>
                  </div>
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[11px] text-slate-300">인식 언어:</span>
                    <div className="flex items-center gap-1">
                      {[
                        { id: 'korean', label: '한국어' },
                        { id: 'english', label: '영어' },
                        { id: 'japanese', label: '일본어' },
                        { id: 'auto', label: '자동 감지' },
                      ].map((lang) => (
                        <button
                          key={lang.id}
                          onClick={() => setWhisperLanguage(lang.id)}
                          className={`px-2 py-0.5 rounded-md text-[10px] font-medium transition ${
                            whisperLanguage === lang.id
                              ? 'bg-orange-600 text-white font-semibold'
                              : 'bg-slate-900 text-slate-400 hover:text-slate-200 border border-slate-800'
                          }`}
                        >
                          {lang.label}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>
              )}
            </div>
            <div className="space-y-3 pt-1 text-xs">
              {/* Split Mode */}
              <div className="flex flex-col gap-1.5">
                <label className="text-[11px] font-medium text-slate-300">자막 분할 기준</label>
                <div className="grid grid-cols-4 gap-1 bg-slate-950 p-1 rounded-xl border border-slate-800 text-[11px]">
                  {[
                    { id: 'auto', label: '스마트 자동' },
                    { id: 'sentence', label: '문장 단위' },
                    { id: 'line', label: '줄바꿈 단위' },
                    { id: 'length', label: '글자수 단위' },
                  ].map((mode) => (
                    <button
                      key={mode.id}
                      onClick={() => setSplitMode(mode.id as SubtitleSplitMode)}
                      className={`py-1 rounded-lg font-medium transition ${
                        splitMode === mode.id
                          ? 'bg-orange-600 text-white shadow-sm'
                          : 'text-slate-400 hover:text-slate-200'
                      }`}
                    >
                      {mode.label}
                    </button>
                  ))}
                </div>
                {splitMode === 'sentence' && (
                  <label className="flex items-center justify-between gap-2 pt-0.5 cursor-pointer">
                    <span className="text-[11px] text-slate-300">쉼표(,)에서도 분리</span>
                    <input
                      type="checkbox"
                      checked={splitOnComma}
                      onChange={(e) => setSplitOnComma(e.target.checked)}
                      className="w-4 h-4 rounded bg-slate-900 border-slate-700 text-orange-500 focus:ring-0 cursor-pointer"
                    />
                  </label>
                )}
              </div>

              {/* Max Chars Slider */}
              <div className="flex items-center justify-between gap-4">
                <div>
                  <span className="text-slate-300 font-medium">최대 글자 수 (줄당)</span>
                  <p className="text-[10px] text-slate-400">화면에 한 번에 표시할 적정 글자 수</p>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="range"
                    min={15}
                    max={50}
                    value={maxChars}
                    onChange={(e) => setMaxChars(parseInt(e.target.value, 10))}
                    className="w-24 h-1 bg-slate-800 rounded appearance-none cursor-pointer accent-orange-500"
                  />
                  <span className="font-mono text-xs text-orange-400 w-8 text-right">{maxChars}자</span>
                </div>
              </div>

              {/* Silence Threshold */}
              <div className="flex items-center justify-between gap-4">
                <div>
                  <span className="text-slate-300 font-medium">묵음 감지 민감도</span>
                  <p className="text-[10px] text-slate-400">발화 구간을 찾는 기준 데시벨</p>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="range"
                    min={-50}
                    max={-20}
                    step={1}
                    value={silenceThresholdDb}
                    onChange={(e) => setSilenceThresholdDb(parseFloat(e.target.value))}
                    className="w-24 h-1 bg-slate-800 rounded appearance-none cursor-pointer accent-orange-500"
                  />
                  <span className="font-mono text-xs text-orange-400 w-12 text-right">{silenceThresholdDb}dB</span>
                </div>
              </div>

              {/* Min Silence & Start Offset */}
              <div className="grid grid-cols-2 gap-3 pt-1">
                <div className="flex flex-col gap-1">
                  <span className="text-slate-300 text-[11px] font-medium">최소 쉼 간격</span>
                  <div className="flex items-center gap-1.5">
                    <input
                      type="range"
                      min={0.1}
                      max={1.0}
                      step={0.05}
                      value={minSilenceDuration}
                      onChange={(e) => setMinSilenceDuration(parseFloat(e.target.value))}
                      className="flex-1 h-1 bg-slate-800 rounded appearance-none cursor-pointer accent-orange-500"
                    />
                    <span className="font-mono text-[11px] text-orange-400 w-9 text-right">{minSilenceDuration.toFixed(2)}s</span>
                  </div>
                </div>

                <div className="flex flex-col gap-1">
                  <span className="text-slate-300 text-[11px] font-medium">시작 오프셋</span>
                  <div className="flex items-center gap-1.5">
                    <input
                      type="range"
                      min={0.0}
                      max={1.0}
                      step={0.05}
                      value={startOffsetSecs}
                      onChange={(e) => setStartOffsetSecs(parseFloat(e.target.value))}
                      className="flex-1 h-1 bg-slate-800 rounded appearance-none cursor-pointer accent-orange-500"
                    />
                    <span className="font-mono text-[11px] text-orange-400 w-9 text-right">{startOffsetSecs.toFixed(2)}s</span>
                  </div>
                </div>
              </div>

              {/* Auto Save Toggle */}
              <div className="flex items-center justify-between pt-1">
                <span className="text-slate-300 font-medium">생성 즉시 파일 자동 저장</span>
                <input
                  type="checkbox"
                  checked={autoSave}
                  onChange={(e) => setAutoSave(e.target.checked)}
                  className="w-4 h-4 rounded bg-slate-900 border-slate-700 text-orange-500 focus:ring-0 cursor-pointer"
                />
              </div>
            </div>

            {/* AI Progress Notification */}
            {isGenerating && aiStatusMsg && (
              <div className="p-3 rounded-xl bg-orange-950/40 border border-orange-500/40 space-y-2 animate-fadeIn">
                <div className="flex items-center justify-between text-xs font-semibold text-orange-300">
                  <div className="flex items-center gap-2">
                    <div className="w-3.5 h-3.5 border-2 border-orange-400/30 border-t-orange-400 rounded-full animate-spin" />
                    <span>{aiStatusMsg}</span>
                  </div>
                  {aiProgress > 0 && <span className="font-mono text-orange-400">{aiProgress}%</span>}
                </div>
                {aiProgress > 0 && (
                  <div className="w-full bg-slate-900 rounded-full h-1.5 overflow-hidden">
                    <div
                      className="bg-gradient-to-r from-amber-500 to-orange-500 h-1.5 rounded-full transition-all duration-300"
                      style={{ width: `${aiProgress}%` }}
                    />
                  </div>
                )}
              </div>
            )}

            {/* Big Action Button */}
            <button
              onClick={handleGenerate}
              disabled={isGenerating || !audioPath || (generationWorkflow === 'with-script' && !scriptText.trim())}
              className="w-full mt-auto py-3 rounded-xl bg-gradient-to-r from-amber-500 via-orange-500 to-yellow-500 hover:from-amber-600 hover:via-orange-600 hover:to-yellow-600 disabled:opacity-50 disabled:cursor-not-allowed text-white font-bold text-sm shadow-lg shadow-orange-500/25 flex items-center justify-center gap-2 transition"
            >
              {isGenerating ? (
                <>
                  <div className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  <span>{syncEngine === 'ai-whisper' ? '로컬 AI 음성인식 및 대본 정밀 매칭 중...' : 'FFmpeg 음성 분석 및 자막 정렬 중...'}</span>
                </>
              ) : (
                <>
                  {syncEngine === 'ai-whisper' ? <Bot className="w-4 h-4" /> : <Wand2 className="w-4 h-4" />}
                  <span>{syncEngine === 'ai-whisper' ? '로컬 AI Whisper 기반 자막 자동 생성' : '대본 기반 자막 자동 생성하기'}</span>
                </>
              )}
            </button>
          </div>
        </div>

        {/* Subtitle Result & Interactive Timeline Editor */}
        {subtitles.length > 0 && (
          <div className="bg-slate-900/90 border border-slate-800 rounded-2xl p-5 space-y-4 shadow-2xl animate-fadeIn">
            {/* Editor Top Bar */}
            <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3 pb-3 border-b border-slate-800">
              <div className="flex items-center gap-3 flex-wrap">
                <div className="flex items-center gap-2">
                  <FileCheck className="w-5 h-5 text-emerald-400" />
                  <h3 className="text-sm font-bold text-white">생성된 자막 목록 ({subtitles.length}개)</h3>
                </div>

                {/* Quick Master Play/Pause in Header */}
                {audioBlobUrl && (
                  <button
                    onClick={togglePlay}
                    className={`px-2.5 py-1 rounded-lg text-xs font-semibold flex items-center gap-1.5 transition ${
                      isPlaying
                        ? 'bg-orange-600 text-white shadow-md shadow-orange-600/30'
                        : 'bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700'
                    }`}
                  >
                    {isPlaying ? <Pause className="w-3.5 h-3.5" /> : <Play className="w-3.5 h-3.5" />}
                    <span>{isPlaying ? '일시정지 (Space)' : '전체 재생 (Space)'}</span>
                    <span className="font-mono text-[10px] text-orange-300 ml-1">
                      {formatSecs(currentTime)}
                    </span>
                  </button>
                )}

                {generateResult && (
                  <span className="text-[11px] px-2 py-0.5 rounded bg-emerald-950/60 text-emerald-400 border border-emerald-800/60 font-medium">
                    총 {formatSecs(generateResult.total_duration)} / {generateResult.speech_segments_detected}개 구간 감지
                  </span>
                )}
                <span className="text-[10px] text-slate-400 bg-slate-800/80 px-2 py-0.5 rounded border border-slate-700/60 hidden xl:inline-block">
                  💡 Space: 재생/정지 | [ / ]: 시작/종료 스냅
                </span>
              </div>

              {/* Time Shift & Search */}
              <div className="flex items-center gap-2 flex-wrap">
                {/* Search */}
                <div className="relative">
                  <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-slate-400" />
                  <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder="자막 검색..."
                    className="pl-8 pr-2.5 py-1 text-xs bg-slate-950 border border-slate-800 rounded-lg text-slate-200 focus:outline-none focus:border-orange-500/60 w-32 focus:w-44 transition-all"
                  />
                </div>

                {/* Auto Scroll Toggle */}
                <button
                  onClick={() => setAutoScroll(!autoScroll)}
                  className={`px-2 py-1 rounded-lg text-[11px] font-medium border transition flex items-center gap-1 ${
                    autoScroll
                      ? 'bg-orange-950/60 border-orange-600/50 text-orange-300 shadow-sm'
                      : 'bg-slate-950 border-slate-800 text-slate-400 hover:text-slate-200'
                  }`}
                  title="재생 중 현재 자막 위치로 자동 스크롤"
                >
                  <Check className={`w-3 h-3 ${autoScroll ? 'text-orange-400 opacity-100' : 'opacity-0'}`} />
                  <span>자동 스크롤</span>
                </button>

                {/* Ripple Edit Toggle */}
                <button
                  onClick={() => setRippleEdit(!rippleEdit)}
                  className={`px-2.5 py-1 rounded-lg text-[11px] font-medium border transition flex items-center gap-1.5 ${
                    rippleEdit
                      ? 'bg-emerald-950/60 border-emerald-600/50 text-emerald-300 shadow-sm'
                      : 'bg-slate-950 border-slate-800 text-slate-400 hover:text-slate-200'
                  }`}
                  title={
                    rippleEdit
                      ? '이후 자막 연동 ON: 하나의 자막 시간을 바꾸면 이후 모든 자막도 함께 이동합니다.'
                      : '이후 자막 연동 OFF: 해당 자막만 독립적으로 수정합니다.'
                  }
                >
                  {rippleEdit ? <Link2 className="w-3.5 h-3.5 text-emerald-400" /> : <Link2Off className="w-3.5 h-3.5 text-slate-500" />}
                  <span>이후 자막 연동 {rippleEdit ? 'ON' : 'OFF'}</span>
                </button>

                {/* Scale to Duration / Fit Buttons */}
                <div className="flex items-center bg-slate-950 rounded-lg border border-slate-800 p-0.5 text-[11px]">
                  {duration > 0 && (
                    <button
                      onClick={() => handleScaleToFit(duration - 0.2)}
                      className="px-2 py-0.5 hover:bg-orange-950/60 hover:text-orange-300 text-slate-300 rounded transition"
                      title="전체 오디오 길이에 맞춰 뒤쪽 싱크를 비례 자동 맞춤"
                    >
                      🎯 오디오 길이에 맞춤
                    </button>
                  )}
                  {currentTime > 1.0 && (
                    <button
                      onClick={() => handleScaleToFit(currentTime)}
                      className="px-2 py-0.5 hover:bg-emerald-950/60 hover:text-emerald-300 text-slate-300 rounded transition"
                      title="현재 재생 중인 위치까지 전체 자막을 비례 맞춤"
                    >
                      현재 위치에 끝 맞춤
                    </button>
                  )}
                </div>

                {/* Shift Buttons */}
                <div className="flex items-center bg-slate-950 rounded-lg border border-slate-800 p-0.5 text-[11px]">
                  <span className="px-2 text-slate-400 font-medium">전체 이동:</span>
                  <button
                    onClick={() => handleShiftAll(-0.5)}
                    className="px-1.5 py-0.5 hover:bg-slate-800 text-slate-300 rounded"
                    title="-0.5초 당기기"
                  >
                    -0.5s
                  </button>
                  <button
                    onClick={() => handleShiftAll(-0.1)}
                    className="px-1.5 py-0.5 hover:bg-slate-800 text-slate-300 rounded"
                    title="-0.1초 당기기"
                  >
                    -0.1s
                  </button>
                  <button
                    onClick={() => handleShiftAll(0.1)}
                    className="px-1.5 py-0.5 hover:bg-slate-800 text-slate-300 rounded"
                    title="+0.1초 미루기"
                  >
                    +0.1s
                  </button>
                  <button
                    onClick={() => handleShiftAll(0.5)}
                    className="px-1.5 py-0.5 hover:bg-slate-800 text-slate-300 rounded"
                    title="+0.5초 미루기"
                  >
                    +0.5s
                  </button>
                </div>
              </div>
            </div>

            {/* Subtitle List / Table */}
            <div
              ref={subtitleListRef}
              className="max-h-[380px] overflow-y-auto space-y-2 pr-1 rounded-xl bg-slate-950/60 p-2 border border-slate-800/80"
            >
              {filteredSubtitles.map((sub) => {
                const isHighlight = highlightedIndex === sub.index - 1;
                return (
                  <div
                    key={sub.index}
                    ref={isHighlight ? activeItemRef : null}
                    className={`flex items-center gap-3 p-2.5 rounded-xl border transition-all ${
                      isHighlight
                        ? 'bg-orange-950/40 border-orange-500/60 shadow-md shadow-orange-500/10'
                        : 'bg-slate-900/60 border-slate-800/70 hover:border-slate-700'
                    }`}
                  >
                    {/* Index */}
                    <div className="w-8 text-center text-xs font-mono font-bold text-slate-400 shrink-0">
                      #{sub.index}
                    </div>

                    {/* Play / Pause Segment Button */}
                    <button
                      onClick={() => handleToggleSubPlay(sub)}
                      title={isHighlight && isPlaying ? '재생 멈춤 (Space)' : '이 구간 재생 (Space)'}
                      className={`w-7 h-7 rounded-lg flex items-center justify-center shrink-0 transition ${
                        isHighlight && isPlaying
                          ? 'bg-orange-600 text-white shadow-md shadow-orange-600/40 animate-pulse'
                          : 'bg-slate-800 hover:bg-orange-600 text-slate-300 hover:text-white'
                      }`}
                    >
                      {isHighlight && isPlaying ? (
                        <Pause className="w-3.5 h-3.5" />
                      ) : (
                        <Play className="w-3.5 h-3.5 translate-x-0.5" />
                      )}
                    </button>

                    {/* Timestamps */}
                    <div className="flex items-center gap-1.5 text-xs font-mono shrink-0">
                      <div className="flex items-center gap-1">
                        <button
                          onClick={() => handleSnapStart(sub.index - 1)}
                          title="현재 재생 위치를 시작 시간으로 맞춤 ([)"
                          className="px-1.5 py-0.5 rounded bg-slate-800 hover:bg-emerald-600 text-emerald-300 hover:text-white text-[10px] font-sans font-semibold border border-slate-700 transition"
                        >
                          시작
                        </button>
                        <input
                          type="number"
                          step={0.1}
                          value={sub.start_secs.toFixed(2)}
                          onChange={(e) => handleStartSecsChange(sub.index - 1, parseFloat(e.target.value))}
                          className="w-16 px-1 py-1 rounded bg-slate-950 border border-slate-800 text-emerald-400 font-mono text-center focus:outline-none focus:border-orange-500"
                        />
                      </div>
                      <span className="text-slate-500">➔</span>
                      <div className="flex items-center gap-1">
                        <input
                          type="number"
                          step={0.1}
                          value={sub.end_secs.toFixed(2)}
                          onChange={(e) => handleEndSecsChange(sub.index - 1, parseFloat(e.target.value))}
                          className="w-16 px-1 py-1 rounded bg-slate-950 border border-slate-800 text-cyan-400 font-mono text-center focus:outline-none focus:border-orange-500"
                        />
                        <button
                          onClick={() => handleSnapEnd(sub.index - 1)}
                          title="현재 재생 위치를 종료 시간으로 맞춤 (])"
                          className="px-1.5 py-0.5 rounded bg-slate-800 hover:bg-cyan-600 text-cyan-300 hover:text-white text-[10px] font-sans font-semibold border border-slate-700 transition"
                        >
                          종료
                        </button>
                      </div>
                    </div>

                    {/* Text Input */}
                    <div className="flex-1">
                      <input
                        type="text"
                        value={sub.text}
                        onChange={(e) => handleTextChange(sub.index - 1, e.target.value)}
                        className="w-full px-2.5 py-1.5 rounded-lg bg-slate-950/80 border border-slate-800 text-xs text-slate-100 focus:outline-none focus:border-orange-500"
                      />
                    </div>

                    {/* Single line fine shift & actions */}
                    <div className="flex items-center gap-1 shrink-0">
                      <div className="flex items-center bg-slate-950 rounded-lg border border-slate-800 p-0.5 text-[10px]">
                        <button
                          onClick={() => handleShiftSingle(sub.index - 1, -0.1)}
                          className="px-1 py-0.5 hover:bg-slate-800 text-slate-400 hover:text-slate-200 rounded"
                          title="0.1초 당기기"
                        >
                          -0.1s
                        </button>
                        <button
                          onClick={() => handleShiftSingle(sub.index - 1, 0.1)}
                          className="px-1 py-0.5 hover:bg-slate-800 text-slate-400 hover:text-slate-200 rounded"
                          title="0.1초 미루기"
                        >
                          +0.1s
                        </button>
                      </div>
                      <button
                        onClick={() => handleInsertItemAfter(sub.index - 1)}
                        title="아래에 새 자막 추가"
                        className="p-1.5 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition"
                      >
                        <Plus className="w-3.5 h-3.5" />
                      </button>
                      <button
                        onClick={() => handleDeleteItem(sub.index - 1)}
                        title="자막 삭제"
                        className="p-1.5 rounded-lg text-slate-400 hover:text-red-400 hover:bg-slate-800 transition"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>

            {/* Bottom Export & Download Bar */}
            <div className="pt-2 flex flex-col md:flex-row items-stretch md:items-center justify-between gap-3">
              {/* Copy actions */}
              <div className="flex items-center gap-2">
                <span className="text-xs text-slate-400 font-medium">클립보드 복사:</span>
                <button
                  onClick={() => handleCopy('srt')}
                  className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700 transition flex items-center gap-1.5"
                >
                  {copyFeedback === 'srt' ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
                  <span>SRT 복사</span>
                </button>
                <button
                  onClick={() => handleCopy('vtt')}
                  className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700 transition flex items-center gap-1.5"
                >
                  {copyFeedback === 'vtt' ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
                  <span>VTT 복사</span>
                </button>
                <button
                  onClick={() => handleCopy('text')}
                  className="px-2.5 py-1.5 rounded-lg text-xs font-medium bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700 transition flex items-center gap-1.5"
                >
                  {copyFeedback === 'text' ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
                  <span>텍스트 복사</span>
                </button>
              </div>

              {/* Save & Explorer */}
              <div className="flex items-center gap-2">
                <button
                  onClick={() => handleSaveAs('srt')}
                  className="px-3.5 py-1.5 rounded-lg text-xs font-semibold bg-emerald-600 hover:bg-emerald-500 text-white shadow-sm transition flex items-center gap-1.5"
                >
                  <Download className="w-3.5 h-3.5" />
                  <span>SRT 파일 저장</span>
                </button>
                <button
                  onClick={() => handleSaveAs('vtt')}
                  className="px-3.5 py-1.5 rounded-lg text-xs font-semibold bg-cyan-600 hover:bg-cyan-500 text-white shadow-sm transition flex items-center gap-1.5"
                >
                  <Download className="w-3.5 h-3.5" />
                  <span>VTT 파일 저장</span>
                </button>
                {savedPaths.srt && (
                  <button
                    onClick={() => onOpenExplorer(savedPaths.srt!)}
                    className="p-1.5 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800 border border-slate-700 transition"
                    title="저장된 폴더 열기"
                  >
                    <FolderOpen className="w-4 h-4" />
                  </button>
                )}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
