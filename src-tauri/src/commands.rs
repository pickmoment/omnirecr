use std::sync::Arc;
use tauri::{AppHandle, Manager, Position, PhysicalPosition, State};

use crate::converter::AudioConverterController;
use crate::history::HistoryManager;
use crate::merger::MergerController;
use crate::recorder::RecorderController;
use crate::settings::SettingsManager;
use crate::subtitle::SubtitleController;
use crate::types::{
    AudioConvertTaskPayload, HistoryItem, MediaProbeInfo, MergeTaskPayload, RecordingStateStatus,
    RecordingStatus, RectRegion, Settings, SubtitleGenerateResult, SubtitleGenerateTask,
};

pub struct AppState {
    pub recorder: Arc<RecorderController>,
    pub merger: Arc<MergerController>,
    pub converter: Arc<AudioConverterController>,
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

    state.recorder.start_screen(&settings, region)
}

#[tauri::command]
pub fn start_audio_record(
    state: State<AppState>,
    settings: Settings,
) -> Result<String, String> {
    state.recorder.start_audio(&settings)
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
pub fn show_selection_overlay(app: AppHandle) -> Result<(), String> {
    // 1. Hide main window so user sees clean screen
    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.hide();
    }

    std::thread::sleep(std::time::Duration::from_millis(150));

    // 2. Show fullscreen transparent overlay
    if let Some(window) = app.get_webview_window("selection-overlay") {
        let _ = window.set_fullscreen(true);
        let _ = window.set_always_on_top(true);
        let _ = window.show();
        let _ = window.set_focus();
        Ok(())
    } else {
        Err("Overlay window not found".to_string())
    }
}

#[tauri::command]
pub fn hide_selection_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("selection-overlay") {
        let _ = window.hide();
    }

    // Restore main window
    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.show();
        let _ = main_win.unminimize();
        let _ = main_win.set_focus();
    }

    Ok(())
}

#[tauri::command]
pub fn confirm_selection_region(app: AppHandle, region: RectRegion) -> Result<(), String> {
    use tauri::Emitter;
    if let Some(window) = app.get_webview_window("selection-overlay") {
        let _ = window.hide();
    }

    // Restore main window
    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.show();
        let _ = main_win.unminimize();
        let _ = main_win.set_focus();
    }

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

