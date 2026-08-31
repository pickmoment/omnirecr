use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;

use crate::audio::engine::{AudioCaptureEngine, AudioEngineEvent, TimelineMode};
use crate::audio::notifications::NotificationSoundManager;
// FFmpeg 자식 수명 관리는 오디오 녹음과 완전히 같은 규약을 써야 한다.
// 한쪽만 고치면 다른 쪽에서 "실패를 성공으로 보고하는" 경로가 되살아난다.
use crate::recorder::audio::{
    abort_ffmpeg, finish_ffmpeg, spawn_stderr_tail, StderrTail, ENGINE_STOP_TIMEOUT,
};
use crate::settings::SettingsManager;
use crate::types::{RectRegion, SelectionScreenInfo, Settings};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct ScreenRecorderSession {
    process: Child,
    audio_engine: AudioCaptureEngine,
    stderr_tail: StderrTail,
    notification_manager: Option<NotificationSoundManager>,
    pub output_path: PathBuf,
}

impl ScreenRecorderSession {
    /// `region` 은 **가상 데스크톱 전역 물리 픽셀** 좌표다(프론트엔드 오버레이가
    /// 선택된 모니터의 원점을 더해서 보낸다). `screen` 은 그 모니터의 원점·크기·
    /// 배율이며, 백엔드별 좌표계 환산에 쓴다.
    ///
    /// - Windows `gdigrab -offset_x/-offset_y` 와 Linux `x11grab :0.0+x,y` 는
    ///   전역 좌표를 그대로 받는다.
    /// - macOS `avfoundation` 의 `crop` 필터는 **캡처 대상 디스플레이 로컬 좌표**라
    ///   모니터 원점을 빼야 한다.
    pub fn start(
        settings: &Settings,
        region: Option<RectRegion>,
        screen: Option<SelectionScreenInfo>,
        event_sender: Sender<AudioEngineEvent>,
    ) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        crate::audio::macos::ensure_screen_capture_permission()?;

        let ffmpeg_path = SettingsManager::find_ffmpeg(settings.custom_ffmpeg_path.as_deref())?;

        if let Some(info) = &screen {
            log::debug!(
                "선택 영역 기준 모니터: 원점=({}, {}) 크기={}x{} 배율={}",
                info.physical_x,
                info.physical_y,
                info.physical_width,
                info.physical_height,
                info.scale_factor
            );
        }

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let filename = format!("Screen_Record_{}.mp4", timestamp);
        let output_dir = if settings.output_dir.trim().is_empty() {
            dirs::video_dir().unwrap_or_else(|| PathBuf::from("."))
        } else {
            PathBuf::from(&settings.output_dir)
        };
        // 폴더를 못 만들면 FFmpeg 이 "출력을 열 수 없다"로 죽는다 —
        // 원인을 여기서 바로 알려 주는 게 낫다.
        std::fs::create_dir_all(&output_dir).map_err(|error| {
            format!(
                "저장 폴더를 만들 수 없습니다({}): {error}",
                output_dir.display()
            )
        })?;
        let output_path = output_dir.join(filename);

        let mut cmd = Command::new(&ffmpeg_path);

        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        cmd.arg("-y"); // Overwrite output

        #[cfg(target_os = "windows")]
        {
            cmd.arg("-f")
                .arg("gdigrab")
                .arg("-framerate")
                .arg(settings.video_fps.to_string())
                .arg("-draw_mouse")
                .arg("1");

            if let Some(r) = &region {
                // Even dimensions required for libx264 yuv420p
                let w = ((r.width / 2) * 2).max(2);
                let h = ((r.height / 2) * 2).max(2);
                // 음수 좌표를 0 으로 클램프하지 않는다 — 주 모니터보다 왼쪽/위에
                // 있는 보조 모니터의 전역 좌표는 실제로 음수이고, gdigrab 은 그
                // 값을 그대로 받는다. 클램프하면 엉뚱한 영역을 녹화한다.
                cmd.arg("-offset_x")
                    .arg(r.x.to_string())
                    .arg("-offset_y")
                    .arg(r.y.to_string())
                    .arg("-video_size")
                    .arg(format!("{}x{}", w, h));
            }
            cmd.arg("-i").arg("desktop");
        }

        #[cfg(target_os = "macos")]
        {
            // macOS AVFoundation screen capture: "1:none" captures default display without hardware audio
            cmd.arg("-f")
                .arg("avfoundation")
                .arg("-framerate")
                .arg(settings.video_fps.to_string())
                .arg("-capture_cursor")
                .arg("1")
                .arg("-i")
                .arg("1:none");
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            cmd.arg("-f")
                .arg("x11grab")
                .arg("-framerate")
                .arg(settings.video_fps.to_string())
                .arg("-draw_mouse")
                .arg("1");

            if let Some(r) = &region {
                let w = ((r.width / 2) * 2).max(2);
                let h = ((r.height / 2) * 2).max(2);
                // x11grab 의 `+x,y` 도 루트 윈도우 전역 좌표다. 클램프하지 않는다.
                cmd.arg("-video_size")
                    .arg(format!("{}x{}", w, h))
                    .arg("-i")
                    .arg(format!(":0.0+{},{}", r.x, r.y));
            } else {
                cmd.arg("-i").arg(":0.0");
            }
        }

        // macOS 만 출력 단계에서 crop 한다. `-vf` 는 **출력 옵션**이므로 입력
        // (`-i`) 사이에 끼워 넣으면 FFmpeg 이 "입력에 출력 옵션을 적용할 수 없다"로
        // 즉시 죽는다 — 예전 코드가 그래서 macOS 영역 녹화를 시도하면 파일이
        // 아예 안 생겼는데, 종료 코드를 안 봐서 성공으로 보고됐다.
        #[cfg(target_os = "macos")]
        let video_filter: Option<String> = match &region {
            Some(r) => {
                // avfoundation 은 `-i "1:none"` 으로 **주 디스플레이 하나**만 잡고,
                // crop 좌표는 그 디스플레이 로컬 좌표계다. 보조 모니터를 선택했으면
                // 조용히 엉뚱한 영역을 녹화하는 대신 실패를 알린다.
                let (origin_x, origin_y) = screen
                    .as_ref()
                    .map(|info| (info.physical_x, info.physical_y))
                    .unwrap_or((0, 0));
                if origin_x != 0 || origin_y != 0 {
                    return Err(
                        "macOS 에서는 주 디스플레이 영역만 녹화할 수 있습니다. 보조 모니터에서 선택한 영역은 지원하지 않으니 주 디스플레이에서 다시 선택해 주세요."
                            .to_string(),
                    );
                }
                let w = ((r.width / 2) * 2).max(2);
                let h = ((r.height / 2) * 2).max(2);
                Some(format!(
                    "crop={}:{}:{}:{}",
                    w,
                    h,
                    r.x - origin_x,
                    r.y - origin_y
                ))
            }
            None => None,
        };
        #[cfg(not(target_os = "macos"))]
        let video_filter: Option<String> = None;

        // Audio input via raw f32le PCM pipe
        let sample_rate = settings.audio_sample_rate.to_string();
        cmd.arg("-f")
            .arg("f32le")
            .arg("-ar")
            .arg(&sample_rate)
            .arg("-ac")
            .arg("2")
            .arg("-i")
            .arg("pipe:0");

        if let Some(filter) = &video_filter {
            cmd.arg("-vf").arg(filter);
        }

        // Encoding settings for ultra-low latency & high quality
        cmd.arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryfast")
            .arg("-crf")
            .arg("22")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("192k")
            .arg("-movflags")
            .arg("+faststart")
            // `-shortest` 는 유일한 크로스플랫폼 정지 레버다. 화면 입력
            // (gdigrab/avfoundation/x11grab)은 스스로 끝나지 않으므로 이걸 빼면
            // stdin EOF 로도 FFmpeg 이 종료되지 않고, 강제 kill → moov 미기록
            // (+faststart)으로 MP4 가 재생 불가가 된다.
            //
            // 대신 오디오 스트림이 벽시계보다 짧아지면 그만큼 영상 뒤가 잘리므로,
            // 엔진이 `TimelineMode::WallClock` 으로 일시정지 구간과 캡처 정지
            // 구간을 무음으로 메운다. 그래서 일시정지 누적 시간만큼 결과 영상이
            // 잘려 나가는 일이 없다.
            .arg("-shortest")
            .arg(output_path.to_string_lossy().to_string());

        // stderr 는 버리지 않고 파이프로 받는다 — 실패 진단의 유일한 단서다.
        // 반드시 계속 읽어 줘야 한다(`spawn_stderr_tail`).
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn FFmpeg: {}", e))?;

        // 이 지점 이후의 실패는 전부 `abort_ffmpeg` 로 자식을 수거한 뒤 반환한다.
        // 그러지 않으면 `Child` 가 wait 없이 drop 되며 좀비가 남는다.
        let Some(stderr) = child.stderr.take() else {
            abort_ffmpeg(&mut child, &output_path);
            return Err("FFmpeg stderr 파이프를 열 수 없습니다.".to_string());
        };
        let stderr_tail = spawn_stderr_tail(stderr);

        let Some(stdin) = child.stdin.take() else {
            abort_ffmpeg(&mut child, &output_path);
            return Err("FFmpeg stdin 파이프를 열 수 없습니다.".to_string());
        };

        let audio_engine =
            match AudioCaptureEngine::start(settings, stdin, event_sender, TimelineMode::WallClock)
            {
                Ok(engine) => engine,
                Err(error) => {
                    abort_ffmpeg(&mut child, &output_path);
                    return Err(error);
                }
            };

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
            stderr_tail,
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

    /// 반드시 유계 시간 안에 반환한다(최악 ≈ 엔진 2초 + FFmpeg 3~30초 + 워커 1초;
    /// FFmpeg 상한은 `+faststart` 재작성 시간을 고려해 출력 크기로 정해진다).
    ///
    /// 1) 정지 플래그 → 워커가 루프를 빠져나오며 FFmpeg stdin 을 닫아 EOF 를 준다.
    ///    `-shortest` 덕분에 그 EOF 가 화면 입력까지 끝낸다.
    /// 2) 워커가 유계 시간 안에 끝났는지 확인(무한 `join()` 이 UI 를 얼리던 경로).
    /// 3) `finish_ffmpeg` 이 자식을 수거하고 종료 코드 · 파일 크기를 검증한다.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        let stdin_closed = self.audio_engine.stop_within(ENGINE_STOP_TIMEOUT);

        // 알림 음소거는 성공/실패와 무관하게 되돌린다.
        if let Some(notif) = self.notification_manager.take() {
            if let Err(error) = notif.restore_system_notifications() {
                log::warn!("Failed to restore notifications: {error}");
            }
        }

        finish_ffmpeg(
            &mut self.process,
            &self.audio_engine,
            &self.stderr_tail,
            &self.output_path,
            stdin_closed,
        )?;

        Ok(self.output_path)
    }
}
