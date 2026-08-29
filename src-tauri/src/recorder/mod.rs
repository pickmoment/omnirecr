pub mod audio;
pub mod screen;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::engine::AudioEngineEvent;
use crate::recorder::audio::AudioRecorderSession;
use crate::recorder::screen::ScreenRecorderSession;
use crate::types::{
    AudioVUMeterPayload, RecordingMode, RecordingStateStatus, RecordingStatus, RectRegion, Settings,
};

enum ActiveSession {
    Screen(ScreenRecorderSession),
    Audio(AudioRecorderSession),
}

pub struct RecorderController {
    session: Arc<Mutex<Option<ActiveSession>>>,
    status: Arc<Mutex<RecordingStateStatus>>,
    mode: Arc<Mutex<Option<RecordingMode>>>,
    start_time: Arc<Mutex<Option<Instant>>>,
    paused_accum: Arc<Mutex<Duration>>,
    pause_start: Arc<Mutex<Option<Instant>>>,
    is_auto_paused: Arc<AtomicBool>,
    output_path: Arc<Mutex<Option<PathBuf>>>,
    last_stopped_path: Arc<Mutex<Option<PathBuf>>>,
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    sys_vu_level: Arc<Mutex<f32>>,
    mic_vu_level: Arc<Mutex<f32>>,
}

impl RecorderController {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(RecordingStateStatus::Idle)),
            mode: Arc::new(Mutex::new(None)),
            start_time: Arc::new(Mutex::new(None)),
            paused_accum: Arc::new(Mutex::new(Duration::ZERO)),
            pause_start: Arc::new(Mutex::new(None)),
            is_auto_paused: Arc::new(AtomicBool::new(false)),
            output_path: Arc::new(Mutex::new(None)),
            last_stopped_path: Arc::new(Mutex::new(None)),
            app_handle: Arc::new(Mutex::new(None)),
            sys_vu_level: Arc::new(Mutex::new(-60.0)),
            mic_vu_level: Arc::new(Mutex::new(-60.0)),
        }
    }

    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock() = Some(handle);
    }

    pub fn start_screen(
        &self,
        settings: &Settings,
        region: Option<RectRegion>,
    ) -> Result<String, String> {
        let mut session_guard = self.session.lock();
        if session_guard.is_some() {
            return Err("A recording is already in progress.".to_string());
        }

        let (tx, rx): (Sender<AudioEngineEvent>, Receiver<AudioEngineEvent>) = channel();
        let session = ScreenRecorderSession::start(settings, region, tx)?;
        let path = session.output_path.clone();

        *session_guard = Some(ActiveSession::Screen(session));
        *self.status.lock() = RecordingStateStatus::Recording;
        *self.mode.lock() = Some(RecordingMode::Screen);
        *self.start_time.lock() = Some(Instant::now());
        *self.paused_accum.lock() = Duration::ZERO;
        *self.pause_start.lock() = None;
        self.is_auto_paused.store(false, Ordering::SeqCst);
        *self.output_path.lock() = Some(path.clone());

        self.spawn_event_listener(rx);
        self.spawn_status_ticker();

        Ok(path.to_string_lossy().to_string())
    }

    pub fn start_audio(
        &self,
        settings: &Settings,
        file_name_prefix: Option<String>,
    ) -> Result<String, String> {
        let mut session_guard = self.session.lock();
        if session_guard.is_some() {
            return Err("A recording is already in progress.".to_string());
        }

        let (tx, rx): (Sender<AudioEngineEvent>, Receiver<AudioEngineEvent>) = channel();
        let session = AudioRecorderSession::start(settings, tx, file_name_prefix.as_deref())?;
        let path = session.output_path.clone();

        *session_guard = Some(ActiveSession::Audio(session));
        *self.status.lock() = RecordingStateStatus::Recording;
        *self.mode.lock() = Some(RecordingMode::Audio);
        *self.start_time.lock() = Some(Instant::now());
        *self.paused_accum.lock() = Duration::ZERO;
        *self.pause_start.lock() = None;
        self.is_auto_paused.store(false, Ordering::SeqCst);
        *self.output_path.lock() = Some(path.clone());

        self.spawn_event_listener(rx);
        self.spawn_status_ticker();

        Ok(path.to_string_lossy().to_string())
    }

    pub fn pause(&self) -> Result<(), String> {
        let mut status = self.status.lock();
        if *status != RecordingStateStatus::Recording {
            return Err("Not currently recording.".to_string());
        }

        if let Some(session) = self.session.lock().as_ref() {
            match session {
                ActiveSession::Screen(s) => s.pause(),
                ActiveSession::Audio(s) => s.pause(),
            }
        }

        *status = RecordingStateStatus::Paused;
        *self.pause_start.lock() = Some(Instant::now());
        self.emit_status_change();
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        let mut status = self.status.lock();
        if *status != RecordingStateStatus::Paused {
            return Err("Not currently paused.".to_string());
        }

        if let Some(session) = self.session.lock().as_ref() {
            match session {
                ActiveSession::Screen(s) => s.resume(),
                ActiveSession::Audio(s) => s.resume(),
            }
        }

        if let Some(start) = self.pause_start.lock().take() {
            *self.paused_accum.lock() += start.elapsed();
        }

        *status = RecordingStateStatus::Recording;
        self.is_auto_paused.store(false, Ordering::SeqCst);
        self.emit_status_change();
        Ok(())
    }

    pub fn stop(&self) -> Result<String, String> {
        let mut status = self.status.lock();
        if *status == RecordingStateStatus::Idle {
            // Idempotent: Already cleanly stopped
            if let Some(path) = self.last_stopped_path.lock().as_ref() {
                return Ok(path.to_string_lossy().to_string());
            }
            return Ok("".to_string());
        }

        *status = RecordingStateStatus::Stopping;
        drop(status);
        self.emit_status_change();

        let session_opt = self.session.lock().take();
        let path = match session_opt {
            Some(ActiveSession::Screen(s)) => s.stop()?,
            Some(ActiveSession::Audio(s)) => s.stop()?,
            None => {
                *self.status.lock() = RecordingStateStatus::Idle;
                self.emit_status_change();
                if let Some(p) = self.last_stopped_path.lock().as_ref() {
                    return Ok(p.to_string_lossy().to_string());
                }
                return Ok("".to_string());
            }
        };

        *self.last_stopped_path.lock() = Some(path.clone());
        *self.status.lock() = RecordingStateStatus::Idle;
        *self.mode.lock() = None;
        *self.start_time.lock() = None;
        *self.paused_accum.lock() = Duration::ZERO;
        *self.pause_start.lock() = None;
        self.is_auto_paused.store(false, Ordering::SeqCst);
        *self.output_path.lock() = None;
        *self.sys_vu_level.lock() = -60.0;
        *self.mic_vu_level.lock() = -60.0;

        self.emit_status_change();

        // Restore main window & hide mini controller
        if let Some(app) = self.app_handle.lock().as_ref() {
            if let Some(mini_win) = app.get_webview_window("mini-controller") {
                let _ = mini_win.hide();
            }
            if let Some(main_win) = app.get_webview_window("main") {
                let _ = main_win.unminimize();
                let _ = main_win.show();
                let _ = main_win.set_focus();
            }
        }

        Ok(path.to_string_lossy().to_string())
    }

    /// 마지막으로 저장된 녹음 결과 경로(자동 종료 포함).
    pub fn last_recorded_path(&self) -> Option<String> {
        self.last_stopped_path
            .lock()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
    }

    pub fn get_status(&self) -> RecordingStatus {
        let status = *self.status.lock();
        let mode = *self.mode.lock();
        let is_auto_paused = self.is_auto_paused.load(Ordering::SeqCst);
        let output_file = self.output_path.lock().as_ref().map(|p| p.to_string_lossy().to_string());
        let sys_vu = *self.sys_vu_level.lock();
        let mic_vu = *self.mic_vu_level.lock();

        let duration_secs = match *self.start_time.lock() {
            Some(start) => {
                let total = start.elapsed();
                let paused = *self.paused_accum.lock();
                let current_pause = if status == RecordingStateStatus::Paused {
                    self.pause_start.lock().map(|s| s.elapsed()).unwrap_or(Duration::ZERO)
                } else {
                    Duration::ZERO
                };
                total.saturating_sub(paused).saturating_sub(current_pause).as_secs_f64()
            }
            None => 0.0,
        };

        let size_bytes = self.output_path.lock().as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);

        RecordingStatus {
            status,
            mode,
            duration_secs,
            size_bytes,
            is_auto_paused,
            output_file,
            sys_vu_level: sys_vu,
            mic_vu_level: mic_vu,
        }
    }

    fn emit_status_change(&self) {
        if let Some(app) = self.app_handle.lock().as_ref() {
            let status = self.get_status();
            let _ = app.emit("recording_status_change", &status);
        }
    }

    fn spawn_event_listener(&self, rx: Receiver<AudioEngineEvent>) {
        let session_arc = self.session.clone();
        let status_arc = self.status.clone();
        let mode_arc = self.mode.clone();
        let start_time_arc = self.start_time.clone();
        let is_auto_paused = self.is_auto_paused.clone();
        let pause_start = self.pause_start.clone();
        let paused_accum = self.paused_accum.clone();
        let output_path_arc = self.output_path.clone();
        let last_stopped_path_arc = self.last_stopped_path.clone();
        let sys_vu_arc = self.sys_vu_level.clone();
        let mic_vu_arc = self.mic_vu_level.clone();
        let app_handle_clone = self.app_handle.clone();

        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                match event {
                    AudioEngineEvent::AutoPause => {
                        let mut st = status_arc.lock();
                        if *st == RecordingStateStatus::Recording {
                            *st = RecordingStateStatus::Paused;
                            is_auto_paused.store(true, Ordering::SeqCst);
                            *pause_start.lock() = Some(Instant::now());

                            if let Some(app) = app_handle_clone.lock().as_ref() {
                                let _ = app.emit("auto_pause_triggered", true);
                                let size_bytes = output_path_arc.lock().as_ref()
                                    .and_then(|p| std::fs::metadata(p).ok())
                                    .map(|m| m.len())
                                    .unwrap_or(0);
                                let payload = RecordingStatus {
                                    status: RecordingStateStatus::Paused,
                                    mode: *mode_arc.lock(),
                                    duration_secs: 0.0,
                                    size_bytes,
                                    is_auto_paused: true,
                                    output_file: output_path_arc.lock().as_ref().map(|p| p.to_string_lossy().to_string()),
                                    sys_vu_level: *sys_vu_arc.lock(),
                                    mic_vu_level: *mic_vu_arc.lock(),
                                };
                                let _ = app.emit("recording_status_change", &payload);
                            }
                        }
                    }
                    AudioEngineEvent::AutoResume => {
                        let mut st = status_arc.lock();
                        if *st == RecordingStateStatus::Paused && is_auto_paused.load(Ordering::SeqCst) {
                            *st = RecordingStateStatus::Recording;
                            is_auto_paused.store(false, Ordering::SeqCst);

                            if let Some(start) = pause_start.lock().take() {
                                *paused_accum.lock() += start.elapsed();
                            }

                            if let Some(app) = app_handle_clone.lock().as_ref() {
                                let _ = app.emit("auto_resume_triggered", true);
                                let size_bytes = output_path_arc.lock().as_ref()
                                    .and_then(|p| std::fs::metadata(p).ok())
                                    .map(|m| m.len())
                                    .unwrap_or(0);
                                let payload = RecordingStatus {
                                    status: RecordingStateStatus::Recording,
                                    mode: *mode_arc.lock(),
                                    duration_secs: 0.0,
                                    size_bytes,
                                    is_auto_paused: false,
                                    output_file: output_path_arc.lock().as_ref().map(|p| p.to_string_lossy().to_string()),
                                    sys_vu_level: *sys_vu_arc.lock(),
                                    mic_vu_level: *mic_vu_arc.lock(),
                                };
                                let _ = app.emit("recording_status_change", &payload);
                            }
                        }
                    }
                    AudioEngineEvent::AutoStop => {
                        let mut st = status_arc.lock();
                        if *st == RecordingStateStatus::Recording || *st == RecordingStateStatus::Paused {
                            *st = RecordingStateStatus::Stopping;
                            drop(st);
                            let session_opt = session_arc.lock().take();
                            if let Some(session) = session_opt {
                                if let Ok(path) = match session {
                                    ActiveSession::Screen(s) => s.stop(),
                                    ActiveSession::Audio(s) => s.stop(),
                                } {
                                    *last_stopped_path_arc.lock() = Some(path);
                                }
                            }

                            *status_arc.lock() = RecordingStateStatus::Idle;
                            *mode_arc.lock() = None;
                            *start_time_arc.lock() = None;
                            *paused_accum.lock() = Duration::ZERO;
                            *pause_start.lock() = None;
                            is_auto_paused.store(false, Ordering::SeqCst);
                            *output_path_arc.lock() = None;
                            *sys_vu_arc.lock() = -60.0;
                            *mic_vu_arc.lock() = -60.0;

                            if let Some(app) = app_handle_clone.lock().as_ref() {
                                if let Some(mini_win) = app.get_webview_window("mini-controller") {
                                    let _ = mini_win.hide();
                                }
                                if let Some(main_win) = app.get_webview_window("main") {
                                    let _ = main_win.unminimize();
                                    let _ = main_win.show();
                                    let _ = main_win.set_focus();
                                }
                                let payload = RecordingStatus {
                                    status: RecordingStateStatus::Idle,
                                    mode: None,
                                    duration_secs: 0.0,
                                    size_bytes: 0,
                                    is_auto_paused: false,
                                    output_file: None,
                                    sys_vu_level: -60.0,
                                    mic_vu_level: -60.0,
                                };
                                let _ = app.emit("recording_status_change", &payload);
                                // 자동 종료로 저장된 결과 파일 경로를 함께 전달해
                                // TTS 녹음 워크플로우가 결과를 대본에 연결할 수 있게 한다.
                                let saved_path = last_stopped_path_arc
                                    .lock()
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().to_string());
                                let _ = app.emit("auto_stop_triggered", saved_path);
                            }
                            break;
                        }
                    }
                }
            }
        });
    }

    fn spawn_status_ticker(&self) {
        let session_arc = self.session.clone();
        let status_arc = self.status.clone();
        let start_time_arc = self.start_time.clone();
        let pause_start_arc = self.pause_start.clone();
        let paused_accum_arc = self.paused_accum.clone();
        let output_path_arc = self.output_path.clone();
        let sys_vu_arc = self.sys_vu_level.clone();
        let mic_vu_arc = self.mic_vu_level.clone();
        let app_handle_clone = self.app_handle.clone();

        thread::spawn(move || {
            while *status_arc.lock() != RecordingStateStatus::Idle {
                let vu = if let Some(session) = session_arc.lock().as_ref() {
                    match session {
                        ActiveSession::Screen(s) => s.get_vu_levels(),
                        ActiveSession::Audio(s) => s.get_vu_levels(),
                    }
                } else {
                    (-60.0, -60.0)
                };

                *sys_vu_arc.lock() = vu.0;
                *mic_vu_arc.lock() = vu.1;

                let st = *status_arc.lock();
                let duration_secs = match *start_time_arc.lock() {
                    Some(start) => {
                        let total = start.elapsed();
                        let paused = *paused_accum_arc.lock();
                        let current_pause = if st == RecordingStateStatus::Paused {
                            pause_start_arc.lock().map(|s| s.elapsed()).unwrap_or(Duration::ZERO)
                        } else {
                            Duration::ZERO
                        };
                        total.saturating_sub(paused).saturating_sub(current_pause).as_secs_f64()
                    }
                    None => 0.0,
                };

                let size_bytes = output_path_arc.lock().as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);

                if let Some(app) = app_handle_clone.lock().as_ref() {
                    let payload = AudioVUMeterPayload {
                        sys_level_db: vu.0,
                        mic_level_db: vu.1,
                        is_silent: vu.0 <= -45.0 && vu.1 <= -45.0,
                        duration_secs,
                        size_bytes,
                    };
                    let _ = app.emit("audio_vu_meter", &payload);
                }

                thread::sleep(Duration::from_millis(50));
            }
        });
    }
}
