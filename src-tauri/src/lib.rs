pub mod audio;
pub mod commands;
pub mod converter;
pub mod history;
pub mod merger;
pub mod recorder;
pub mod settings;
pub mod subtitle;
pub mod types;

use std::sync::Arc;
use commands::AppState;
use converter::AudioConverterController;
use merger::MergerController;
use recorder::RecorderController;
use tauri::Manager;

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
    };

    let recorder_for_hotkey = recorder.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
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
            commands::run_macos_shortcut,
            commands::start_screen_record,
            commands::start_audio_record,
            commands::pause_record,
            commands::resume_record,
            commands::toggle_pause_record,
            commands::stop_record,
            commands::get_recording_status,
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
            commands::save_subtitle_file,
            commands::read_script_file,
            commands::extract_audio_pcm_16k,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(target_os = "windows")]
fn start_global_hotkeys(app_handle: tauri::AppHandle, recorder: Arc<RecorderController>) {
    use std::thread;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use tauri::Manager;

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
                            if let Some(mini_win) = app_handle.get_webview_window("mini-controller") {
                                let _ = mini_win.hide();
                            }
                            if let Some(main_win) = app_handle.get_webview_window("main") {
                                let _ = main_win.unminimize();
                                let _ = main_win.show();
                                let _ = main_win.set_focus();
                            }
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
