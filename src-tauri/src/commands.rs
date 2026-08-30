use std::sync::Arc;
use tauri::{AppHandle, Manager, PhysicalPosition, Position, Size, State};

use crate::converter::AudioConverterController;
use crate::history::HistoryManager;
use crate::merger::MergerController;
use crate::recorder::RecorderController;
use crate::settings::SettingsManager;
use crate::subtitle::SubtitleController;
use crate::script::ScriptManager;
use crate::tts::TypecastController;
use crate::types::{
    AudioConvertTaskPayload, HistoryItem, MediaProbeInfo, MergeTaskPayload, RecordingStateStatus,
    RecordingStatus, RectRegion, ScriptDraft, ScriptItem, SelectionScreenInfo, Settings,
    SubtitleGenerateResult, SubtitleGenerateTask, TypecastBrowserState,
};

pub struct AppState {
    pub recorder: Arc<RecorderController>,
    pub merger: Arc<MergerController>,
    pub converter: Arc<AudioConverterController>,
    pub last_selection_screen: Arc<parking_lot::Mutex<Option<SelectionScreenInfo>>>,
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

#[tauri::command]
pub fn start_screen_record(
    app: AppHandle,
    state: State<AppState>,
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
            let _ = mini_win.set_position(Position::Physical(PhysicalPosition { x: mini_x, y: 20 }));
        }
        let _ = mini_win.set_always_on_top(true);
        let _ = mini_win.show();
    }

    // Small delay to allow window minimize animation to finish on Windows
    std::thread::sleep(std::time::Duration::from_millis(300));

    let result = state.recorder.start_screen(&settings, region);
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
pub fn start_audio_record(
    app: AppHandle,
    state: State<AppState>,
    settings: Settings,
    file_name_prefix: Option<String>,
    show_mini_controller: Option<bool>,
    exact_file_name: Option<bool>,
) -> Result<String, String> {
    let result = state
        .recorder
        .start_audio(&settings, file_name_prefix, exact_file_name.unwrap_or(false));

    // TTS 낭독 녹음처럼 다른 창(Typecast)에서 작업하는 동안에는
    // 항상 위에 뜨는 미니 컨트롤러로 정지/일시정지를 할 수 있게 한다.
    // (화면 녹화와 달리 메인 창을 최소화하지는 않는다.)
    if result.is_ok() && show_mini_controller.unwrap_or(false) {
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
    }

    result
}

/// 대본 & TTS 녹음이 실제로 저장할 경로를 미리 계산해, 같은 이름의 파일이 이미
/// 있는지 확인한다. 있으면 그 경로를 반환해 프론트가 덮어쓰기 확인을 띄우게 한다.
/// 녹음을 시작하지 않는 순수 조회이며, 실제 저장 경로 계산 로직은 `start_audio_record`
/// 와 `AudioRecorderSession::resolve_output_path` 를 공유해 어긋나지 않는다.
#[tauri::command]
pub fn check_script_recording_exists(
    settings: Settings,
    file_name_prefix: String,
) -> Result<Option<String>, String> {
    let path = crate::recorder::audio::AudioRecorderSession::resolve_output_path(
        &settings,
        Some(&file_name_prefix),
        true,
    );
    if path.exists() {
        Ok(Some(path.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
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
pub fn stop_record(app: AppHandle, state: State<AppState>) -> Result<String, String> {
    let res = state.recorder.stop();

    // 1. Hide floating mini controller
    if let Some(mini_win) = app.get_webview_window("mini-controller") {
        let _ = mini_win.hide();
    }

    // 2. Restore and focus main window when recording finishes
    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.unminimize();
        let _ = main_win.show();
        let _ = main_win.set_focus();
    }

    res
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

    tokio::task::spawn_blocking(move || {
        merger.merge(app, task, custom_path)
    })
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

    tokio::task::spawn_blocking(move || {
        converter.convert(app, task, custom_path)
    })
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

    let screen_info = SelectionScreenInfo {
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

    tokio::task::spawn_blocking(move || {
        SubtitleController::generate(task, custom_path)
    })
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

#[tauri::command]
pub async fn extract_audio_pcm_16k(path: String) -> Result<Vec<f32>, String> {
    let settings = SettingsManager::load();
    let custom_path = settings.custom_ffmpeg_path.clone();

    tokio::task::spawn_blocking(move || {
        let p = std::path::PathBuf::from(&path);
        SubtitleController::extract_pcm_16k(&p, custom_path.as_deref())
    })
    .await
    .map_err(|e| format!("PCM 추출 작업 실패: {}", e))?
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

#[tauri::command]
pub fn open_typecast_browser(app: AppHandle, url: Option<String>) -> Result<(), String> {
    TypecastController::open(&app, url)
}

#[tauri::command]
pub fn close_typecast_browser(app: AppHandle) -> Result<(), String> {
    TypecastController::close(&app)
}

#[tauri::command]
pub fn focus_typecast_browser(app: AppHandle) -> Result<(), String> {
    TypecastController::focus(&app)
}

#[tauri::command]
pub fn navigate_typecast_browser(app: AppHandle, url: String) -> Result<(), String> {
    TypecastController::navigate(&app, url)
}

#[tauri::command]
pub fn typecast_go_back(app: AppHandle) -> Result<(), String> {
    TypecastController::go_back(&app)
}

#[tauri::command]
pub fn typecast_reload(app: AppHandle) -> Result<(), String> {
    TypecastController::reload(&app)
}

#[tauri::command]
pub fn clear_typecast_session(app: AppHandle) -> Result<(), String> {
    TypecastController::clear_session(&app)
}

#[tauri::command]
pub fn get_typecast_browser_state(app: AppHandle) -> TypecastBrowserState {
    TypecastController::state(&app)
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
pub fn notify_typecast(app: AppHandle, message: String, tone: Option<String>) -> Result<(), String> {
    TypecastController::notify(&app, message, tone)
}

// ── Typecast 페이지 자동화 ──────────────────────────────────

/// 대본을 편집기에 주입한다. 결과는 `typecast_step` 이벤트로 보고된다.
#[tauri::command]
pub fn typecast_prepare_script(app: AppHandle, text: String) -> Result<(), String> {
    TypecastController::prepare_script(&app, text)
}

#[tauri::command]
pub fn typecast_play(app: AppHandle) -> Result<(), String> {
    TypecastController::play(&app)
}

#[tauri::command]
pub fn typecast_stop_playback(app: AppHandle) -> Result<(), String> {
    TypecastController::stop_playback(&app)
}

/// 편집기 / 재생 버튼을 어떻게 찾았는지 진단 보고를 요청한다.
#[tauri::command]
pub fn typecast_probe(app: AppHandle) -> Result<(), String> {
    TypecastController::probe(&app)
}
