pub mod audio;
pub mod screen;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};

use crate::audio::engine::AudioEngineEvent;
use crate::recorder::audio::AudioRecorderSession;
use crate::recorder::screen::ScreenRecorderSession;
use crate::types::{
    AudioVUMeterPayload, RecordingMode, RecordingStateStatus, RecordingStatus, RectRegion,
    SelectionScreenInfo, Settings,
};

enum ActiveSession {
    Screen(ScreenRecorderSession),
    Audio(AudioRecorderSession),
}

impl ActiveSession {
    fn pause(&self) {
        match self {
            Self::Screen(s) => s.pause(),
            Self::Audio(s) => s.pause(),
        }
    }

    fn resume(&self) {
        match self {
            Self::Screen(s) => s.resume(),
            Self::Audio(s) => s.resume(),
        }
    }

    fn vu_levels(&self) -> (f32, f32) {
        match self {
            Self::Screen(s) => s.get_vu_levels(),
            Self::Audio(s) => s.get_vu_levels(),
        }
    }

    fn stop(self) -> Result<PathBuf, String> {
        match self {
            Self::Screen(s) => s.stop(),
            Self::Audio(s) => s.stop(),
        }
    }
}

/// 녹음/녹화 한 건을 둘러싼 공유 상태.
///
/// **잠금 순서는 항상 `status` → `session` 이다.** 두 잠금을 동시에 잡는 곳이
/// 시작 경로와 정지 경로 둘인데, 순서를 뒤집으면 ABBA 교착이 난다.
///
/// **어떤 잠금이든 잡은 채로 `status_snapshot()` / `emit_status()` 를 부르지 말 것.**
/// `parking_lot::Mutex` 는 재진입이 불가능해 같은 스레드에서 다시 잠그면 자기 자신을
/// 영구히 기다린다. 실제로 `pause()`/`resume()` 이 `status` 가드를 들고 상태 스냅샷을
/// 만들다가 앱 전체(동기 커맨드 = Tauri 메인 스레드)가 얼어붙는 버그가 있었다.
struct SharedState<R: Runtime> {
    session: Mutex<Option<ActiveSession>>,
    status: Mutex<RecordingStateStatus>,
    mode: Mutex<Option<RecordingMode>>,
    start_time: Mutex<Option<Instant>>,
    paused_accum: Mutex<Duration>,
    pause_start: Mutex<Option<Instant>>,
    /// 무음 자동 일시정지로 멈춘 상태인지. 수동 일시정지와 구분해야 자동 재개가
    /// 사용자의 수동 일시정지를 되돌리지 않는다.
    is_auto_paused: AtomicBool,
    output_path: Mutex<Option<PathBuf>>,
    last_stopped_path: Mutex<Option<PathBuf>>,
    app_handle: Mutex<Option<AppHandle<R>>>,
    sys_vu_level: Mutex<f32>,
    mic_vu_level: Mutex<f32>,
    /// 상태 틱커 세대 번호. 녹음이 새로 시작될 때마다 올라가고, 틱커는 자기 세대가
    /// 아직 최신일 때만 계속 돈다. `status != Idle` 만 보고 돌면, 한 녹음이 끝난 뒤
    /// 50ms 슬립에서 깨어나기 전에 다음 녹음이 시작될 경우(대본 자동 일괄 녹음처럼
    /// 연속으로 녹음할 때) 이전 틱커가 죽지 않고 남아 `audio_vu_meter` 이벤트를
    /// 중복으로 쏘는 스레드가 대본 수만큼 쌓인다.
    ticker_generation: AtomicU64,
}

impl<R: Runtime> SharedState<R> {
    fn new() -> Self {
        Self {
            session: Mutex::new(None),
            status: Mutex::new(RecordingStateStatus::Idle),
            mode: Mutex::new(None),
            start_time: Mutex::new(None),
            paused_accum: Mutex::new(Duration::ZERO),
            pause_start: Mutex::new(None),
            is_auto_paused: AtomicBool::new(false),
            output_path: Mutex::new(None),
            last_stopped_path: Mutex::new(None),
            app_handle: Mutex::new(None),
            sys_vu_level: Mutex::new(-60.0),
            mic_vu_level: Mutex::new(-60.0),
            ticker_generation: AtomicU64::new(0),
        }
    }

    /// 이벤트를 보낼 앱 핸들. **핸들을 복제해 잠금을 즉시 놓는다** — 잠금을 든 채로
    /// `emit` 하면 프론트엔드 직렬화 도중 다른 스레드가 상태를 못 읽는다.
    fn app(&self) -> Option<AppHandle<R>> {
        self.app_handle.lock().clone()
    }

    /// 경과 시간. 일시정지 누적과 현재 일시정지 구간을 뺀 값이다.
    /// 호출자는 `status` 잠금을 **놓은 뒤** 상태 값을 넘겨야 한다.
    fn duration_secs(&self, status: RecordingStateStatus) -> f64 {
        let Some(start) = *self.start_time.lock() else {
            return 0.0;
        };
        let total = start.elapsed();
        let paused = *self.paused_accum.lock();
        let current_pause = if status == RecordingStateStatus::Paused {
            self.pause_start
                .lock()
                .map(|s| s.elapsed())
                .unwrap_or(Duration::ZERO)
        } else {
            Duration::ZERO
        };
        total
            .saturating_sub(paused)
            .saturating_sub(current_pause)
            .as_secs_f64()
    }

    fn size_bytes(&self) -> u64 {
        self.output_path
            .lock()
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// 프론트엔드로 나가는 상태 스냅샷을 만드는 **유일한 곳**. 예전에는 커맨드·틱커·
    /// 자동 일시정지/재개/종료가 각자 만들어, 자동 일시정지 payload 의
    /// `duration_secs` 가 0.0 으로 하드코딩돼 타이머가 0 으로 튀는 버그가 있었다.
    fn status_snapshot(&self) -> RecordingStatus {
        let status = *self.status.lock();
        RecordingStatus {
            status,
            mode: *self.mode.lock(),
            duration_secs: self.duration_secs(status),
            size_bytes: self.size_bytes(),
            is_auto_paused: self.is_auto_paused.load(Ordering::SeqCst),
            output_file: self
                .output_path
                .lock()
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            sys_vu_level: *self.sys_vu_level.lock(),
            mic_vu_level: *self.mic_vu_level.lock(),
        }
    }

    fn emit_status(&self) {
        if let Some(app) = self.app() {
            let _ = app.emit("recording_status_change", self.status_snapshot());
        }
    }

    fn reset_to_idle(&self) {
        *self.status.lock() = RecordingStateStatus::Idle;
        *self.mode.lock() = None;
        *self.start_time.lock() = None;
        *self.paused_accum.lock() = Duration::ZERO;
        *self.pause_start.lock() = None;
        self.is_auto_paused.store(false, Ordering::SeqCst);
        *self.output_path.lock() = None;
        *self.sys_vu_level.lock() = -60.0;
        *self.mic_vu_level.lock() = -60.0;
    }

    fn last_stopped(&self) -> String {
        self.last_stopped_path
            .lock()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 진행 중인 세션을 정지하고 공유 상태를 Idle 로 되돌린다.
    /// 수동 정지 · 무음 자동 종료 · 캡처 치명적 실패가 모두 이 한 곳을 쓴다.
    ///
    /// `None` 은 "정지할 것이 없었다"(이미 Idle 이거나 다른 경로가 이미 Stopping 으로
    /// 전이시켰다)는 뜻이다. `Some` 이면 **성공이든 실패든 상태는 Idle 로 되돌아간다** —
    /// 여기서 에러로 조기 반환하면 status 가 Stopping 에 박혀 이후 어떤 녹음도 시작할
    /// 수 없다.
    fn finish_active_session(&self) -> Option<Result<PathBuf, String>> {
        let session_opt = {
            let mut status = self.status.lock();
            if *status == RecordingStateStatus::Idle || *status == RecordingStateStatus::Stopping {
                return None;
            }
            *status = RecordingStateStatus::Stopping;
            self.session.lock().take()
        };

        self.emit_status();

        let outcome = match session_opt {
            Some(session) => session.stop(),
            None => Err("정지할 녹음 세션이 없습니다.".to_string()),
        };
        if let Ok(path) = &outcome {
            *self.last_stopped_path.lock() = Some(path.clone());
        }
        self.reset_to_idle();
        Some(outcome)
    }
}

/// 런타임 타입 매개변수는 **테스트용**이다. 기본값이 `tauri::Wry` 라 프로덕션 코드는
/// 그냥 `RecorderController` 로 쓰면 되고, 테스트는 `MockRuntime` 으로 실제 `AppHandle`
/// 을 붙여 상태 이벤트 경로(예전에 앱 전체를 얼렸던 자기 교착)를 그대로 태울 수 있다.
pub struct RecorderController<R: Runtime = tauri::Wry> {
    shared: Arc<SharedState<R>>,
}

impl<R: Runtime> Default for RecorderController<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Runtime> RecorderController<R> {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(SharedState::new()),
        }
    }

    pub fn set_app_handle(&self, handle: AppHandle<R>) {
        *self.shared.app_handle.lock() = Some(handle);
    }

    /// 시작 전이. `status` → `session` 순서로 잠그고 **둘 다 Idle/비어 있을 때만**
    /// 진행한다. 세션 유무만 보면, 정지 처리가 끝나기 전(`Stopping`)에 새 녹음이
    /// 끼어들 수 있고 뒤늦게 끝난 정지 경로가 살아 있는 세션의 공유 상태를 Idle 로
    /// 지워 버린다(그 뒤 정지는 이전 경로를 반환하고 새 FFmpeg 는 고아가 된다).
    fn begin<'a>(
        &'a self,
    ) -> Result<
        (
            parking_lot::MutexGuard<'a, RecordingStateStatus>,
            parking_lot::MutexGuard<'a, Option<ActiveSession>>,
        ),
        String,
    > {
        let status = self.shared.status.lock();
        if *status != RecordingStateStatus::Idle {
            return Err(format!(
                "이전 녹음 정리가 끝나지 않았습니다(상태: {:?}). 잠시 후 다시 시도하세요.",
                *status
            ));
        }
        let session = self.shared.session.lock();
        if session.is_some() {
            return Err("이미 녹음/녹화가 진행 중입니다.".to_string());
        }
        Ok((status, session))
    }

    /// `region` 은 **가상 데스크톱 전역 물리 좌표**이고, `screen` 은 그 좌표가 나온
    /// 모니터 정보다(macOS 는 디스플레이 로컬 crop 으로 환산해야 하므로 원점이 필요).
    pub fn start_screen(
        &self,
        settings: &Settings,
        region: Option<RectRegion>,
        screen: Option<SelectionScreenInfo>,
    ) -> Result<String, String> {
        let (mut status, mut session_guard) = self.begin()?;

        let (tx, rx): (Sender<AudioEngineEvent>, Receiver<AudioEngineEvent>) = channel();
        let session = ScreenRecorderSession::start(settings, region, screen, tx)?;
        let path = session.output_path.clone();

        *session_guard = Some(ActiveSession::Screen(session));
        *status = RecordingStateStatus::Recording;
        *self.shared.mode.lock() = Some(RecordingMode::Screen);
        *self.shared.start_time.lock() = Some(Instant::now());
        *self.shared.paused_accum.lock() = Duration::ZERO;
        *self.shared.pause_start.lock() = None;
        self.shared.is_auto_paused.store(false, Ordering::SeqCst);
        *self.shared.output_path.lock() = Some(path.clone());
        drop(session_guard);
        drop(status);

        self.spawn_event_listener(rx);
        self.spawn_status_ticker();

        Ok(path.to_string_lossy().to_string())
    }

    pub fn start_audio(
        &self,
        settings: &Settings,
        file_name_prefix: Option<String>,
        exact_name: bool,
    ) -> Result<String, String> {
        let (mut status, mut session_guard) = self.begin()?;

        let (tx, rx): (Sender<AudioEngineEvent>, Receiver<AudioEngineEvent>) = channel();
        let session =
            AudioRecorderSession::start(settings, tx, file_name_prefix.as_deref(), exact_name)?;
        let path = session.output_path.clone();

        *session_guard = Some(ActiveSession::Audio(session));
        *status = RecordingStateStatus::Recording;
        *self.shared.mode.lock() = Some(RecordingMode::Audio);
        *self.shared.start_time.lock() = Some(Instant::now());
        *self.shared.paused_accum.lock() = Duration::ZERO;
        *self.shared.pause_start.lock() = None;
        self.shared.is_auto_paused.store(false, Ordering::SeqCst);
        *self.shared.output_path.lock() = Some(path.clone());
        drop(session_guard);
        drop(status);

        self.spawn_event_listener(rx);
        self.spawn_status_ticker();

        Ok(path.to_string_lossy().to_string())
    }

    pub fn pause(&self) -> Result<(), String> {
        {
            let mut status = self.shared.status.lock();
            if *status != RecordingStateStatus::Recording {
                return Err("녹음 중이 아닙니다.".to_string());
            }
            if let Some(session) = self.shared.session.lock().as_ref() {
                session.pause();
            }
            *status = RecordingStateStatus::Paused;
            *self.shared.pause_start.lock() = Some(Instant::now());
        }
        // 잠금을 놓은 뒤에 알린다 — 가드를 든 채로 부르면 상태 스냅샷이 같은 뮤텍스를
        // 다시 잠그려다 영구 교착에 빠진다(앱 전체가 멈췄던 원인).
        self.shared.emit_status();
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        {
            let mut status = self.shared.status.lock();
            if *status != RecordingStateStatus::Paused {
                return Err("일시정지 상태가 아닙니다.".to_string());
            }
            if let Some(session) = self.shared.session.lock().as_ref() {
                session.resume();
            }
            if let Some(start) = self.shared.pause_start.lock().take() {
                *self.shared.paused_accum.lock() += start.elapsed();
            }
            *status = RecordingStateStatus::Recording;
            self.shared.is_auto_paused.store(false, Ordering::SeqCst);
        }
        self.shared.emit_status();
        Ok(())
    }

    pub fn stop(&self) -> Result<String, String> {
        let Some(outcome) = self.shared.finish_active_session() else {
            // 멱등: 이미 정지됐으면 마지막 결과 경로를 그대로 돌려준다.
            return Ok(self.shared.last_stopped());
        };

        self.shared.emit_status();
        if let Some(app) = self.shared.app() {
            crate::commands::finish_recording_windows(&app);
        }

        outcome.map(|path| path.to_string_lossy().to_string())
    }

    /// 마지막으로 저장된 녹음 결과 경로(자동 종료 포함).
    pub fn last_recorded_path(&self) -> Option<String> {
        self.shared
            .last_stopped_path
            .lock()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
    }

    pub fn get_status(&self) -> RecordingStatus {
        self.shared.status_snapshot()
    }

    fn spawn_event_listener(&self, rx: Receiver<AudioEngineEvent>) {
        let shared = self.shared.clone();

        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                match event {
                    AudioEngineEvent::AutoPause => {
                        let changed = {
                            let mut st = shared.status.lock();
                            if *st == RecordingStateStatus::Recording {
                                *st = RecordingStateStatus::Paused;
                                shared.is_auto_paused.store(true, Ordering::SeqCst);
                                *shared.pause_start.lock() = Some(Instant::now());
                                true
                            } else {
                                false
                            }
                        };
                        if changed {
                            if let Some(app) = shared.app() {
                                let _ = app.emit("auto_pause_triggered", true);
                            }
                            shared.emit_status();
                        }
                    }
                    AudioEngineEvent::AutoResume => {
                        let changed = {
                            let mut st = shared.status.lock();
                            if *st == RecordingStateStatus::Paused
                                && shared.is_auto_paused.load(Ordering::SeqCst)
                            {
                                if let Some(start) = shared.pause_start.lock().take() {
                                    *shared.paused_accum.lock() += start.elapsed();
                                }
                                *st = RecordingStateStatus::Recording;
                                shared.is_auto_paused.store(false, Ordering::SeqCst);
                                true
                            } else {
                                false
                            }
                        };
                        if changed {
                            if let Some(app) = shared.app() {
                                let _ = app.emit("auto_resume_triggered", true);
                            }
                            shared.emit_status();
                        }
                    }
                    AudioEngineEvent::AutoStop => {
                        // 수동 정지가 먼저 전이를 가져갔으면(`None`) 이 이벤트로는
                        // 아무것도 알리지 않는다 — `auto_stop_triggered` 를 중복으로
                        // 쏘면 대본 자동 녹음이 정지 결과를 두 번 처리한다.
                        if let Some(outcome) = shared.finish_active_session() {
                            shared.emit_status();
                            if let Some(app) = shared.app() {
                                crate::commands::finish_recording_windows(&app);
                                // 자동 종료로 저장된 결과 파일 경로를 함께 전달해
                                // TTS 녹음 워크플로우가 결과를 대본에 연결할 수 있게 한다.
                                match &outcome {
                                    Ok(path) => {
                                        let _ = app.emit(
                                            "auto_stop_triggered",
                                            Some(path.to_string_lossy().to_string()),
                                        );
                                    }
                                    Err(err) => {
                                        let _ =
                                            app.emit("auto_stop_triggered", Option::<String>::None);
                                        let _ = app.emit(
                                            "recording_failed",
                                            format!("자동 종료 중 저장에 실패했습니다: {err}"),
                                        );
                                    }
                                }
                            }
                        }
                        break;
                    }
                    AudioEngineEvent::Fatal(message) => {
                        // 캡처/인코딩이 죽었다. 여기서 조용히 넘어가면 UI 는 계속
                        // "녹음 중"으로 보이고, 비어 있거나 잘린 파일이 정상 결과처럼
                        // 히스토리에 올라간다.
                        let outcome = shared.finish_active_session();
                        shared.emit_status();
                        if let Some(app) = shared.app() {
                            crate::commands::finish_recording_windows(&app);
                            let detail = match &outcome {
                                Some(Err(err)) => format!("{message} (정지 중 오류: {err})"),
                                _ => message.clone(),
                            };
                            let _ = app.emit("recording_failed", detail);
                        }
                        break;
                    }
                }
            }
        });
    }

    fn spawn_status_ticker(&self) {
        let shared = self.shared.clone();
        let generation = shared.ticker_generation.fetch_add(1, Ordering::SeqCst) + 1;

        thread::spawn(move || {
            while shared.ticker_generation.load(Ordering::SeqCst) == generation
                && *shared.status.lock() != RecordingStateStatus::Idle
            {
                let vu = shared
                    .session
                    .lock()
                    .as_ref()
                    .map(|session| session.vu_levels())
                    .unwrap_or((-60.0, -60.0));

                *shared.sys_vu_level.lock() = vu.0;
                *shared.mic_vu_level.lock() = vu.1;

                let status = *shared.status.lock();
                if let Some(app) = shared.app() {
                    let payload = AudioVUMeterPayload {
                        sys_level_db: vu.0,
                        mic_level_db: vu.1,
                        is_silent: vu.0 <= -45.0 && vu.1 <= -45.0,
                        duration_secs: shared.duration_secs(status),
                        size_bytes: shared.size_bytes(),
                    };
                    let _ = app.emit("audio_vu_meter", &payload);
                }

                thread::sleep(Duration::from_millis(50));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;

    /// 데드라인 안에 끝나야 하는 조작을 별도 스레드에서 돌린다. 교착이면 테스트가
    /// 통째로 멈추는 대신 여기서 실패한다.
    fn within<F>(label: &str, op: F) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(op());
        });
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                panic!("{label} 이(가) 3초 안에 끝나지 않았습니다 — 교착으로 봅니다")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("{label} 스레드가 패닉했습니다"),
        }
    }

    /// 회귀 테스트: `pause()`/`resume()` 는 `AppHandle` 이 붙어 있을 때
    /// `emit_status()` → `status_snapshot()` 으로 같은 뮤텍스를 다시 잠그려다
    /// 영구 교착에 빠졌다. `pause_record` 는 동기 커맨드(= Tauri 메인 스레드)라
    /// 앱 전체와 모든 웹뷰가 함께 얼었다. 초기 커밋부터 존재한 버그다.
    ///
    /// 이 테스트는 실제 장치/FFmpeg 없이 상태만 세팅해 그 경로를 그대로 태운다 —
    /// `AppHandle` 이 `None` 이면 `emit_status()` 가 조기 반환해 교착이 재현되지 않으므로
    /// 반드시 mock 앱 핸들을 붙여야 한다.
    #[test]
    fn pause_and_resume_do_not_deadlock_with_an_app_handle_attached() {
        let app = tauri::test::mock_app();
        let controller = Arc::new(RecorderController::<tauri::test::MockRuntime>::new());
        controller.set_app_handle(app.handle().clone());

        // 장치를 열지 않고 "녹음 중" 상태만 만든다.
        *controller.shared.status.lock() = RecordingStateStatus::Recording;
        *controller.shared.mode.lock() = Some(RecordingMode::Audio);
        *controller.shared.start_time.lock() = Some(Instant::now());

        let c = controller.clone();
        within("pause()", move || c.pause()).expect("일시정지는 성공해야 한다");
        assert_eq!(
            *controller.shared.status.lock(),
            RecordingStateStatus::Paused
        );

        // 교착이 아니었다면 상태 스냅샷도 정상적으로 만들어져야 한다.
        let snapshot = controller.get_status();
        assert_eq!(snapshot.status, RecordingStateStatus::Paused);

        let c = controller.clone();
        within("resume()", move || c.resume()).expect("재개는 성공해야 한다");
        assert_eq!(
            *controller.shared.status.lock(),
            RecordingStateStatus::Recording
        );
    }

    /// 일시정지 구간은 경과 시간에서 빠지고, 자동 일시정지 payload 의 duration 이
    /// 0.0 으로 하드코딩돼 타이머가 0 으로 튀던 문제도 같은 계산으로 사라진다.
    #[test]
    fn paused_time_is_excluded_from_the_reported_duration() {
        let controller = RecorderController::<tauri::test::MockRuntime>::new();
        *controller.shared.status.lock() = RecordingStateStatus::Paused;
        *controller.shared.start_time.lock() = Some(Instant::now() - Duration::from_secs(10));
        *controller.shared.pause_start.lock() = Some(Instant::now() - Duration::from_secs(4));

        let snapshot = controller.get_status();
        assert!(
            (snapshot.duration_secs - 6.0).abs() < 0.5,
            "일시정지 4초를 뺀 6초 근처여야 한다: {}",
            snapshot.duration_secs
        );
    }

    /// 정지 처리가 끝나기 전(`Stopping`)에 새 녹음이 끼어들면, 뒤늦게 끝난 정지 경로가
    /// 살아 있는 세션의 공유 상태를 Idle 로 지워 버린다(그 뒤 정지는 이전 결과 경로를
    /// 반환하고 새 FFmpeg 는 고아가 된다). 시작은 상태가 정확히 Idle 일 때만 허용한다.
    #[test]
    fn starting_is_rejected_while_the_previous_stop_is_still_running() {
        let controller = RecorderController::<tauri::test::MockRuntime>::new();
        *controller.shared.status.lock() = RecordingStateStatus::Stopping;

        let error = controller
            .start_audio(&crate::types::Settings::default(), None, false)
            .expect_err("정지 처리 중에는 시작이 거부되어야 한다");
        assert!(error.contains("정리"), "{error}");
    }

    /// 이미 정지된 상태에서의 `stop()` 은 멱등이어야 한다(핫키·미니 컨트롤러·자동 종료가
    /// 겹쳐 두 번 들어올 수 있다).
    #[test]
    fn stopping_twice_is_idempotent() {
        let controller = RecorderController::<tauri::test::MockRuntime>::new();
        *controller.shared.last_stopped_path.lock() = Some(PathBuf::from("/tmp/omnirec/a.m4a"));

        assert_eq!(controller.stop().unwrap(), "/tmp/omnirec/a.m4a");
        assert_eq!(controller.stop().unwrap(), "/tmp/omnirec/a.m4a");
    }
}

#[cfg(test)]
mod smoke_tests {
    use super::*;
    use crate::types::{RecordingStateStatus, Settings};

    /// 실제 장치 + 실제 FFmpeg 로 녹음 → 일시정지 → 재개 → 정지 전 과정을 돌리는
    /// 수동 스모크 테스트. 시스템 오디오 캡처 권한(macOS 화면 기록 권한)과 FFmpeg 가
    /// 필요해 일반 `cargo test` 에서는 제외한다.
    ///
    /// 확인 대상:
    /// 1. `pause()`/`resume()` 가 **실제 세션이 붙은 상태에서도** 교착 없이 끝난다.
    /// 2. 일시정지 구간이 결과 파일의 길이를 잘라내지 않는다(무음 패딩 + `-shortest`).
    /// 3. `stop()` 이 FFmpeg 종료 코드를 확인하고 0바이트가 아닌 파일을 돌려준다.
    ///
    /// 수동 실행:
    /// `cargo test --manifest-path src-tauri/Cargo.toml --lib recorder::smoke_tests -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn records_pauses_and_stops_with_real_devices() {
        let dir = std::env::temp_dir().join(format!("omnirec-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let settings = Settings {
            output_dir: dir.to_string_lossy().to_string(),
            audio_format: "m4a".to_string(),
            // 마이크만 쓴다 — 시스템 오디오(ScreenCaptureKit)는 앱 번들에만 부여된
            // macOS 화면 기록 권한이 필요해 `cargo test` 바이너리에서는 항상 거부된다.
            system_audio_enabled: false,
            mic_audio_enabled: true,
            auto_pause_enabled: false,
            auto_stop_enabled: false,
            ..Settings::default()
        };

        let controller = RecorderController::<tauri::test::MockRuntime>::new();
        let app = tauri::test::mock_app();
        controller.set_app_handle(app.handle().clone());

        let path = controller
            .start_audio(&settings, Some("smoke".to_string()), true)
            .expect("녹음이 시작되어야 한다");
        println!("녹음 시작: {path}");
        thread::sleep(Duration::from_millis(1200));

        let began = Instant::now();
        controller.pause().expect("일시정지");
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "pause() 가 즉시 끝나야 한다: {:?}",
            began.elapsed()
        );
        assert_eq!(controller.get_status().status, RecordingStateStatus::Paused);
        thread::sleep(Duration::from_millis(600));

        controller.resume().expect("재개");
        assert_eq!(
            controller.get_status().status,
            RecordingStateStatus::Recording
        );
        thread::sleep(Duration::from_millis(1200));

        let saved = controller.stop().expect("정지 및 저장");
        println!("저장 완료: {saved}");
        let size = std::fs::metadata(&saved).expect("결과 파일").len();
        assert!(size > 0, "0바이트 파일은 성공이 아니다");
        assert_eq!(controller.get_status().status, RecordingStateStatus::Idle);

        // 오디오 전용 녹음은 `TimelineMode::SkipPaused` 다 — 일시정지한 0.6초는 결과에
        // 담기지 않아야 한다(무음 자동 일시정지로 무음을 걷어내는 기능이 이 의미에 의존).
        // 화면 녹화만 `WallClock` 으로 무음을 메운다.
        let probed = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
                &saved,
            ])
            .output()
            .expect("ffprobe 실행");
        let duration: f64 = String::from_utf8_lossy(&probed.stdout)
            .trim()
            .parse()
            .expect("길이 파싱");
        println!("결과 크기: {size} bytes / 길이: {duration:.3}s");
        assert!(
            (1.9..=2.9).contains(&duration),
            "녹음 2.4초(1.2+1.2) 근처여야 하고 일시정지 0.6초는 빠져야 한다: {duration}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
