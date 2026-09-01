pub mod audio;
pub mod clipboard;
pub mod commands;
pub mod converter;
pub mod history;
pub mod merger;
pub mod recorder;
pub mod script;
pub mod settings;
pub mod subtitle;
pub mod tts;
pub mod types;

use commands::AppState;
use converter::AudioConverterController;
use merger::MergerController;
use recorder::RecorderController;
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use tauri::RunEvent;
use tauri::{Manager, WindowEvent};
use tts::TypecastCdpState;

/// 기본(그리고 유일하게 사용자에게 보이는) 창의 라벨.
pub const MAIN_WINDOW_LABEL: &str = "main";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let recorder = Arc::new(RecorderController::new());
    let merger = Arc::new(MergerController::new());
    let converter = Arc::new(AudioConverterController::new());

    let app_state = AppState {
        recorder: recorder.clone(),
        merger,
        converter,
        last_selection_screen: Arc::new(parking_lot::Mutex::new(None)),
        main_window_closed_by_user: Arc::new(AtomicBool::new(false)),
    };

    let recorder_for_hotkey = recorder.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .manage(TypecastCdpState::new())
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW_LABEL {
                return;
            }
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            // 메인 창을 그냥 파괴하면 프로세스는 계속 살아 있다 — `selection-overlay` 와
            // `mini-controller` 가 숨겨진 채로 존재해 마지막 창 종료 조건이 성립하지 않는다.
            // 그 상태에서는 보이는 창이 하나도 없고, Dock/Finder 로 다시 열어도 macOS 는
            // 이미 실행 중인 인스턴스를 활성화할 뿐이라 아무 창도 뜨지 않았다(강제 종료 후에야
            // 다시 열렸다). macOS 는 창을 숨겨 두고 재열기(Reopen)로 되살리고,
            // Dock 재열기 개념이 없는 OS 는 그대로 종료한다.
            #[cfg(target_os = "macos")]
            {
                api.prevent_close();
                let _ = window.hide();
                window
                    .state::<AppState>()
                    .main_window_closed_by_user
                    .store(true, Ordering::SeqCst);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = api;
                window.app_handle().exit(0);
            }
        })
        .setup(move |app| {
            recorder.set_app_handle(app.handle().clone());
            if let Some(window) = app.get_webview_window("selection-overlay") {
                let _ = window.hide();
                let _ = window.set_fullscreen(false);
            }
            if let Some(window) = app.get_webview_window("mini-controller") {
                let _ = window.hide();
            }

            #[cfg(target_os = "windows")]
            start_global_hotkeys(app.handle().clone(), recorder_for_hotkey);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::check_ffmpeg_status,
            commands::check_chrome_status,
            commands::run_macos_shortcut,
            commands::start_screen_record,
            commands::start_audio_record,
            commands::resolve_script_recording_targets,
            commands::pause_record,
            commands::resume_record,
            commands::toggle_pause_record,
            commands::stop_record,
            commands::get_recording_status,
            commands::get_last_recorded_path,
            commands::list_history_files,
            commands::delete_history_file,
            commands::rename_history_file,
            commands::read_audio_file,
            commands::open_in_explorer,
            commands::open_with_default_player,
            commands::probe_media_files,
            commands::merge_media_files,
            commands::cancel_merge,
            commands::convert_audio_files,
            commands::cancel_conversion,
            commands::show_selection_overlay,
            commands::get_selection_screen_info,
            commands::hide_selection_overlay,
            commands::confirm_selection_region,
            commands::generate_subtitles,
            commands::save_subtitle_file,
            commands::read_script_file,
            commands::extract_audio_pcm_16k,
            commands::list_scripts,
            commands::save_script,
            commands::delete_script,
            commands::duplicate_script,
            commands::import_script_file,
            commands::export_script_file,
            commands::attach_script_recording,
            commands::open_typecast_browser,
            commands::close_typecast_browser,
            commands::focus_typecast_browser,
            commands::navigate_typecast_browser,
            commands::typecast_go_back,
            commands::typecast_reload,
            commands::clear_typecast_session,
            commands::get_typecast_browser_state,
            commands::mark_typecast_login,
            commands::copy_text_to_clipboard,
            commands::notify_typecast,
            commands::typecast_editor_ready,
            commands::typecast_prepare_script,
            commands::typecast_play,
            commands::typecast_stop_playback,
            commands::typecast_probe,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            // macOS 전용 이벤트. Dock 아이콘 클릭 · Finder 재실행 · `open -a` 가 모두 여기로 온다.
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { .. } = event {
                show_main_window(app_handle);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app_handle, event);
            }
        });
}

/// 메인 창을 다시 보여 준다. 창이 사라진 경우(다른 경로에서 destroy)에는
/// `tauri.conf.json` 의 정의를 그대로 다시 만들어 설정이 두 곳으로 갈라지지 않게 한다.
#[cfg(target_os = "macos")]
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.main_window_closed_by_user.store(false, Ordering::SeqCst);
    }

    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW_LABEL)
        .cloned();
    let Some(config) = config else {
        return;
    };
    match tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(err) => {
            log::error!("메인 창을 다시 만들지 못했습니다: {err}");
        }
    }
}

#[cfg(target_os = "windows")]
fn start_global_hotkeys(app_handle: tauri::AppHandle, recorder: Arc<RecorderController>) {
    use std::thread;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    thread::spawn(move || {
        unsafe {
            // Hotkey 1: F9 (Stop Recording)
            let _ = RegisterHotKey(None, 101, HOT_KEY_MODIFIERS(0), 0x78);
            // Hotkey 2: F10 (Pause / Resume Recording)
            let _ = RegisterHotKey(None, 102, HOT_KEY_MODIFIERS(0), 0x79);
            // Hotkey 3: Ctrl + Shift + R (Stop Recording)
            let _ = RegisterHotKey(None, 103, MOD_CONTROL | MOD_SHIFT, 0x52);
            // Hotkey 4: Ctrl + Shift + P (Pause / Resume Recording)
            let _ = RegisterHotKey(None, 104, MOD_CONTROL | MOD_SHIFT, 0x50);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                if msg.message == WM_HOTKEY {
                    let id = msg.wParam.0 as i32;
                    if id == 101 || id == 103 {
                        // Stop recording
                        let st = recorder.get_status().status;
                        if st != crate::types::RecordingStateStatus::Idle {
                            let _ = recorder.stop();
                            crate::commands::finish_recording_windows(&app_handle);
                        }
                    } else if id == 102 || id == 104 {
                        // Toggle pause
                        let st = recorder.get_status().status;
                        if st == crate::types::RecordingStateStatus::Recording {
                            let _ = recorder.pause();
                        } else if st == crate::types::RecordingStateStatus::Paused {
                            let _ = recorder.resume();
                        }
                    }
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}
