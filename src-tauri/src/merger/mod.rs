use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::settings::SettingsManager;
use crate::types::{MediaProbeInfo, MergeProgressPayload, MergeTaskPayload};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct MergerController {
    active_child: Arc<Mutex<Option<Child>>>,
    is_cancelled: Arc<AtomicBool>,
}

impl MergerController {
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

    pub fn probe_files(
        files: Vec<String>,
        custom_ffmpeg_path: Option<String>,
    ) -> Result<Vec<MediaProbeInfo>, String> {
        let ffprobe_path = SettingsManager::find_ffprobe(custom_ffmpeg_path.as_deref())?;
        let mut results = Vec::new();

        for file_str in files {
            let p = Path::new(&file_str);
            if !p.exists() {
                continue;
            }

            let mut cmd = Command::new(&ffprobe_path);
            #[cfg(target_os = "windows")]
            cmd.creation_flags(CREATE_NO_WINDOW);

            cmd.arg("-v").arg("quiet")
                .arg("-print_format").arg("json")
                .arg("-show_format")
                .arg("-show_streams")
                .arg(&file_str);

            let output = cmd.output().map_err(|e| format!("Failed to run ffprobe: {}", e))?;
            if !output.status.success() {
                continue;
            }

            let json: Value = serde_json::from_slice(&output.stdout)
                .map_err(|e| format!("Failed to parse ffprobe json: {}", e))?;

            let duration_secs = json["format"]["duration"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);

            let size_bytes = json["format"]["size"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or_else(|| p.metadata().map(|m| m.len()).unwrap_or(0));

            let format_name = json["format"]["format_name"]
                .as_str()
                .unwrap_or("")
                .to_string();

            let mut video_codec = None;
            let mut audio_codec = None;
            let mut width = None;
            let mut height = None;
            let mut fps = None;
            let mut sample_rate = None;
            let mut channels = None;
            let mut is_video = false;

            if let Some(streams) = json["streams"].as_array() {
                for s in streams {
                    let codec_type = s["codec_type"].as_str().unwrap_or("");
                    if codec_type == "video" && video_codec.is_none() {
                        is_video = true;
                        video_codec = s["codec_name"].as_str().map(|c| c.to_string());
                        width = s["width"].as_u64().map(|v| v as u32);
                        height = s["height"].as_u64().map(|v| v as u32);
                        if let Some(r_frame_rate) = s["r_frame_rate"].as_str() {
                            if let Some((num, den)) = r_frame_rate.split_once('/') {
                                if let (Ok(n), Ok(d)) = (num.parse::<f64>(), den.parse::<f64>()) {
                                    if d > 0.0 {
                                        fps = Some(n / d);
                                    }
                                }
                            }
                        }
                    } else if codec_type == "audio" && audio_codec.is_none() {
                        audio_codec = s["codec_name"].as_str().map(|c| c.to_string());
                        sample_rate = s["sample_rate"].as_str().and_then(|r| r.parse::<u32>().ok());
                        channels = s["channels"].as_u64().map(|c| c as u32);
                    }
                }
            }

            let file_name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let file_type = if is_video { "video".to_string() } else { "audio".to_string() };

            results.push(MediaProbeInfo {
                path: file_str,
                file_name,
                file_type,
                format_name,
                duration_secs,
                size_bytes,
                video_codec,
                audio_codec,
                width,
                height,
                fps,
                sample_rate,
                channels,
            });
        }

        Ok(results)
    }

    pub fn merge(
        &self,
        app_handle: AppHandle,
        task: MergeTaskPayload,
        custom_ffmpeg_path: Option<String>,
    ) -> Result<String, String> {
        self.is_cancelled.store(false, Ordering::SeqCst);

        if task.input_files.len() < 2 {
            return Err("At least 2 files are required to merge.".to_string());
        }

        let ffmpeg_path = SettingsManager::find_ffmpeg(custom_ffmpeg_path.as_deref())?;
        let probes = Self::probe_files(task.input_files.clone(), custom_ffmpeg_path)?;
        if probes.len() < 2 {
            return Err("Failed to analyze input media files.".to_string());
        }

        let total_duration: f64 = probes.iter().map(|p| p.duration_secs).sum();
        let total_duration = if total_duration <= 0.0 { 1.0 } else { total_duration };

        // Determine if lossless direct copy is possible
        let first = &probes[0];
        let is_video = first.file_type == "video";
        let target_fmt = task.output_format.to_lowercase();

        let is_direct_copy = probes.iter().all(|p| {
            if is_video {
                p.file_type == "video"
                    && p.video_codec == first.video_codec
                    && p.audio_codec == first.audio_codec
                    && p.width == first.width
                    && p.height == first.height
                    && target_fmt == "mp4"
            } else {
                p.file_type == "audio"
                    && p.audio_codec == first.audio_codec
                    && p.sample_rate == first.sample_rate
                    && ((target_fmt == "mp3" && first.audio_codec.as_deref() == Some("mp3"))
                        || (target_fmt == "m4a" && first.audio_codec.as_deref() == Some("aac")))
            }
        });

        let output_path = PathBuf::from(&task.output_path);
        if let Some(parent) = output_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut cmd = Command::new(&ffmpeg_path);
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        cmd.arg("-y");

        let mut temp_concat_file = None;

        if is_direct_copy {
            // Direct concat demuxer
            let temp_dir = std::env::temp_dir();
            let concat_file = temp_dir.join(format!("omnirec_concat_{}.txt", chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)));
            let mut list_content = String::new();
            for file in &task.input_files {
                let escaped = file.replace('\\', "/").replace('\'', "'\\''");
                list_content.push_str(&format!("file '{}'\n", escaped));
            }
            fs::write(&concat_file, list_content)
                .map_err(|e| format!("Failed to create temporary concat list: {}", e))?;

            temp_concat_file = Some(concat_file.clone());

            cmd.arg("-f").arg("concat")
                .arg("-safe").arg("0")
                .arg("-i").arg(concat_file.to_string_lossy().to_string())
                .arg("-c").arg("copy")
                .arg("-progress").arg("pipe:1")
                .arg(output_path.to_string_lossy().to_string());
        } else {
            // Smart re-encoding fallback
            for file in &task.input_files {
                cmd.arg("-i").arg(file);
            }

            let num_inputs = task.input_files.len();

            if is_video {
                // Video merge: standardizing video size to first file or 1920x1080
                let target_w = first.width.unwrap_or(1920);
                let target_h = first.height.unwrap_or(1080);
                let mut filter_complex = String::new();

                for (i, p) in probes.iter().enumerate() {
                    let has_audio = p.audio_codec.is_some();
                    filter_complex.push_str(&format!(
                        "[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30[v{i}]; ",
                        i = i, w = target_w, h = target_h
                    ));

                    if has_audio {
                        filter_complex.push_str(&format!("[{i}:a]aresample=48000[a{i}]; ", i = i));
                    } else {
                        // Generate silent audio for clips without an audio stream
                        filter_complex.push_str(&format!("anullsrc=channel_layout=stereo:sample_rate=48000[a{i}]; ", i = i));
                    }
                }

                for i in 0..num_inputs {
                    filter_complex.push_str(&format!("[v{i}][a{i}]", i = i));
                }
                filter_complex.push_str(&format!("concat=n={}:v=1:a=1[outv][outa]", num_inputs));

                cmd.arg("-filter_complex").arg(filter_complex)
                    .arg("-map").arg("[outv]")
                    .arg("-map").arg("[outa]")
                    .arg("-c:v").arg("libx264")
                    .arg("-preset").arg("veryfast")
                    .arg("-crf").arg("22")
                    .arg("-pix_fmt").arg("yuv420p")
                    .arg("-c:a").arg("aac")
                    .arg("-b:a").arg("192k")
                    .arg("-progress").arg("pipe:1")
                    .arg(output_path.to_string_lossy().to_string());
            } else {
                // Audio merge
                let mut filter_complex = String::new();
                for i in 0..num_inputs {
                    filter_complex.push_str(&format!("[{}:a]aresample=48000[a{}]; ", i, i));
                }
                for i in 0..num_inputs {
                    filter_complex.push_str(&format!("[a{}]", i));
                }
                filter_complex.push_str(&format!("concat=n={}:v=0:a=1[outa]", num_inputs));

                let ext = output_path.extension().and_then(|s| s.to_str()).unwrap_or("m4a");
                let codec = if ext == "mp3" { "libmp3lame" } else { "aac" };

                cmd.arg("-filter_complex").arg(filter_complex)
                    .arg("-map").arg("[outa]")
                    .arg("-c:a").arg(codec)
                    .arg("-b:a").arg("256k")
                    .arg("-progress").arg("pipe:1")
                    .arg(output_path.to_string_lossy().to_string());
            }
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| format!("Failed to run FFmpeg merge: {}", e))?;
        let stdout = child.stdout.take().ok_or("Failed to capture FFmpeg progress output")?;

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
                            let percent = ((current_secs / total_duration) * 100.0).clamp(0.0, 99.0) as f32;

                            let _ = app_handle.emit(
                                "merge_progress",
                                &MergeProgressPayload {
                                    percent,
                                    current_time_secs: current_secs,
                                    total_time_secs: total_duration,
                                    is_direct_copy,
                                    speed: current_speed.clone(),
                                    finished: false,
                                    error: None,
                                },
                            );
                        }
                    } else if key == "progress" && val == "end" {
                        let _ = app_handle.emit(
                            "merge_progress",
                            &MergeProgressPayload {
                                percent: 100.0,
                                current_time_secs: total_duration,
                                total_time_secs: total_duration,
                                is_direct_copy,
                                speed: current_speed.clone(),
                                finished: true,
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

        if let Some(temp) = temp_concat_file {
            let _ = fs::remove_file(temp);
        }

        if self.is_cancelled.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&output_path);
            return Err("Merge operation was cancelled.".to_string());
        }

        if output_path.exists() {
            Ok(output_path.to_string_lossy().to_string())
        } else {
            Err("Merge failed: output file was not generated.".to_string())
        }
    }
}
