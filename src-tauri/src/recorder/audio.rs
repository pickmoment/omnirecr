use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::audio::engine::{AudioCaptureEngine, AudioEngineEvent};
use crate::audio::notifications::NotificationSoundManager;
use crate::settings::SettingsManager;
use crate::types::Settings;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct AudioRecorderSession {
    process: Child,
    audio_engine: AudioCaptureEngine,
    notification_manager: Option<NotificationSoundManager>,
    pub output_path: PathBuf,
}

impl AudioRecorderSession {
    pub fn start(
        settings: &Settings,
        event_sender: Sender<AudioEngineEvent>,
    ) -> Result<Self, String> {
        let ffmpeg_path = SettingsManager::find_ffmpeg(settings.custom_ffmpeg_path.as_deref())?;

        let ext = match settings.audio_format.to_lowercase().as_str() {
            "mp3" => "mp3",
            "wav" => "wav",
            "m4a" => "m4a",
            _ => "m4a",
        };

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let filename = format!("Audio_Record_{}.{}", timestamp, ext);
        let output_dir = if settings.output_dir.trim().is_empty() {
            dirs::audio_dir().or_else(|| dirs::video_dir()).unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(&settings.output_dir)
        };
        let _ = std::fs::create_dir_all(&output_dir);
        let output_path = output_dir.join(filename);

        let mut cmd = Command::new(&ffmpeg_path);

        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let sample_rate = settings.audio_sample_rate.to_string();
        let bitrate_arg = format!("{}k", settings.audio_bitrate);

        cmd.arg("-y")
            .arg("-f").arg("f32le")
            .arg("-ar").arg(&sample_rate)
            .arg("-ac").arg("2")
            .arg("-i").arg("pipe:0");

        if ext == "mp3" {
            cmd.arg("-c:a").arg("libmp3lame")
                .arg("-b:a").arg(&bitrate_arg);
        } else if ext == "wav" {
            cmd.arg("-c:a").arg("pcm_s16le");
        } else {
            // M4A / AAC
            cmd.arg("-c:a").arg("aac")
                .arg("-b:a").arg(&bitrate_arg);
        }

        cmd.arg(output_path.to_string_lossy().to_string());

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn FFmpeg for audio: {}", e))?;

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
        self.audio_engine.stop();

        if let Some(notif) = self.notification_manager.take() {
            if let Err(error) = notif.restore_system_notifications() {
                log::warn!("Failed to restore notifications: {error}");
            }
        }

        let start = Instant::now();
        let mut finished = false;
        while start.elapsed() < Duration::from_millis(3000) {
            if let Ok(Some(_)) = self.process.try_wait() {
                finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        if !finished {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }

        Ok(self.output_path)
    }
}
