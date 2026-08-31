import React, { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

import type {
  TabType,
  FilesView,
  Settings,
  RecordingStatus,
  HistoryItem,
  RectRegion,
  AudioVUMeterPayload,
  ScriptRecordingTarget,
} from './types';
import type { StartTtsRecordOptions } from './components/TtsRecorder';
import { Navbar } from './components/Navbar';
import { RecordStudio } from './components/RecordStudio';
import { FileStudio } from './components/FileStudio';
import { SubtitleStudio } from './components/SubtitleStudio';
import { ScriptStudio } from './components/ScriptStudio';
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
  system_audio_include_own_app: false,
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
  typecast_editor_url: 'https://studio.typecast.ai/text-to-speech',
  typecast_signin_url: 'https://studio.typecast.ai/sign-in',
  custom_chrome_path: null,
  typecast_account_email: null,
  typecast_session_saved: false,
  typecast_last_login_at: null,
  typecast_editor_selector: '',
  typecast_play_selector: '',
  tts_countdown_secs: 3,
  tts_mic_enabled: false,
  tts_auto_stop_seconds: 4.0,
  tts_speech_threshold_db: -45.0,
  tts_start_timeout_secs: 25,
  tts_gap_secs: 2,
  tts_batch_continue_on_error: true,
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

/**
 * 설정 저장 디바운스 간격. 슬라이더를 드래그하면 변경이 수십 번 쏟아지는데
 * 그때마다 파일을 쓰면 디스크 I/O 가 UI 를 따라오지 못한다. 너무 길게 잡으면
 * 마지막 변경이 앱 종료와 겹쳐 유실될 수 있어 짧게 유지한다.
 */
const SETTINGS_SAVE_DEBOUNCE_MS = 120;

export const App: React.FC = () => {
  const [windowLabel] = useState<string>(() => {
    try {
      return getCurrentWebviewWindow()?.label || 'main';
    } catch {
      return 'main';
    }
  });

  const [currentTab, setCurrentTab] = useState<TabType>('record');
  // 이벤트 리스너 콜백이 최신 탭을 볼 수 있도록 ref 로도 보관한다.
  const currentTabRef = useRef<TabType>('record');
  useEffect(() => {
    currentTabRef.current = currentTab;
  }, [currentTab]);
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [recordingStatus, setRecordingStatus] = useState<RecordingStatus>(initialRecordingStatus);
  const [selectedRegion, setSelectedRegion] = useState<RectRegion | null>(null);

  // 설정 저장 경합 방지 장치.
  // (1) settingsRef: 다음 값을 "가장 최신" 스냅샷에서 계산한다. 렌더에서 캡처한
  //     settings 위에 부분 변경을 얹으면, 다음 렌더 전에 두 컨트롤이 바뀔 때 두 호출이
  //     같은 낡은 기반을 공유해 나중 저장이 앞의 변경을 되돌린다.
  // (2) saveQueueRef: 저장 IPC 를 한 줄로 직렬화한다. 나란히 보내면 응답 순서가
  //     뒤바뀌어 디스크에 남는 최종 상태가 호출 순서와 어긋날 수 있다.
  // (3) pendingFlushRef + saveTimerRef: 짧은 간격의 변경은 한 번으로 합쳐 보내되,
  //     합쳐진 호출자 전원에게 같은 완료 프라미스를 돌려준다
  //     (TypecastSessionCard 처럼 `await onUpdateSettings(...)` 하는 호출자가 있다).
  const settingsRef = useRef<Settings>(defaultSettings);
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const saveTimerRef = useRef<number | null>(null);
  const pendingFlushRef = useRef<{ promise: Promise<void>; fire: () => void } | null>(null);
  const ffmpegRecheckRef = useRef(false);
  // 녹음 상태를 이벤트 콜백에서도 읽을 수 있게 ref 로 미러링한다.
  const recordingStatusRef = useRef<RecordingStatus>(initialRecordingStatus);
  useEffect(() => {
    recordingStatusRef.current = recordingStatus;
  }, [recordingStatus]);
  // 녹음 종료 직후 한 번 더 도는 히스토리 새로고침 타이머. 언마운트 때 끊어야
  // 사라진 컴포넌트에 결과가 날아오지 않는다.
  const delayedHistoryTimerRef = useRef<number | null>(null);

  const [historyItems, setHistoryItems] = useState<HistoryItem[]>([]);
  const [isHistoryLoading, setIsHistoryLoading] = useState(false);

  const [joinerInitialFiles, setJoinerInitialFiles] = useState<string[]>([]);
  const [converterInitialFiles, setConverterInitialFiles] = useState<string[]>([]);
  // 히스토리에서 병합 · 변환으로 보낼 때 파일 탭의 서브 화면을 지정한다.
  const [filesView, setFilesView] = useState<FilesView | null>(null);
  const [subtitleInitialAudio, setSubtitleInitialAudio] = useState<string | null>(null);
  const [subtitleInitialScript, setSubtitleInitialScript] = useState<string | null>(null);
  const [ffmpegDetected, setFfmpegDetected] = useState(false);

  // Initialize app if in main window
  useEffect(() => {
    if (windowLabel !== 'main') return;

    // 1. Load settings
    invoke<Settings>('get_settings')
      .then((loaded) => {
        // 저장 경로가 참조하는 스냅샷도 함께 갱신한다 — 여기서 빼먹으면 첫 설정
        // 변경이 기본값 위에 얹혀 저장되어 불러온 설정이 통째로 날아간다.
        settingsRef.current = loaded;
        setSettings(loaded);
        refreshHistory();
        checkFfmpeg(loaded.custom_ffmpeg_path);
      })
      .catch((err) => console.error('Failed to load settings:', err));

    // 2. Event listeners
    const unlistenStatus = listen<RecordingStatus>('recording_status_change', (event) => {
      // 전환 판단과 부수효과는 상태 갱신 함수 **밖** 에서 한다. React StrictMode 는
      // 갱신 함수를 두 번 호출하므로, 안에서 새로고침을 걸면 IPC 와 타이머가 두 번 뜬다.
      const prevStatus = recordingStatusRef.current.status;
      const justFinished =
        event.payload.status === 'idle' &&
        (prevStatus === 'recording' || prevStatus === 'paused' || prevStatus === 'stopping');

      // 미러도 렌더를 기다리지 않고 즉시 맞춘다. 한 틱에 상태 이벤트가 두 번
      // 도착하면(즉사한 녹음처럼) 두 번째 판단이 낡은 값을 보게 된다.
      recordingStatusRef.current = { ...recordingStatusRef.current, ...event.payload };
      setRecordingStatus((prev) => ({
        ...prev,
        ...event.payload,
      }));

      if (justFinished) {
        refreshHistory();
        // FFmpeg 가 컨테이너를 닫고 파일이 목록에 보이기까지 잠깐 걸린다.
        if (delayedHistoryTimerRef.current !== null) {
          window.clearTimeout(delayedHistoryTimerRef.current);
        }
        delayedHistoryTimerRef.current = window.setTimeout(() => {
          delayedHistoryTimerRef.current = null;
          refreshHistory();
        }, 600);
      }
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

    const unlistenAutoStop = listen<string | null>('auto_stop_triggered', () => {
      recordingStatusRef.current = initialRecordingStatus;
      setRecordingStatus(initialRecordingStatus);
      refreshHistory();
      // 대본 스튜디오에서 TTS 낭독을 녹음하는 중이라면 화면을 그대로 유지한다.
      if (currentTabRef.current !== 'script') {
        setCurrentTab('files');
      }
    });

    // 캡처·인코딩이 사실상 죽은 경우 백엔드가 보내는 알림(계약 C4).
    // 예전에는 이런 실패가 조용히 묻혀 UI 는 계속 "녹음 중" 을 보여 주고
    // 사용자는 나중에 빈 파일을 발견했다.
    const unlistenFailed = listen<string>('recording_failed', (event) => {
      recordingStatusRef.current = initialRecordingStatus;
      setRecordingStatus(initialRecordingStatus);
      refreshHistory();
      // alert 는 JS 를 멈춰 세우므로 위 상태 갱신이 화면에 그려진 다음에 띄운다.
      // 그렇지 않으면 모달 뒤에 "녹음 중" 화면이 그대로 남아 더 헷갈린다.
      window.setTimeout(() => alert(`녹음이 중단되었습니다: ${event.payload}`), 0);
    });

    return () => {
      unlistenStatus.then((u) => u());
      unlistenVu.then((u) => u());
      unlistenRegion.then((u) => u());
      unlistenAutoStop.then((u) => u());
      unlistenFailed.then((u) => u());
      if (delayedHistoryTimerRef.current !== null) {
        window.clearTimeout(delayedHistoryTimerRef.current);
        delayedHistoryTimerRef.current = null;
      }
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

  /**
   * 디바운스가 만료되면 최신 스냅샷을 직렬 큐 뒤에 붙여 저장한다.
   * 대기 중인 flush 가 있으면 새로 만들지 않고 그 완료 프라미스를 재사용한다.
   */
  const scheduleSettingsSave = (): Promise<void> => {
    let pending = pendingFlushRef.current;
    if (!pending) {
      let fire: () => void = () => {};
      const armed = new Promise<void>((resolve) => {
        fire = resolve;
      });
      const promise = armed.then(() => {
        pendingFlushRef.current = null;
        // 큐에 넣는 시점의 최신 스냅샷 — 그 사이 들어온 변경까지 한 번에 나간다.
        const snapshot = settingsRef.current;
        const recheckFfmpeg = ffmpegRecheckRef.current;
        ffmpegRecheckRef.current = false;
        // 여기서 예외를 밖으로 흘리지 않는다. 한 번 reject 되면 큐 프라미스가
        // 오염되어 이후 저장이 전부 건너뛰어진다.
        const job = saveQueueRef.current.then(async () => {
          try {
            await invoke('save_settings', { settings: snapshot });
            if (recheckFfmpeg) {
              checkFfmpeg(snapshot.custom_ffmpeg_path);
            }
          } catch (err) {
            console.error('Failed to save settings:', err);
            alert(`설정을 저장하지 못했습니다: ${err}\n\n앱을 다시 켜면 이 변경은 사라집니다.`);
          }
        });
        saveQueueRef.current = job;
        return job;
      });
      pending = { promise, fire };
      pendingFlushRef.current = pending;
    }

    if (saveTimerRef.current !== null) {
      window.clearTimeout(saveTimerRef.current);
    }
    const firePending = pending.fire;
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null;
      firePending();
    }, SETTINGS_SAVE_DEBOUNCE_MS);

    return pending.promise;
  };

  // 대기 중이던 저장을 언마운트 때 즉시 밀어낸다. 없으면 마지막 변경이 유실된다.
  useEffect(() => {
    return () => {
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
      pendingFlushRef.current?.fire();
    };
  }, []);

  /**
   * 설정 부분 변경 → 최신 스냅샷에 병합 → 디바운스 후 직렬 큐로 저장.
   *
   * 반환 프라미스는 이 변경이 실제로 저장된 뒤 resolve 하고, 실패해도 reject 하지
   * 않는다. 호출자 대부분이 이벤트 핸들러라서 await 하지 않으므로, reject 로 알리면
   * 처리되지 않은 예외로만 남고 사용자는 저장 실패를 끝까지 모른다.
   */
  const handleUpdateSettings = (partial: Partial<Settings>): Promise<void> => {
    const next = { ...settingsRef.current, ...partial };
    settingsRef.current = next;
    setSettings(next);
    if (partial.custom_ffmpeg_path !== undefined) {
      ffmpegRecheckRef.current = true;
    }
    return scheduleSettingsSave();
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

  /**
   * 녹음을 시작하고 저장될 파일 경로를 반환한다. 실패 또는 사용자가 덮어쓰기를
   * 취소하면 null.
   *
   * `exactFileName` 요청(대본 & TTS 녹음)은 시작 전에 같은 이름의 파일이 이미
   * 있는지 확인해 덮어쓰기 확인을 받는다. 실제 저장 경로 계산은 Rust 쪽
   * `AudioRecorderSession::resolve_output_path` 하나로 모아 어긋나지 않게 한다.
   *
   * 자동 일괄 녹음은 시작 전에 대상 전체의 덮어쓰기를 한 번에 확인하므로
   * `skipOverwriteCheck` 로 이 확인을 건너뛴다 — 실행 도중 모달이 뜨면 배치가
   * 사용자가 볼 수도 없는 창 뒤에서 멈춘다. 같은 이유로 `throwOnError` 를 주면
   * `alert` 대신 예외를 던져, 호출한 화면이 실패 사유를 항목 옆에 인라인으로 보여줄 수 있다.
   */
  const handleStartAudioRecord = async (
    options?: StartTtsRecordOptions,
  ): Promise<string | null> => {
    const mergedSettings = { ...settings, ...(options?.settingsOverride ?? {}) };
    try {
      if (options?.exactFileName && options.fileNamePrefix && !options.skipOverwriteCheck) {
        const [target] = await invoke<ScriptRecordingTarget[]>(
          'resolve_script_recording_targets',
          { settings: mergedSettings, fileNamePrefixes: [options.fileNamePrefix] },
        );
        if (
          target?.exists &&
          !window.confirm(
            `이미 같은 이름으로 저장된 파일이 있습니다:\n${target.path}\n\n덮어쓸까요?`,
          )
        ) {
          return null;
        }
      }
      const outputPath = await invoke<string>('start_audio_record', {
        settings: mergedSettings,
        fileNamePrefix: options?.fileNamePrefix ?? null,
        showMiniController: options?.showMiniController ?? false,
        exactFileName: options?.exactFileName ?? false,
      });
      setRecordingStatus((prev) => ({
        ...prev,
        status: 'recording',
        mode: 'audio',
        duration_secs: 0,
      }));
      return outputPath || null;
    } catch (err) {
      if (options?.throwOnError) throw err;
      alert(`오디오 녹음 시작 실패: ${err}`);
      return null;
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

  // TTS 낭독 녹음 중에는 결과 연결 UI를 계속 보여줘야 하므로 탭을 바꾸지 않는다.
  const navigateToHistoryUnlessScriptStudio = () => {
    if (currentTabRef.current !== 'script') {
      setCurrentTab('files');
    }
  };

  /**
   * `silent` 는 자동 일괄 녹음용이다. 저장 실패를 `alert` 로 띄우면 배치 루프가
   * 사용자가 보지 못하는 모달 앞에서 멈추므로, 대신 예외를 던져 러너가 해당 대본
   * 항목에 실패 사유를 표시하고 다음 대본으로 넘어가게 한다.
   */
  const handleStopRecord = async (options?: { silent?: boolean }) => {
    try {
      setRecordingStatus((prev) => ({ ...prev, status: 'stopping' }));
      await invoke<string>('stop_record');
      setRecordingStatus(initialRecordingStatus);
      refreshHistory();
      navigateToHistoryUnlessScriptStudio();
    } catch (err) {
      setRecordingStatus(initialRecordingStatus);
      const errMsg = String(err);
      if (errMsg.includes('No active recording')) {
        // Already stopped successfully, safe to ignore
        refreshHistory();
        navigateToHistoryUnlessScriptStudio();
        return;
      }
      if (options?.silent) throw err;
      alert(`녹화 저장 실패: ${err}`);
    }
  };

  /**
   * 삭제 실패를 호출한 화면으로 그대로 던진다. 예전에는 `console.error` 로만 남겨서,
   * 백엔드가 삭제를 거부해도 목록은 그대로 남고 사용자는 지워진 줄 알았다.
   * 목록 새로고침은 성공·실패와 무관하게 한 번 돈다 — "파일을 찾을 수 없습니다" 라면
   * 그 항목은 이미 사라진 것이므로 목록에서도 걷어내야 한다.
   */
  const handleDeleteHistoryFile = async (path: string) => {
    try {
      await invoke('delete_history_file', { path });
    } finally {
      refreshHistory();
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
    setFilesView('merger');
    setCurrentTab('files');
  };

  const handleSendToConverter = (selectedPaths: string[]) => {
    setConverterInitialFiles(selectedPaths);
    setFilesView('converter');
    setCurrentTab('files');
  };

  const handleSendToSubtitle = (audioPath: string, scriptText?: string) => {
    setSubtitleInitialAudio(audioPath);
    setSubtitleInitialScript(scriptText ?? null);
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
          if (tab === 'files') {
            refreshHistory();
          }
        }}
        recordingStatus={recordingStatus}
        ffmpegDetected={ffmpegDetected}
      />

      {/* Main Tab Content */}
      <main className="flex-1 min-h-0 overflow-y-auto relative">
        {currentTab === 'record' && (
          <RecordStudio
            settings={settings}
            recordingStatus={recordingStatus}
            selectedRegion={selectedRegion}
            onClearRegion={() => setSelectedRegion(null)}
            onOpenSelectionOverlay={handleOpenSelectionOverlay}
            onOpenSettings={() => setCurrentTab('settings')}
            onStartScreenRecord={handleStartScreenRecord}
            onStartAudioRecord={async () => {
              await handleStartAudioRecord();
            }}
            onPauseRecord={handlePauseRecord}
            onResumeRecord={handleResumeRecord}
            onStopRecord={handleStopRecord}
          />
        )}

        {currentTab === 'files' && (
          <FileStudio
            settings={settings}
            historyItems={historyItems}
            isHistoryLoading={isHistoryLoading}
            joinerInitialFiles={joinerInitialFiles}
            converterInitialFiles={converterInitialFiles}
            onRefreshHistory={refreshHistory}
            onDeleteFile={handleDeleteHistoryFile}
            onOpenExplorer={handleOpenExplorer}
            onOpenDefaultPlayer={handleOpenDefaultPlayer}
            onSendToMerger={handleSendToMerger}
            onSendToConverter={handleSendToConverter}
            onSendToSubtitle={handleSendToSubtitle}
            requestedView={filesView}
            onViewHandled={() => setFilesView(null)}
          />
        )}

        <div className={currentTab === 'script' ? 'h-full' : 'hidden'}>
          <ScriptStudio
            settings={settings}
            recordingStatus={recordingStatus}
            onUpdateSettings={handleUpdateSettings}
            onStartRecord={handleStartAudioRecord}
            onPauseRecord={handlePauseRecord}
            onResumeRecord={handleResumeRecord}
            onStopRecord={handleStopRecord}
            onSendToSubtitle={handleSendToSubtitle}
            onOpenExplorer={handleOpenExplorer}
            onOpenDefaultPlayer={handleOpenDefaultPlayer}
            onOpenSettings={() => setCurrentTab('settings')}
          />
        </div>

        {currentTab === 'subtitle' && (
          <SubtitleStudio
            settings={settings}
            initialAudioPath={subtitleInitialAudio}
            initialScriptText={subtitleInitialScript}
            onOpenExplorer={handleOpenExplorer}
            onSettingsChange={handleUpdateSettings}
            onOpenSettings={() => setCurrentTab('settings')}
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

    </div>
  );
};

export default App;
