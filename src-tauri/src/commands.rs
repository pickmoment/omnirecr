use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager, PhysicalPosition, Position, Size, State};

use crate::converter::AudioConverterController;
use crate::history::HistoryManager;
use crate::merger::MergerController;
use crate::recorder::RecorderController;
use crate::script::ScriptManager;
use crate::settings::SettingsManager;
use crate::subtitle::SubtitleController;
use crate::tts::TypecastController;
use crate::types::{
    AudioConvertTaskPayload, HistoryItem, MediaProbeInfo, MergeTaskPayload, RecordingStateStatus,
    RecordingStatus, RectRegion, ScriptDraft, ScriptItem, ScriptRecordingTarget,
    SelectionScreenInfo, Settings, SubtitleGenerateResult, SubtitleGenerateTask,
    TypecastBrowserState,
};

pub struct AppState {
    pub recorder: Arc<RecorderController>,
    pub merger: Arc<MergerController>,
    pub converter: Arc<AudioConverterController>,
    pub last_selection_screen: Arc<parking_lot::Mutex<Option<SelectionScreenInfo>>>,
    /// 사용자가 메인 창을 직접 닫았는가(macOS 는 파괴 대신 숨긴다).
    /// 이 상태에서는 녹음 종료 같은 자동 복원이 창을 되살리지 않는다.
    pub main_window_closed_by_user: Arc<AtomicBool>,
}

#[tauri::command]
pub fn get_settings() -> Settings {
    SettingsManager::load()
}

#[tauri::command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    SettingsManager::save(&settings)
}

#[tauri::command]
pub fn check_ffmpeg_status(custom_ffmpeg_path: Option<String>) -> Result<String, String> {
    let path = SettingsManager::find_ffmpeg(custom_ffmpeg_path.as_deref())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn run_macos_shortcut(shortcut_name: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::audio::notifications::NotificationSoundManager::run_macos_shortcut(&shortcut_name)
    })
    .await
    .map_err(|e| format!("단축어 실행 작업 실패: {e}"))?
}

/// 녹화/녹음 시작·종료는 FFmpeg 스폰, cpal/ScreenCaptureKit 스트림 준비, 종료 대기(최대 3초)
/// 같은 블로킹 작업을 한다. 동기 커맨드는 Tauri 가 **메인 스레드**에서 실행하므로 그대로 두면
/// 그 시간 동안 UI 와 웹뷰 JS 가 통째로 멈춘다 — 대본 자동 일괄 녹음처럼 대본마다 시작/종료를
/// 반복하는 흐름에서는 매번 멈춰 중단 버튼조차 먹지 않는다. 블로킹 부분은 `spawn_blocking` 으로 옮긴다.
#[tauri::command]
pub async fn start_screen_record(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
    region: Option<RectRegion>,
) -> Result<String, String> {
    // 1. Minimize main window to avoid capturing the app itself
    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.minimize();
    }

    // 2. Position and show floating mini controller
    if let Some(mini_win) = app.get_webview_window("mini-controller") {
        if let Ok(Some(monitor)) = mini_win.primary_monitor() {
            let screen_w = monitor.size().width as i32;
            let mini_x = (screen_w - 360) / 2;
            let _ =
                mini_win.set_position(Position::Physical(PhysicalPosition { x: mini_x, y: 20 }));
        }
        let _ = mini_win.set_always_on_top(true);
        let _ = mini_win.show();
    }

    // 영역 좌표가 나온 모니터 정보를 함께 넘긴다. macOS 는 `crop` 이 디스플레이 로컬
    // 좌표라서 전역 좌표에서 모니터 원점을 빼야 하고, 보조 모니터 선택은 아예 거부해야
    // 한다(주 디스플레이만 캡처하므로 조용히 엉뚱한 영역이 녹화된다).
    let screen = region
        .as_ref()
        .and_then(|_| state.last_selection_screen.lock().clone());
    let recorder = state.recorder.clone();
    let result = tokio::task::spawn_blocking(move || {
        // Small delay to allow window minimize animation to finish on Windows
        std::thread::sleep(std::time::Duration::from_millis(300));
        recorder.start_screen(&settings, region, screen)
    })
    .await
    .map_err(|e| format!("화면 녹화 시작 작업 실패: {e}"))?;

    if result.is_err() {
        if let Some(mini_win) = app.get_webview_window("mini-controller") {
            let _ = mini_win.hide();
        }
        if let Some(main_win) = app.get_webview_window("main") {
            let _ = main_win.unminimize();
            let _ = main_win.show();
            let _ = main_win.set_focus();
        }
    }
    result
}

#[tauri::command]
pub async fn start_audio_record(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
    file_name_prefix: Option<String>,
    show_mini_controller: Option<bool>,
    exact_file_name: Option<bool>,
) -> Result<String, String> {
    let recorder = state.recorder.clone();
    let exact_name = exact_file_name.unwrap_or(false);
    let result = tokio::task::spawn_blocking(move || {
        recorder.start_audio(&settings, file_name_prefix, exact_name)
    })
    .await
    .map_err(|e| format!("녹음 시작 작업 실패: {e}"))?;

    // TTS 낭독 녹음처럼 다른 창(Typecast)에서 작업하는 동안에는
    // 항상 위에 뜨는 미니 컨트롤러로 정지/일시정지를 할 수 있게 한다.
    // (화면 녹화와 달리 메인 창을 최소화하지는 않는다.)
    if result.is_ok() && show_mini_controller.unwrap_or(false) {
        if let Some(mini_win) = app.get_webview_window("mini-controller") {
            if let Ok(Some(monitor)) = mini_win.primary_monitor() {
                let screen_w = monitor.size().width as i32;
                let mini_x = (screen_w - 360) / 2;
                let _ = mini_win
                    .set_position(Position::Physical(PhysicalPosition { x: mini_x, y: 20 }));
            }
            let _ = mini_win.set_always_on_top(true);
            let _ = mini_win.show();
        }
    }

    result
}

/// 대본 & TTS 녹음이 실제로 저장할 경로를 미리 계산한다(녹음은 시작하지 않는다).
///
/// 두 가지에 쓴다.
/// 1. **덮어쓰기 확인** — `exists` 가 true 면 프론트가 사용자에게 물어본다. 자동 일괄
///    녹음은 시작 전에 한 번에 다 확인해, 실행 도중 모달이 떠서 배치가 멈추지 않게 한다.
/// 2. **중복 제목 검사** — 제목이 곧 파일명이라 제목이 겹치면(또는 특수문자 치환·길이
///    제한 때문에 같은 이름으로 정규화되면) 뒤 대본이 앞 대본 결과를 덮어쓴다. 경로가
///    같은 항목이 있는지 프론트가 이 결과로 판정한다.
///
/// 파일명 규칙은 `AudioRecorderSession::resolve_output_path` 하나가 단일 출처다.
#[tauri::command]
pub fn resolve_script_recording_targets(
    settings: Settings,
    file_name_prefixes: Vec<String>,
) -> Vec<ScriptRecordingTarget> {
    file_name_prefixes
        .into_iter()
        .map(|prefix| {
            let path = crate::recorder::audio::AudioRecorderSession::resolve_output_path(
                &settings,
                Some(&prefix),
                true,
            );
            ScriptRecordingTarget {
                prefix,
                exists: path.exists(),
                path: path.to_string_lossy().to_string(),
            }
        })
        .collect()
}

#[tauri::command]
pub fn pause_record(state: State<AppState>) -> Result<(), String> {
    state.recorder.pause()
}

#[tauri::command]
pub fn resume_record(state: State<AppState>) -> Result<(), String> {
    state.recorder.resume()
}

#[tauri::command]
pub fn toggle_pause_record(state: State<AppState>) -> Result<(), String> {
    let st = state.recorder.get_status().status;
    if st == RecordingStateStatus::Recording {
        state.recorder.pause()
    } else if st == RecordingStateStatus::Paused {
        state.recorder.resume()
    } else {
        Ok(())
    }
}

#[tauri::command]
pub async fn stop_record(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let recorder = state.recorder.clone();
    let res = tokio::task::spawn_blocking(move || recorder.stop())
        .await
        .map_err(|e| format!("녹음 종료 작업 실패: {e}"))?;
    finish_recording_windows(&app);
    res
}

/// 녹화/녹음이 끝났을 때 미니 컨트롤러를 감추고 메인 창을 되돌린다.
/// 종료 경로가 셋(정지 커맨드 · 무음 자동 종료 · 글로벌 핫키)이라 한곳으로 모은다.
pub fn finish_recording_windows<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(mini_win) = app.get_webview_window("mini-controller") {
        let _ = mini_win.hide();
    }
    restore_main_window_if_hidden(app);
}

/// 메인 창이 **실제로 최소화·숨김 상태일 때만** 되돌리고 포커스를 가져온다.
///
/// 화면 녹화는 시작할 때 메인 창을 최소화하므로 끝나면 되돌려야 한다. 반면 대본 자동
/// 녹음은 메인 창을 건드리지 않는데, 그때도 무조건 `set_focus()` 를 부르면 대본 하나가
/// 끝날 때마다 Typecast Chrome 창에서 포커스를 빼앗아 화면이 깜빡이고, 다음 대본의
/// 재생 클릭 전에 창을 다시 앞으로 올려야 한다.
///
/// 사용자가 창을 직접 닫아 숨겨 둔 경우(macOS)는 되돌리지 않는다 — 닫아 둔 창이 대본
/// 하나가 끝날 때마다 되살아나면 안 된다. 그 창은 Dock 재열기로만 다시 뜬다.
pub fn restore_main_window_if_hidden<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<AppState>() {
        if state.main_window_closed_by_user.load(Ordering::SeqCst) {
            return;
        }
    }
    let Some(main_win) = app.get_webview_window("main") else {
        return;
    };
    let minimized = main_win.is_minimized().unwrap_or(false);
    let visible = main_win.is_visible().unwrap_or(true);
    if !minimized && visible {
        return;
    }
    if minimized {
        let _ = main_win.unminimize();
    }
    if !visible {
        let _ = main_win.show();
    }
    let _ = main_win.set_focus();
}

#[tauri::command]
pub fn get_recording_status(state: State<AppState>) -> RecordingStatus {
    state.recorder.get_status()
}

#[tauri::command]
pub fn get_last_recorded_path(state: State<AppState>) -> Option<String> {
    state.recorder.last_recorded_path()
}

#[tauri::command]
pub fn list_history_files() -> Vec<HistoryItem> {
    let settings = SettingsManager::load();
    HistoryManager::list_files(&settings.output_dir, settings.custom_ffmpeg_path)
}

#[tauri::command]
pub fn delete_history_file(path: String) -> Result<(), String> {
    HistoryManager::delete_file(&path)
}

#[tauri::command]
pub fn rename_history_file(old_path: String, new_name: String) -> Result<String, String> {
    HistoryManager::rename_file(&old_path, &new_name)
}

#[tauri::command]
pub fn read_audio_file(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| format!("Failed to read audio file: {}", e))
}

#[tauri::command]
pub fn open_in_explorer(path: String) -> Result<(), String> {
    HistoryManager::open_in_explorer(&path)
}

#[tauri::command]
pub fn open_with_default_player(path: String) -> Result<(), String> {
    HistoryManager::open_with_default_player(&path)
}

#[tauri::command]
pub fn probe_media_files(files: Vec<String>) -> Result<Vec<MediaProbeInfo>, String> {
    let settings = SettingsManager::load();
    MergerController::probe_files(files, settings.custom_ffmpeg_path)
}

#[tauri::command]
pub async fn merge_media_files(
    app: AppHandle,
    state: State<'_, AppState>,
    task: MergeTaskPayload,
) -> Result<String, String> {
    let settings = SettingsManager::load();
    let merger = state.merger.clone();
    let custom_path = settings.custom_ffmpeg_path.clone();

    tokio::task::spawn_blocking(move || merger.merge(app, task, custom_path))
        .await
        .map_err(|e| format!("Join task execution error: {}", e))?
}

#[tauri::command]
pub fn cancel_merge(state: State<AppState>) -> Result<(), String> {
    state.merger.cancel();
    Ok(())
}

#[tauri::command]
pub async fn convert_audio_files(
    app: AppHandle,
    state: State<'_, AppState>,
    task: AudioConvertTaskPayload,
) -> Result<Vec<String>, String> {
    let settings = SettingsManager::load();
    let converter = state.converter.clone();
    let custom_path = settings.custom_ffmpeg_path.clone();

    tokio::task::spawn_blocking(move || converter.convert(app, task, custom_path))
        .await
        .map_err(|e| format!("오디오 변환 작업 실행 오류: {}", e))?
}

#[tauri::command]
pub fn cancel_conversion(state: State<AppState>) -> Result<(), String> {
    state.converter.cancel();
    Ok(())
}

#[tauri::command]
pub fn show_selection_overlay(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    use tauri::Emitter;

    let window = app
        .get_webview_window("selection-overlay")
        .ok_or_else(|| "Overlay window not found".to_string())?;
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or(window.primary_monitor().map_err(|e| e.to_string())?)
        .ok_or_else(|| "영역을 선택할 디스플레이를 찾을 수 없습니다.".to_string())?;

    // 오버레이는 `monitor.position()`(전역 물리 좌표)에 배치된다. 그 원점을 프론트엔드에
    // 함께 알려야 뷰포트 로컬 좌표를 전역 좌표로 되돌릴 수 있다 — 원점 없이는 보조
    // 모니터에서 잡은 영역이 주 모니터 좌표로 해석돼 엉뚱한 화면이 녹화된다.
    let screen_info = SelectionScreenInfo {
        physical_x: monitor.position().x,
        physical_y: monitor.position().y,
        physical_width: monitor.size().width,
        physical_height: monitor.size().height,
        scale_factor: monitor.scale_factor(),
    };
    *state.last_selection_screen.lock() = Some(screen_info.clone());

    if let Some(main_win) = app.get_webview_window("main") {
        main_win.hide().map_err(|e| e.to_string())?;
    }

    let show_result = (|| -> Result<(), String> {
        window.set_fullscreen(false).map_err(|e| e.to_string())?;
        window
            .set_position(Position::Physical(*monitor.position()))
            .map_err(|e| e.to_string())?;
        window
            .set_size(Size::Physical(*monitor.size()))
            .map_err(|e| e.to_string())?;
        window.set_always_on_top(true).map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = show_result {
        let _ = window.hide();
        restore_main_window(&app);
        return Err(error);
    }

    let _ = app.emit("selection_screen_ready", &screen_info);
    Ok(())
}

#[tauri::command]
pub fn get_selection_screen_info(state: State<'_, AppState>) -> Option<SelectionScreenInfo> {
    state.last_selection_screen.lock().clone()
}

fn restore_main_window(app: &AppHandle) {
    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.show();
        let _ = main_win.unminimize();
        let _ = main_win.set_focus();
    }
}

#[tauri::command]
pub fn hide_selection_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("selection-overlay") {
        let _ = window.hide();
    }

    restore_main_window(&app);

    Ok(())
}

#[tauri::command]
pub fn confirm_selection_region(app: AppHandle, region: RectRegion) -> Result<(), String> {
    use tauri::Emitter;
    if let Some(window) = app.get_webview_window("selection-overlay") {
        let _ = window.hide();
    }

    restore_main_window(&app);

    let _ = app.emit("region_selected", &region);
    Ok(())
}

#[tauri::command]
pub async fn generate_subtitles(
    task: SubtitleGenerateTask,
) -> Result<SubtitleGenerateResult, String> {
    let settings = SettingsManager::load();
    let custom_path = settings.custom_ffmpeg_path.clone();

    tokio::task::spawn_blocking(move || SubtitleController::generate(task, custom_path))
        .await
        .map_err(|e| format!("자막 생성 작업 실행 오류: {}", e))?
}

#[tauri::command]
pub fn save_subtitle_file(path: String, content: String) -> Result<(), String> {
    SubtitleController::save_subtitle_file(&path, &content)
}

#[tauri::command]
pub fn read_script_file(path: String) -> Result<String, String> {
    SubtitleController::read_script_file(&path)
}

/// Whisper 전사용 PCM 을 **raw f32le 바이트**로 넘긴다.
///
/// `Vec<f32>` 로 반환하면 Tauri 가 JSON `number[]` 로 직렬화한다 — 1시간 오디오가
/// 5,760만 샘플이라 문자열 수백 MB 를 만들고, 프론트엔드가 그걸 다시 배열 → Float32Array
/// 로 복제한다. 실측으로 전사 시작 전에 앱이 멈추거나 죽었다. `tauri::ipc::Response` 는
/// 바이트를 그대로 실어 프론트엔드에서 `ArrayBuffer` 로 도착하므로,
/// `new Float32Array(buf)` 가 **사본 없이** 같은 메모리를 본다.
#[tauri::command]
pub async fn extract_audio_pcm_16k(path: String) -> Result<tauri::ipc::Response, String> {
    let settings = SettingsManager::load();
    let custom_path = settings.custom_ffmpeg_path.clone();

    let bytes = tokio::task::spawn_blocking(move || {
        let p = std::path::PathBuf::from(&path);
        SubtitleController::extract_pcm_16k_bytes(&p, custom_path.as_deref())
    })
    .await
    .map_err(|e| format!("PCM 추출 작업 실패: {}", e))??;

    Ok(tauri::ipc::Response::new(bytes))
}

// ─────────────────────────────────────────────────────────────
// 대본 관리 (Script Library)
// ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_scripts() -> Vec<ScriptItem> {
    ScriptManager::list()
}

#[tauri::command]
pub fn save_script(draft: ScriptDraft) -> Result<ScriptItem, String> {
    ScriptManager::upsert(draft)
}

#[tauri::command]
pub fn delete_script(id: String) -> Result<(), String> {
    ScriptManager::delete(&id)
}

#[tauri::command]
pub fn duplicate_script(id: String) -> Result<ScriptItem, String> {
    ScriptManager::duplicate(&id)
}

#[tauri::command]
pub fn import_script_file(path: String) -> Result<ScriptItem, String> {
    ScriptManager::import_from_file(&path)
}

#[tauri::command]
pub fn export_script_file(id: String, path: String) -> Result<(), String> {
    ScriptManager::export_to_file(&id, &path)
}

#[tauri::command]
pub fn attach_script_recording(id: String, recorded_path: String) -> Result<ScriptItem, String> {
    ScriptManager::attach_recording(&id, &recorded_path)
}

// ─────────────────────────────────────────────────────────────
// Typecast TTS 브라우저
// ─────────────────────────────────────────────────────────────

/// chromiumoxide 의 CDP 요청/응답 채널이 (드물지만) 응답을 못 받고 영원히 기다리는
/// 경우가 실측으로 확인됐다 — Chrome 창이 죽거나, 실행 컨텍스트가 사라지는 등.
/// 이 await 가 무한정 걸리면 `TtsBatchRunner` 의 for-loop 전체가 멈춰서, 그 스크립트의
/// `onStopRecord()`/저장 처리까지 실행되지 못한 채 배치 전체가 정지해 버린다(실제 증상:
/// 대본 3개 중 마지막에서 멈추고 녹음 종료 처리도 안 됨). 시간 제한을 걸어 반드시
/// `Result` 로 돌아가게 해, 프론트엔드의 기존 실패 처리(건너뛰기/중단 선택)가 정상 동작하게 한다.
async fn with_typecast_timeout<T>(
    op: &str,
    seconds: u64,
    fut: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    match tokio::time::timeout(std::time::Duration::from_secs(seconds), fut).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "Typecast 응답이 {}초 안에 오지 않았습니다({}). Chrome 창이 응답하지 않는 것 같습니다.",
            seconds, op
        )),
    }
}

#[tauri::command]
pub async fn open_typecast_browser(app: AppHandle, url: Option<String>) -> Result<(), String> {
    with_typecast_timeout("열기", 60, TypecastController::open(&app, url)).await
}

#[tauri::command]
pub async fn close_typecast_browser(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("닫기", 30, TypecastController::close(&app)).await
}

#[tauri::command]
pub async fn focus_typecast_browser(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("포커스", 20, TypecastController::focus(&app)).await
}

#[tauri::command]
pub async fn navigate_typecast_browser(app: AppHandle, url: String) -> Result<(), String> {
    with_typecast_timeout("이동", 60, TypecastController::navigate(&app, url)).await
}

#[tauri::command]
pub async fn typecast_go_back(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("뒤로 가기", 20, TypecastController::go_back(&app)).await
}

#[tauri::command]
pub async fn typecast_reload(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("새로고침", 30, TypecastController::reload(&app)).await
}

#[tauri::command]
pub async fn clear_typecast_session(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("세션 초기화", 60, TypecastController::clear_session(&app)).await
}

#[tauri::command]
pub async fn get_typecast_browser_state(app: AppHandle) -> TypecastBrowserState {
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        TypecastController::state(&app),
    )
    .await
    {
        Ok(state) => state,
        Err(_) => TypecastBrowserState {
            is_open: false,
            looks_signed_in: false,
            current_url: None,
            account_email: None,
            last_login_at: None,
        },
    }
}

/// 로그인 완료를 기록한다. 비밀번호는 저장하지 않고,
/// 세션 자체는 브라우저 창의 영구 쿠키 저장소가 유지한다.
#[tauri::command]
pub fn mark_typecast_login(email: Option<String>) -> Result<Settings, String> {
    let mut settings = SettingsManager::load();
    settings.typecast_session_saved = true;
    settings.typecast_last_login_at =
        Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
    settings.typecast_account_email = email
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .or(settings.typecast_account_email.clone());
    SettingsManager::save(&settings)?;
    Ok(settings)
}

#[tauri::command]
pub fn copy_text_to_clipboard(text: String) -> Result<(), String> {
    crate::clipboard::copy_text(&text)
}

/// Typecast 페이지 위에 안내 배너를 띄운다(카운트다운 / 녹음 시작 알림).
#[tauri::command]
pub async fn notify_typecast(
    app: AppHandle,
    message: String,
    tone: Option<String>,
) -> Result<(), String> {
    with_typecast_timeout(
        "알림 표시",
        20,
        TypecastController::notify(&app, message, tone),
    )
    .await
}

// ── Typecast 페이지 자동화 ──────────────────────────────────

/// 사용자가 직접 연 Typecast 프로젝트에 편집기와 재생 버튼이 준비됐는지 확인한다.
#[tauri::command]
pub async fn typecast_editor_ready(app: AppHandle) -> Result<bool, String> {
    with_typecast_timeout(
        "프로젝트 편집기 확인",
        20,
        TypecastController::editor_ready(&app),
    )
    .await
}

/// 대본을 편집기에 주입한다. 결과는 `typecast_step` 이벤트로 보고된다.
///
/// `copy_to_clipboard` 는 기본 true(수동 입력 폴백). 자동 일괄 녹음만 false 로 끈다.
#[tauri::command]
pub async fn typecast_prepare_script(
    app: AppHandle,
    text: String,
    copy_to_clipboard: Option<bool>,
) -> Result<(), String> {
    // 입력은 CDP 키/텍스트 이벤트를 단락 수만큼 순차로 보낸다 — 긴 대본은 20초를 넘길 수 있다.
    with_typecast_timeout(
        "대본 입력",
        60,
        TypecastController::prepare_script(&app, text, copy_to_clipboard.unwrap_or(true)),
    )
    .await
}

#[tauri::command]
pub async fn typecast_play(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("재생", 20, TypecastController::play(&app)).await
}

#[tauri::command]
pub async fn typecast_stop_playback(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("재생 정지", 20, TypecastController::stop_playback(&app)).await
}

/// 편집기 / 재생 버튼을 어떻게 찾았는지 진단 보고를 요청한다.
#[tauri::command]
pub async fn typecast_probe(app: AppHandle) -> Result<(), String> {
    with_typecast_timeout("진단", 20, TypecastController::probe(&app)).await
}

/// Chrome 실행 파일을 찾을 수 있는지 확인한다(설정 화면 "테스트" 버튼용).
#[tauri::command]
pub fn check_chrome_status(custom_chrome_path: Option<String>) -> Result<String, String> {
    let path = SettingsManager::find_chrome(custom_chrome_path.as_deref())?;
    Ok(path.to_string_lossy().to_string())
}
