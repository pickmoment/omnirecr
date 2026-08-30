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
    /// 파일 이름에 쓸 수 없는 문자를 걷어내고 길이를 제한한다.
    fn sanitize_prefix(raw: &str) -> Option<String> {
        let cleaned: String = raw
            .trim()
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => '_',
                c => c,
            })
            .take(40)
            .collect();
        let cleaned = cleaned.trim().trim_matches('.').to_string();
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned)
        }
    }

    /// 실제로 녹음을 시작하지 않고 저장될 경로만 계산한다. `start()` 와 파일명 규칙을
    /// 공유해, 사전 존재 여부 확인(`check_script_recording_exists`)이 실제 저장 경로와
    /// 어긋나지 않게 한다.
    ///
    /// `exact_name` 이 true 면 타임스탬프를 붙이지 않고 접두어 그대로를 파일명으로 쓴다.
    /// 대본 & TTS 녹음은 대본 제목과 동일한 파일명을 유지해야 다음에 다시 녹음할 때도
    /// 같은 파일을 가리킬 수 있다(덮어쓰기 확인의 전제 조건이기도 하다).
    pub fn resolve_output_path(
        settings: &Settings,
        file_name_prefix: Option<&str>,
        exact_name: bool,
    ) -> PathBuf {
        let ext = match settings.audio_format.to_lowercase().as_str() {
            "mp3" => "mp3",
            "wav" => "wav",
            "m4a" => "m4a",
            _ => "m4a",
        };

        let prefix = file_name_prefix
            .and_then(Self::sanitize_prefix)
            .unwrap_or_else(|| "Audio_Record".to_string());

        let filename = if exact_name {
            format!("{}.{}", prefix, ext)
        } else {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
            format!("{}_{}.{}", prefix, timestamp, ext)
        };

        let output_dir = if settings.output_dir.trim().is_empty() {
            dirs::audio_dir().or_else(|| dirs::video_dir()).unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(&settings.output_dir)
        };

        output_dir.join(filename)
    }

    pub fn start(
        settings: &Settings,
        event_sender: Sender<AudioEngineEvent>,
        file_name_prefix: Option<&str>,
        exact_name: bool,
    ) -> Result<Self, String> {
        let ffmpeg_path = SettingsManager::find_ffmpeg(settings.custom_ffmpeg_path.as_deref())?;

        let ext = match settings.audio_format.to_lowercase().as_str() {
            "mp3" => "mp3",
            "wav" => "wav",
            "m4a" => "m4a",
            _ => "m4a",
        };

        let output_path = Self::resolve_output_path(settings, file_name_prefix, exact_name);
        if let Some(output_dir) = output_path.parent() {
            let _ = std::fs::create_dir_all(output_dir);
        }

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
