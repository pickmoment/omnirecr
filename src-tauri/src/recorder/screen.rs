use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::audio::engine::{AudioCaptureEngine, AudioEngineEvent};
use crate::audio::notifications::NotificationSoundManager;
use crate::settings::SettingsManager;
use crate::types::{RectRegion, Settings};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct ScreenRecorderSession {
    process: Child,
    audio_engine: AudioCaptureEngine,
    notification_manager: Option<NotificationSoundManager>,
    pub output_path: PathBuf,
}

impl ScreenRecorderSession {
    pub fn start(
        settings: &Settings,
        region: Option<RectRegion>,
        event_sender: Sender<AudioEngineEvent>,
    ) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        crate::audio::macos::ensure_screen_capture_permission()?;

        let ffmpeg_path = SettingsManager::find_ffmpeg(settings.custom_ffmpeg_path.as_deref())?;

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let filename = format!("Screen_Record_{}.mp4", timestamp);
        let output_dir = if settings.output_dir.trim().is_empty() {
            dirs::video_dir().unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(&settings.output_dir)
        };
        let _ = std::fs::create_dir_all(&output_dir);
        let output_path = output_dir.join(filename);

        let mut cmd = Command::new(&ffmpeg_path);

        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        cmd.arg("-y"); // Overwrite output

        #[cfg(target_os = "windows")]
        {
            cmd.arg("-f").arg("gdigrab")
                .arg("-framerate").arg(settings.video_fps.to_string())
                .arg("-draw_mouse").arg("1");

            if let Some(r) = region {
                // Even dimensions required for libx264 yuv420p
                let w = (r.width / 2) * 2;
                let h = (r.height / 2) * 2;
                cmd.arg("-offset_x").arg(r.x.to_string())
                    .arg("-offset_y").arg(r.y.to_string())
                    .arg("-video_size").arg(format!("{}x{}", w, h));
            }

            cmd.arg("-i").arg("desktop");
        }

        #[cfg(target_os = "macos")]
        {
            // macOS AVFoundation screen capture: "1:none" captures default display without hardware audio
            cmd.arg("-f").arg("avfoundation")
                .arg("-framerate").arg(settings.video_fps.to_string())
                .arg("-capture_cursor").arg("1")
                .arg("-i").arg("1:none");

            if let Some(r) = region {
                let w = (r.width / 2) * 2;
                let h = (r.height / 2) * 2;
                cmd.arg("-vf").arg(format!("crop={}:{}:{}:{}", w, h, r.x, r.y));
            }
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            cmd.arg("-f").arg("x11grab")
                .arg("-framerate").arg(settings.video_fps.to_string())
                .arg("-draw_mouse").arg("1");

            if let Some(r) = region {
                let w = (r.width / 2) * 2;
                let h = (r.height / 2) * 2;
                cmd.arg("-video_size").arg(format!("{}x{}", w, h))
                    .arg("-i").arg(format!(":0.0+{},{}", r.x, r.y));
            } else {
                cmd.arg("-i").arg(":0.0");
            }
        }

        // Audio input via raw f32le PCM pipe
        let sample_rate = settings.audio_sample_rate.to_string();
        cmd.arg("-f").arg("f32le")
            .arg("-ar").arg(&sample_rate)
            .arg("-ac").arg("2")
            .arg("-i").arg("pipe:0");

        // Encoding settings for ultra-low latency & high quality
        cmd.arg("-c:v").arg("libx264")
            .arg("-preset").arg("veryfast")
            .arg("-crf").arg("22")
            .arg("-pix_fmt").arg("yuv420p")
            .arg("-c:a").arg("aac")
            .arg("-b:a").arg("192k")
            .arg("-movflags").arg("+faststart")
            .arg("-shortest") // Finish encoding as soon as audio stdin stream closes!
            .arg(output_path.to_string_lossy().to_string());

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn FFmpeg: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to open FFmpeg stdin pipe")?;

        let audio_engine = AudioCaptureEngine::start(settings, stdin, event_sender)?;

        let mut notification_manager = None;
        if settings.mute_notifications {
            let notif = NotificationSoundManager::new(settings);
            if let Err(error) = notif.mute_system_notifications() {
                log::warn!("Failed to suppress notifications: {error}");
            }
            notification_manager = Some(notif);
        }

        Ok(Self {
            process: child,
            audio_engine,
            notification_manager,
            output_path,
        })
    }

    pub fn pause(&self) {
        self.audio_engine.pause();
    }

    pub fn resume(&self) {
        self.audio_engine.resume();
    }

    pub fn get_vu_levels(&self) -> (f32, f32) {
        self.audio_engine.get_vu_levels()
    }

    pub fn stop(mut self) -> Result<PathBuf, String> {
        // 1. Stop audio engine & close stdin pipe -> sends EOF to FFmpeg
        self.audio_engine.stop();

        if let Some(notif) = self.notification_manager.take() {
            if let Err(error) = notif.restore_system_notifications() {
                log::warn!("Failed to restore notifications: {error}");
            }
        }

        // 2. Wait up to 3.0 seconds for FFmpeg to finish writing MP4 container metadata
        let start = Instant::now();
        let mut finished = false;
        while start.elapsed() < Duration::from_millis(3000) {
            if let Ok(Some(_)) = self.process.try_wait() {
                finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        // 3. Fallback kill if process didn't terminate cleanly
        if !finished {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }

        Ok(self.output_path)
    }
}
