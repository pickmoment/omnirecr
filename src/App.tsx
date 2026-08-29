import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

import type {
  TabType,
  Settings,
  RecordingStatus,
  HistoryItem,
  RectRegion,
  AudioVUMeterPayload,
} from './types';
import { Navbar } from './components/Navbar';
import { ScreenRecorder } from './components/ScreenRecorder';
import { AudioRecorder } from './components/AudioRecorder';
import { HistoryList } from './components/HistoryList';
import { MediaJoiner } from './components/MediaJoiner';
import { AudioConverter } from './components/AudioConverter';
import { SubtitleGenerator } from './components/SubtitleGenerator';
import { SettingsModal } from './components/SettingsModal';
import { SettingsView } from './components/SettingsView';
import { SelectionOverlay } from './components/SelectionOverlay';
import { MiniController } from './components/MiniController';

const defaultSettings: Settings = {
  output_dir: '',
  audio_format: 'm4a',
  audio_bitrate: 256,
  audio_sample_rate: 48000,
  video_fps: 60,
  system_audio_enabled: true,
  system_audio_volume: 1.0,
  mic_audio_enabled: true,
  mic_audio_volume: 1.0,
  noise_gate_enabled: true,
  noise_gate_threshold_db: -45.0,
  highpass_filter_enabled: true,
  mute_notifications: true,
  macos_shortcut_start: 'OmniRec 녹화 시작',
  macos_shortcut_stop: 'OmniRec 녹화 종료',
  auto_pause_enabled: false,
  auto_pause_seconds: 1.0,
  auto_stop_enabled: false,
  auto_stop_seconds: 5.0,
  custom_ffmpeg_path: null,
  subtitle_generation_workflow: 'with-script',
  subtitle_sync_engine: 'ai-whisper',
  subtitle_whisper_model: 'Xenova/whisper-base',
  subtitle_whisper_language: 'korean',
  subtitle_split_mode: 'auto',
  subtitle_max_chars: 28,
  subtitle_silence_threshold_db: -35.0,
  subtitle_min_silence_duration: 0.25,
  subtitle_start_offset_secs: 0.1,
  subtitle_auto_save: true,
  subtitle_auto_scroll: true,
  subtitle_ripple_edit: false,
  subtitle_split_on_comma: false,
};

const initialRecordingStatus: RecordingStatus = {
  status: 'idle',
  mode: null,
  duration_secs: 0,
  size_bytes: 0,
  is_auto_paused: false,
  output_file: null,
  sys_vu_level: -60.0,
  mic_vu_level: -60.0,
};

export const App: React.FC = () => {
  const [windowLabel] = useState<string>(() => {
    try {
      return getCurrentWebviewWindow()?.label || 'main';
    } catch {
      return 'main';
    }
  });

  const [currentTab, setCurrentTab] = useState<TabType>('screen');
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [recordingStatus, setRecordingStatus] = useState<RecordingStatus>(initialRecordingStatus);
  const [selectedRegion, setSelectedRegion] = useState<RectRegion | null>(null);

  const [historyItems, setHistoryItems] = useState<HistoryItem[]>([]);
  const [isHistoryLoading, setIsHistoryLoading] = useState(false);

  const [joinerInitialFiles, setJoinerInitialFiles] = useState<string[]>([]);
  const [converterInitialFiles, setConverterInitialFiles] = useState<string[]>([]);
  const [subtitleInitialAudio, setSubtitleInitialAudio] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [ffmpegDetected, setFfmpegDetected] = useState(false);

  // Initialize app if in main window
  useEffect(() => {
    if (windowLabel !== 'main') return;

    // 1. Load settings
    invoke<Settings>('get_settings')
      .then((loaded) => {
        setSettings(loaded);
        refreshHistory();
        checkFfmpeg(loaded.custom_ffmpeg_path);
      })
      .catch((err) => console.error('Failed to load settings:', err));

    // 2. Event listeners
    const unlistenStatus = listen<RecordingStatus>('recording_status_change', (event) => {
      setRecordingStatus((prev) => {
        // If recording just finished (transition to idle from active recording)
        if (event.payload.status === 'idle' && (prev.status === 'recording' || prev.status === 'paused' || prev.status === 'stopping')) {
          refreshHistory();
          setTimeout(() => refreshHistory(), 600);
        }
        return {
          ...prev,
          ...event.payload,
        };
      });
    });

    const unlistenVu = listen<AudioVUMeterPayload>('audio_vu_meter', (event) => {
      setRecordingStatus((prev) => ({
        ...prev,
        sys_vu_level: event.payload.sys_level_db,
        mic_vu_level: event.payload.mic_level_db,
        duration_secs: event.payload.duration_secs,
        size_bytes: event.payload.size_bytes,
      }));
    });

    const unlistenRegion = listen<RectRegion>('region_selected', (event) => {
      setSelectedRegion(event.payload);
    });

    const unlistenAutoStop = listen('auto_stop_triggered', () => {
      setRecordingStatus(initialRecordingStatus);
      refreshHistory();
      setCurrentTab('history');
    });

    return () => {
      unlistenStatus.then((u) => u());
      unlistenVu.then((u) => u());
      unlistenRegion.then((u) => u());
      unlistenAutoStop.then((u) => u());
    };
  }, [windowLabel]);

  const checkFfmpeg = (customPath?: string | null) => {
    invoke<string>('check_ffmpeg_status', { customFfmpegPath: customPath || null })
      .then(() => setFfmpegDetected(true))
      .catch(() => setFfmpegDetected(false));
  };

  const refreshHistory = () => {
    setIsHistoryLoading(true);
    invoke<HistoryItem[]>('list_history_files')
      .then((items) => setHistoryItems(items))
      .catch((err) => console.error('Failed to list history files:', err))
      .finally(() => setIsHistoryLoading(false));
  };

  const handleUpdateSettings = async (partial: Partial<Settings>) => {
    const updated = { ...settings, ...partial };
    setSettings(updated);
    try {
      await invoke('save_settings', { settings: updated });
      if (partial.custom_ffmpeg_path !== undefined) {
        checkFfmpeg(updated.custom_ffmpeg_path);
      }
    } catch (err) {
      console.error('Failed to save settings:', err);
    }
  };

  const handleStartScreenRecord = async (region: RectRegion | null) => {
    try {
      await invoke('start_screen_record', {
        settings,
        region,
      });
      setRecordingStatus((prev) => ({
        ...prev,
        status: 'recording',
        mode: 'screen',
        duration_secs: 0,
      }));
    } catch (err) {
      alert(`화면 녹화 시작 실패: ${err}`);
    }
  };

  const handleStartAudioRecord = async () => {
    try {
      await invoke('start_audio_record', {
        settings,
      });
      setRecordingStatus((prev) => ({
        ...prev,
        status: 'recording',
        mode: 'audio',
        duration_secs: 0,
      }));
    } catch (err) {
      alert(`오디오 녹음 시작 실패: ${err}`);
    }
  };

  const handlePauseRecord = async () => {
    try {
      await invoke('pause_record');
      setRecordingStatus((prev) => ({ ...prev, status: 'paused' }));
    } catch (err) {
      console.error(err);
    }
  };

  const handleResumeRecord = async () => {
    try {
      await invoke('resume_record');
      setRecordingStatus((prev) => ({ ...prev, status: 'recording' }));
    } catch (err) {
      console.error(err);
    }
  };

  const handleStopRecord = async () => {
    try {
      setRecordingStatus((prev) => ({ ...prev, status: 'stopping' }));
      await invoke<string>('stop_record');
      setRecordingStatus(initialRecordingStatus);
      refreshHistory();
      setCurrentTab('history');
    } catch (err) {
      setRecordingStatus(initialRecordingStatus);
      const errMsg = String(err);
      if (errMsg.includes('No active recording')) {
        // Already stopped successfully, safe to ignore
        refreshHistory();
        setCurrentTab('history');
      } else {
        alert(`녹화 저장 실패: ${err}`);
      }
    }
  };

  const handleDeleteHistoryFile = async (path: string) => {
    try {
      await invoke('delete_history_file', { path });
      refreshHistory();
    } catch (err) {
      console.error(err);
    }
  };

  const handleOpenExplorer = async (path: string) => {
    try {
      await invoke('open_in_explorer', { path });
    } catch (err) {
      console.error(err);
    }
  };

  const handleOpenDefaultPlayer = async (path: string) => {
    try {
      await invoke('open_with_default_player', { path });
    } catch (err) {
      console.error(err);
    }
  };

  const handleSendToMerger = (selectedPaths: string[]) => {
    setJoinerInitialFiles(selectedPaths);
    setCurrentTab('merger');
  };

  const handleSendToConverter = (selectedPaths: string[]) => {
    setConverterInitialFiles(selectedPaths);
    setCurrentTab('converter');
  };

  const handleSendToSubtitle = (audioPath: string) => {
    setSubtitleInitialAudio(audioPath);
    setCurrentTab('subtitle');
  };

  const handleOpenSelectionOverlay = async () => {
    try {
      await invoke('show_selection_overlay');
    } catch (err) {
      alert(`영역 선택을 열 수 없습니다: ${err}`);
    }
  };

  // Window Routing
  if (windowLabel === 'selection-overlay') {
    return <SelectionOverlay />;
  }

  if (windowLabel === 'mini-controller') {
    return <MiniController />;
  }

  return (
    <div className="h-screen w-screen flex flex-col bg-slate-950 text-slate-100 overflow-hidden select-none">
      {/* Top Navigation */}
      <Navbar
        currentTab={currentTab}
        onSelectTab={(tab) => {
          setCurrentTab(tab);
          if (tab === 'history') {
            refreshHistory();
          }
        }}
        recordingStatus={recordingStatus}
        ffmpegDetected={ffmpegDetected}
      />

      {/* Main Tab Content */}
      <main className="flex-1 min-h-0 overflow-y-auto relative">
        {currentTab === 'screen' && (
          <ScreenRecorder
            settings={settings}
            recordingStatus={recordingStatus}
            selectedRegion={selectedRegion}
            onClearRegion={() => setSelectedRegion(null)}
            onOpenSelectionOverlay={handleOpenSelectionOverlay}
            onOpenSettings={() => setCurrentTab('settings')}
            onStartRecord={handleStartScreenRecord}
            onPauseRecord={handlePauseRecord}
            onResumeRecord={handleResumeRecord}
            onStopRecord={handleStopRecord}
          />
        )}

        {currentTab === 'audio' && (
          <AudioRecorder
            settings={settings}
            recordingStatus={recordingStatus}
            onOpenSettings={() => setCurrentTab('settings')}
            onStartRecord={handleStartAudioRecord}
            onPauseRecord={handlePauseRecord}
            onResumeRecord={handleResumeRecord}
            onStopRecord={handleStopRecord}
          />
        )}

        {currentTab === 'history' && (
          <HistoryList
            items={historyItems}
            isLoading={isHistoryLoading}
            onRefresh={refreshHistory}
            onDeleteFile={handleDeleteHistoryFile}
            onOpenExplorer={handleOpenExplorer}
            onOpenDefaultPlayer={handleOpenDefaultPlayer}
            onSendToMerger={handleSendToMerger}
            onSendToConverter={handleSendToConverter}
            onSendToSubtitle={handleSendToSubtitle}
          />
        )}

        {currentTab === 'merger' && (
          <MediaJoiner
            settings={settings}
            initialFiles={joinerInitialFiles}
            onOpenExplorer={handleOpenExplorer}
            onOpenDefaultPlayer={handleOpenDefaultPlayer}
          />
        )}

        {currentTab === 'converter' && (
          <AudioConverter
            settings={settings}
            initialFiles={converterInitialFiles}
            onOpenExplorer={handleOpenExplorer}
            onOpenDefaultPlayer={handleOpenDefaultPlayer}
            onNavigateToHistory={() => {
              refreshHistory();
              setCurrentTab('history');
            }}
            onSendToSubtitle={handleSendToSubtitle}
          />
        )}

        {currentTab === 'subtitle' && (
          <SubtitleGenerator
            settings={settings}
            initialAudioPath={subtitleInitialAudio}
            onOpenExplorer={handleOpenExplorer}
            onSettingsChange={handleUpdateSettings}
          />
        )}

        {currentTab === 'settings' && (
          <SettingsView
            settings={settings}
            onUpdateSettings={handleUpdateSettings}
            ffmpegDetected={ffmpegDetected}
            onRefreshFfmpeg={checkFfmpeg}
          />
        )}
      </main>

      {/* Settings Modal (Optional Quick Modal fallback) */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
        settings={settings}
        onSave={async (saved) => {
          setSettings(saved);
          await invoke('save_settings', { settings: saved });
          checkFfmpeg(saved.custom_ffmpeg_path);
          refreshHistory();
        }}
      />
    </div>
  );
};

export default App;
