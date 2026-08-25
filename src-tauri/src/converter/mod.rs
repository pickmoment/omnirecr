use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::merger::MergerController;
use crate::settings::SettingsManager;
use crate::types::{AudioConvertProgressPayload, AudioConvertTaskPayload};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct AudioConverterController {
    active_child: Arc<Mutex<Option<Child>>>,
    is_cancelled: Arc<AtomicBool>,
}

impl AudioConverterController {
    pub fn new() -> Self {
        Self {
            active_child: Arc::new(Mutex::new(None)),
            is_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.is_cancelled.store(true, Ordering::SeqCst);
        let mut guard = self.active_child.lock();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
        }
    }

    pub fn convert(
        &self,
        app_handle: AppHandle,
        task: AudioConvertTaskPayload,
        custom_ffmpeg_path: Option<String>,
    ) -> Result<Vec<String>, String> {
        self.is_cancelled.store(false, Ordering::SeqCst);

        if task.input_files.is_empty() {
            return Err("변환할 입력 파일이 지정되지 않았습니다.".to_string());
        }

        let ffmpeg_path = SettingsManager::find_ffmpeg(custom_ffmpeg_path.as_deref())?;
        let probes = MergerController::probe_files(task.input_files.clone(), custom_ffmpeg_path)?;

        let total_files = task.input_files.len();
        let target_fmt = task.target_format.to_lowercase();
        let ext = if target_fmt == "mp3" { "mp3" } else { "m4a" };

        let mut converted_files = Vec::new();

        for (idx, input_path_str) in task.input_files.iter().enumerate() {
            if self.is_cancelled.load(Ordering::SeqCst) {
                break;
            }

            let input_path = Path::new(input_path_str);
            if !input_path.exists() {
                continue;
            }

            let file_stem = input_path.file_stem().unwrap_or_default().to_string_lossy();
            let file_name = input_path.file_name().unwrap_or_default().to_string_lossy().to_string();

            // Probe duration for accurate progress
            let duration = probes
                .iter()
                .find(|p| &p.path == input_path_str)
                .map(|p| p.duration_secs)
                .unwrap_or(0.0);
            let duration = if duration <= 0.0 { 1.0 } else { duration };

            // Determine output path
            let output_dir = if let Some(ref dir) = task.output_dir {
                if !dir.trim().is_empty() {
                    PathBuf::from(dir)
                } else {
                    input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
                }
            } else {
                input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
            };

            let _ = fs::create_dir_all(&output_dir);

            // Determine output filename
            let mut output_path = output_dir.join(format!("{}.{}", file_stem, ext));
            // If output_path is same as input_path, append _converted
            if output_path == input_path {
                output_path = output_dir.join(format!("{}_converted.{}", file_stem, ext));
            }

            let mut cmd = Command::new(&ffmpeg_path);
            #[cfg(target_os = "windows")]
            cmd.creation_flags(CREATE_NO_WINDOW);

            cmd.arg("-y")
                .arg("-i").arg(input_path_str);

            if target_fmt == "mp3" {
                cmd.arg("-c:a").arg("libmp3lame")
                    .arg("-b:a").arg(format!("{}k", task.bitrate));
            } else {
                // m4a / aac
                cmd.arg("-c:a").arg("aac")
                    .arg("-b:a").arg(format!("{}k", task.bitrate));
            }

            if let Some(sr) = task.sample_rate {
                if sr > 0 {
                    cmd.arg("-ar").arg(sr.to_string());
                }
            }

            if let Some(ch) = task.channels {
                if ch > 0 {
                    cmd.arg("-ac").arg(ch.to_string());
                }
            }

            cmd.arg("-progress").arg("pipe:1");
            cmd.arg(output_path.to_string_lossy().to_string());

            cmd.stdout(Stdio::piped()).stderr(Stdio::null());

            let mut child = cmd.spawn().map_err(|e| format!("FFmpeg 변환 프로세스 실행 실패: {}", e))?;
            let stdout = child.stdout.take().ok_or("FFmpeg 진행률 출력을 가져올 수 없습니다.")?;

            *self.active_child.lock() = Some(child);

            let reader = BufReader::new(stdout);
            let mut current_speed = "1.0x".to_string();

            for line_res in reader.lines() {
                if self.is_cancelled.load(Ordering::SeqCst) {
                    break;
                }

                if let Ok(line) = line_res {
                    let parts: Vec<&str> = line.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        let key = parts[0].trim();
                        let val = parts[1].trim();

                        if key == "speed" {
                            current_speed = val.to_string();
                        } else if key == "out_time_us" {
                            if let Ok(us) = val.parse::<f64>() {
                                let current_secs = us / 1_000_000.0;
                                let percent = ((current_secs / duration) * 100.0).clamp(0.0, 99.0) as f32;
                                let overall_percent = (((idx as f64 + (percent as f64 / 100.0)) / total_files as f64) * 100.0).clamp(0.0, 99.0) as f32;

                                let _ = app_handle.emit(
                                    "conversion_progress",
                                    &AudioConvertProgressPayload {
                                        file_index: idx,
                                        total_files,
                                        current_file_name: file_name.clone(),
                                        output_file_path: output_path.to_string_lossy().to_string(),
                                        percent,
                                        overall_percent,
                                        current_time_secs: current_secs,
                                        total_time_secs: duration,
                                        speed: current_speed.clone(),
                                        finished: false,
                                        error: None,
                                    },
                                );
                            }
                        } else if key == "progress" && val == "end" {
                            let overall_percent = (((idx + 1) as f64 / total_files as f64) * 100.0) as f32;
                            let _ = app_handle.emit(
                                "conversion_progress",
                                &AudioConvertProgressPayload {
                                    file_index: idx,
                                    total_files,
                                    current_file_name: file_name.clone(),
                                    output_file_path: output_path.to_string_lossy().to_string(),
                                    percent: 100.0,
                                    overall_percent,
                                    current_time_secs: duration,
                                    total_time_secs: duration,
                                    speed: current_speed.clone(),
                                    finished: idx + 1 == total_files,
                                    error: None,
                                },
                            );
                        }
                    }
                }
            }

            if let Some(mut child) = self.active_child.lock().take() {
                let _ = child.wait();
            }

            if self.is_cancelled.load(Ordering::SeqCst) {
                let _ = fs::remove_file(&output_path);
                return Err("변환 작업이 사용자에 의해 취소되었습니다.".to_string());
            }

            if output_path.exists() {
                converted_files.push(output_path.to_string_lossy().to_string());
            }
        }

        Ok(converted_files)
    }
}
