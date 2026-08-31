use parking_lot::Mutex;
use std::collections::VecDeque;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::engine::{AudioCaptureEngine, AudioEngineEvent, TimelineMode};
use crate::audio::notifications::NotificationSoundManager;
use crate::settings::SettingsManager;
use crate::types::Settings;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// ─────────────────────────────────────────────────────────────
// FFmpeg 자식 프로세스 수명 관리 (recorder::screen 과 공유)
//
// 화면 녹화와 오디오 녹음이 완전히 같은 종료 규약을 쓰게 하려고 한 곳에 모았다.
// 한쪽만 고치면 다른 쪽에서 "실패를 성공으로 보고하는" 경로가 되살아난다.
// ─────────────────────────────────────────────────────────────

/// 실패 진단으로 남겨 둘 stderr 줄 수.
const STDERR_TAIL_LINES: usize = 20;

/// 한 줄이 이보다 길면 잘라 버린다(FFmpeg 이 아주 긴 필터 그래프를 뱉을 수 있다).
const STDERR_LINE_MAX: usize = 512;

/// 엔진 워커가 FFmpeg stdin 을 닫을 때까지 기다리는 상한.
pub(crate) const ENGINE_STOP_TIMEOUT: Duration = Duration::from_millis(2000);

/// stdin EOF 를 받은 FFmpeg 이 컨테이너를 마무리할 때까지 기다리는 기본 상한.
const FFMPEG_EXIT_TIMEOUT_BASE: Duration = Duration::from_millis(3000);

/// 크기에 비례해 늘려 주는 상한의 천장.
const FFMPEG_EXIT_TIMEOUT_MAX: Duration = Duration::from_secs(30);

/// 종료 대기 상한을 출력 파일 크기에 비례해 늘린다(100MB 당 1초, 최대 30초).
///
/// `+faststart` 는 종료 시점에 파일 전체를 다시 써서 moov 아톰을 앞으로 옮긴다.
/// 큰 파일은 이 재작성이 몇 초 이상 걸리는데, 그 도중에 kill 하면 재작성 중인
/// 컨테이너까지 깨져 녹화물이 통째로 재생 불가가 된다. 고정 3초는 500MB 넘는
/// 녹화를 사실상 매번 망가뜨린다.
///
/// 천장을 두는 이유: 동기 Tauri 커맨드는 메인 스레드에서 돌기 때문에 여기서
/// 무한정 기다리면 앱이 얼어붙는다. UI 정지 시간과 파일을 살리는 것 사이의 타협이다.
fn ffmpeg_exit_timeout(output_path: &Path) -> Duration {
    let bytes = std::fs::metadata(output_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let extra = Duration::from_secs(bytes / (100 * 1024 * 1024));
    (FFMPEG_EXIT_TIMEOUT_BASE + extra).min(FFMPEG_EXIT_TIMEOUT_MAX)
}

/// FFmpeg 을 kill 해 파이프를 깬 뒤 엔진 워커를 수거하는 상한.
const WORKER_REAP_TIMEOUT: Duration = Duration::from_millis(1000);

/// 초기화 실패 경로에서 FFmpeg 이 스스로 끝나기를 기다리는 짧은 상한.
const ABORT_EXIT_TIMEOUT: Duration = Duration::from_millis(500);

/// FFmpeg 자식의 stderr 마지막 몇 줄을 담아 두는 유계 버퍼.
///
/// 성공하면 그냥 버리고, 실패했을 때만 진단으로 쓴다. 예전에는 stderr 를
/// `Stdio::null()` 로 버려서 왜 실패했는지 알 방법이 아예 없었다.
pub(crate) type StderrTail = Arc<Mutex<VecDeque<String>>>;

/// stderr 를 계속 읽어 마지막 `STDERR_TAIL_LINES` 줄만 남기는 스레드를 띄운다.
///
/// 파이프를 비워 주는 역할도 겸한다 — 아무도 읽지 않으면 FFmpeg 이 stderr 파이프가
/// 가득 찬 순간 그대로 멈춰 인코딩이 죽는다(그래서 `piped()` 로 바꾸면서 반드시
/// 읽어 줘야 한다).
pub(crate) fn spawn_stderr_tail(stderr: ChildStderr) -> StderrTail {
    let tail: StderrTail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let sink = tail.clone();

    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buffer = [0u8; 4096];
        let mut line: Vec<u8> = Vec::with_capacity(256);

        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) => {
                    log::warn!("FFmpeg stderr 를 읽지 못했습니다: {error}");
                    break;
                }
            };

            for &byte in &buffer[..read] {
                // FFmpeg 진행 상황(`frame= ... fps= ...`)은 `\r` 로만 끝난다.
                // `\n` 만 구분자로 쓰면 진행 로그 전체가 하나의 거대한 줄로 쌓여
                // 장시간 녹화에서 메모리를 먹는다.
                if byte == b'\n' || byte == b'\r' {
                    flush_stderr_line(&sink, &mut line);
                } else if line.len() < STDERR_LINE_MAX {
                    line.push(byte);
                }
            }
        }

        flush_stderr_line(&sink, &mut line);
    });

    tail
}

fn flush_stderr_line(sink: &StderrTail, line: &mut Vec<u8>) {
    if line.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(line).trim_end().to_string();
    line.clear();
    if text.is_empty() {
        return;
    }

    let mut tail = sink.lock();
    if tail.len() >= STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(text);
}

/// 실패 진단에 붙일 stderr 꼬리 텍스트.
fn stderr_tail_text(tail: &StderrTail) -> String {
    let lines = tail.lock();
    let mut text = String::new();
    for line in lines.iter() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
    }
    text
}

/// 자식이 유계 시간 안에 끝나면 종료 상태를, 안 끝나면 `None`.
fn wait_child_for(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(error) => {
                log::warn!("FFmpeg 종료 상태를 확인하지 못했습니다: {error}");
                return None;
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// FFmpeg 을 스폰한 뒤 **초기화가 실패한** 경로에서 자식을 반드시 수거한다.
///
/// `?` 로 그냥 반환하면 `Child` 가 drop 되는데, Rust 의 `Child::drop` 은 `wait`
/// 하지 않으므로 좀비 프로세스가 남는다(녹음 시작 실패를 몇 번 반복하면
/// FFmpeg 프로세스가 계속 쌓인다).
///
/// 이 경로에서는 녹음이 시작조차 안 됐으므로 `-y` 로 이미 만들어진(그리고
/// 잘라낸) 출력 파일은 아무 가치가 없다 → 지운다. 남겨 두면 사용자 폴더에
/// 재생 불가 파일이 쌓인다.
pub(crate) fn abort_ffmpeg(child: &mut Child, output_path: &Path) {
    // 아직 우리가 stdin 을 들고 있으면 닫아 EOF 를 준다.
    drop(child.stdin.take());

    if wait_child_for(child, ABORT_EXIT_TIMEOUT).is_none() {
        if let Err(error) = child.kill() {
            log::warn!("초기화 실패 후 FFmpeg 을 종료하지 못했습니다: {error}");
        }
        if let Err(error) = child.wait() {
            log::warn!("초기화 실패 후 FFmpeg 을 수거하지 못했습니다: {error}");
        }
    }

    if output_path.exists() {
        if let Err(error) = std::fs::remove_file(output_path) {
            log::warn!("시작 실패로 남은 출력 파일을 지우지 못했습니다: {error}");
        }
    }
}

/// 정지 경로: FFmpeg 자식을 **유계 시간 안에** 수거하고 결과를 검증한다.
///
/// `stdin_closed` 는 엔진 워커가 정상적으로 루프를 빠져나와 stdin 을 닫았는지다.
/// false 면 워커가 커널 파이프 쓰기에서 막혀 있다는 뜻이고, 그건 FFmpeg 이 파이프를
/// 비우지 못한다는 뜻이므로 EOF 는 영원히 도착하지 않는다 → 기다리지 않고 곧바로
/// kill 해서 파이프를 깨야 워커도 빠져나올 수 있다.
///
/// "출력 파일이 존재한다"만으로 성공 판정하지 않는다. 종료 코드 · 파일 크기 ·
/// 엔진이 기록한 파이프 오류를 모두 본다.
pub(crate) fn finish_ffmpeg(
    child: &mut Child,
    engine: &AudioCaptureEngine,
    tail: &StderrTail,
    output_path: &Path,
    stdin_closed: bool,
) -> Result<(), String> {
    let exit_timeout = ffmpeg_exit_timeout(output_path);
    let status = if stdin_closed {
        wait_child_for(child, exit_timeout)
    } else {
        None
    };

    let forced = status.is_none();
    if forced {
        if let Err(error) = child.kill() {
            log::warn!("FFmpeg 프로세스를 강제 종료하지 못했습니다: {error}");
        }
    }

    if !stdin_closed {
        // 파이프가 깨졌으니 이제 막혀 있던 write 가 오류로 돌아오고 워커가 끝난다.
        engine.reap_worker(WORKER_REAP_TIMEOUT);
    }

    let status = match status {
        Some(status) => Some(status),
        None => match child.wait() {
            Ok(status) => Some(status),
            Err(error) => {
                log::warn!("FFmpeg 을 수거하지 못했습니다: {error}");
                None
            }
        },
    };

    let engine_fatal = engine.fatal_message();
    let engine_error = engine_fatal.as_deref();

    if !stdin_closed {
        return Err(finish_error(
            "오디오 엔진이 FFmpeg 파이프 쓰기에서 멈춰 강제 종료했습니다. FFmpeg 이 인코딩을 따라가지 못했을 수 있습니다.".to_string(),
            tail,
            output_path,
            engine_error,
        ));
    }

    if forced {
        return Err(finish_error(
            format!(
                "FFmpeg 이 {}초 안에 종료되지 않아 강제 종료했습니다. 컨테이너 메타데이터가 기록되지 않아 파일이 재생되지 않을 수 있습니다.",
                exit_timeout.as_secs_f32()
            ),
            tail,
            output_path,
            engine_error,
        ));
    }

    let Some(status) = status else {
        return Err(finish_error(
            "FFmpeg 종료 상태를 확인할 수 없습니다.".to_string(),
            tail,
            output_path,
            engine_error,
        ));
    };

    if !status.success() {
        return Err(finish_error(
            format!(
                "FFmpeg 이 오류로 종료했습니다(종료 코드 {}).",
                exit_code_text(&status)
            ),
            tail,
            output_path,
            engine_error,
        ));
    }

    match std::fs::metadata(output_path) {
        Ok(meta) if meta.len() > 0 => {}
        Ok(_) => {
            return Err(finish_error(
                "녹음 결과 파일이 비어 있습니다.".to_string(),
                tail,
                output_path,
                engine_error,
            ));
        }
        Err(error) => {
            return Err(finish_error(
                format!("녹음 결과 파일을 찾을 수 없습니다: {error}"),
                tail,
                output_path,
                engine_error,
            ));
        }
    }

    // FFmpeg 이 0 으로 끝났어도 엔진이 중간에 파이프 오류를 기록했다면 그 뒤의
    // 오디오는 전부 유실됐다. 조용히 성공으로 넘기면 잘린 파일이 정상 결과처럼
    // 히스토리에 올라간다.
    if let Some(engine_error) = engine_error {
        return Err(finish_error(
            "녹음 중 오디오 캡처가 중단됐습니다.".to_string(),
            tail,
            output_path,
            Some(engine_error),
        ));
    }

    Ok(())
}

fn exit_code_text(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "시그널로 종료".to_string(),
    }
}

/// 실패 메시지에 엔진 진단과 stderr 꼬리를 합치고, 쓸 수 없는 출력 파일을 정리한다.
fn finish_error(
    reason: String,
    tail: &StderrTail,
    output_path: &Path,
    engine_error: Option<&str>,
) -> String {
    let mut message = reason;

    if let Some(engine_error) = engine_error {
        message.push_str("\n오디오 엔진: ");
        message.push_str(engine_error);
    }

    let tail_text = stderr_tail_text(tail);
    if !tail_text.is_empty() {
        message.push_str("\nFFmpeg 마지막 출력:\n");
        message.push_str(&tail_text);
    }

    // 재현할 수 없는 녹음물은 **지우지 않는다** — 잘렸더라도 사용자의 유일한
    // 사본이고, 여기서 지우면 버그 하나 고치려다 더 큰 데이터 손실을 만든다.
    // 비어 있거나 아예 없는 파일만 치운다(0바이트 파일이 정상 결과처럼
    // 히스토리에 올라가면 더 헷갈린다).
    match std::fs::metadata(output_path) {
        Ok(meta) if meta.len() == 0 => {
            if let Err(error) = std::fs::remove_file(output_path) {
                log::warn!("비어 있는 출력 파일을 지우지 못했습니다: {error}");
            }
        }
        Ok(meta) => {
            message.push_str(&format!(
                "\n부분 파일({} 바이트)은 {} 에 남겨 두었습니다.",
                meta.len(),
                output_path.display()
            ));
        }
        Err(_) => {}
    }

    message
}

pub struct AudioRecorderSession {
    process: Child,
    audio_engine: AudioCaptureEngine,
    stderr_tail: StderrTail,
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
    /// 공유해, 사전 확인(`resolve_script_recording_targets` — 덮어쓰기 확인 · 제목 충돌
    /// 검사)이 실제 저장 경로와 어긋나지 않게 한다.
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
            dirs::audio_dir()
                .or_else(|| dirs::video_dir())
                .unwrap_or_else(|| PathBuf::from("."))
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
            // 폴더를 못 만들면 FFmpeg 이 "출력을 열 수 없다"로 죽는다 —
            // 원인을 여기서 바로 알려 주는 게 낫다.
            std::fs::create_dir_all(output_dir).map_err(|error| {
                format!(
                    "저장 폴더를 만들 수 없습니다({}): {error}",
                    output_dir.display()
                )
            })?;
        }

        let mut cmd = Command::new(&ffmpeg_path);

        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let sample_rate = settings.audio_sample_rate.to_string();
        let bitrate_arg = format!("{}k", settings.audio_bitrate);

        cmd.arg("-y")
            .arg("-f")
            .arg("f32le")
            .arg("-ar")
            .arg(&sample_rate)
            .arg("-ac")
            .arg("2")
            .arg("-i")
            .arg("pipe:0");

        if ext == "mp3" {
            cmd.arg("-c:a")
                .arg("libmp3lame")
                .arg("-b:a")
                .arg(&bitrate_arg);
        } else if ext == "wav" {
            cmd.arg("-c:a").arg("pcm_s16le");
        } else {
            // M4A / AAC
            cmd.arg("-c:a").arg("aac").arg("-b:a").arg(&bitrate_arg);
        }

        cmd.arg(output_path.to_string_lossy().to_string());

        // stderr 는 버리지 않고 파이프로 받는다 — 실패 진단의 유일한 단서다.
        // 반드시 계속 읽어 줘야 한다(`spawn_stderr_tail`).
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn FFmpeg for audio: {}", e))?;

        // 이 지점 이후의 실패는 전부 `abort_ffmpeg` 로 자식을 수거한 뒤 반환한다.
        let Some(stderr) = child.stderr.take() else {
            abort_ffmpeg(&mut child, &output_path);
            return Err("FFmpeg stderr 파이프를 열 수 없습니다.".to_string());
        };
        let stderr_tail = spawn_stderr_tail(stderr);

        let Some(stdin) = child.stdin.take() else {
            abort_ffmpeg(&mut child, &output_path);
            return Err("FFmpeg stdin 파이프를 열 수 없습니다.".to_string());
        };

        // 오디오 전용 녹음은 일시정지한 시간을 결과 파일에 담지 않는다.
        let audio_engine = match AudioCaptureEngine::start(
            settings,
            stdin,
            event_sender,
            TimelineMode::SkipPaused,
        ) {
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
    /// FFmpeg 상한은 `ffmpeg_exit_timeout` 이 출력 크기에 따라 정한다).
    ///
    /// 1) 정지 플래그 → 워커가 루프를 빠져나오며 FFmpeg stdin 을 닫아 EOF 를 준다.
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

#[cfg(test)]
mod tests {
    use super::{flush_stderr_line, stderr_tail_text, AudioRecorderSession, StderrTail};
    use crate::types::Settings;
    use parking_lot::Mutex;
    use std::collections::VecDeque;
    use std::sync::Arc;

    fn settings_with(output_dir: &str, format: &str) -> Settings {
        let mut settings = Settings::default();
        settings.output_dir = output_dir.to_string();
        settings.audio_format = format.to_string();
        settings
    }

    #[test]
    fn exact_name_keeps_the_script_title_verbatim() {
        let settings = settings_with("/tmp/omnirec", "m4a");
        let path = AudioRecorderSession::resolve_output_path(&settings, Some("1편 도입부"), true);
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/omnirec/1편 도입부.m4a")
        );
    }

    #[test]
    fn timestamped_name_is_used_when_not_exact() {
        let settings = settings_with("/tmp/omnirec", "wav");
        let path = AudioRecorderSession::resolve_output_path(&settings, Some("intro"), false);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("intro_"), "{name}");
        assert!(name.ends_with(".wav"), "{name}");
    }

    /// 서로 다른 제목이 같은 파일명으로 정규화될 수 있다 — 자동 일괄 녹음이
    /// 시작 전에 `resolve_script_recording_targets` 로 경로 충돌을 검사하는 이유다.
    /// (검사가 없으면 뒤 대본이 앞 대본 결과를 조용히 덮어쓴다.)
    #[test]
    fn different_titles_can_collapse_to_the_same_file_name() {
        let settings = settings_with("/tmp/omnirec", "m4a");
        let a = AudioRecorderSession::resolve_output_path(&settings, Some("1부: 시작"), true);
        let b = AudioRecorderSession::resolve_output_path(&settings, Some("1부/ 시작"), true);
        assert_eq!(a, b);

        let long_a = "가".repeat(40) + "첫째";
        let long_b = "가".repeat(40) + "둘째";
        let a = AudioRecorderSession::resolve_output_path(&settings, Some(&long_a), true);
        let b = AudioRecorderSession::resolve_output_path(&settings, Some(&long_b), true);
        assert_eq!(a, b, "40자 제한 때문에 뒤쪽만 다른 제목은 같은 파일이 된다");
    }

    #[test]
    fn blank_title_falls_back_to_the_default_prefix() {
        let settings = settings_with("/tmp/omnirec", "mp3");
        let path = AudioRecorderSession::resolve_output_path(&settings, Some("   "), true);
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/omnirec/Audio_Record.mp3")
        );
    }

    /// stderr 꼬리는 유계다. FFmpeg 은 진행 상황을 `\r` 로만 끝내며 쏟아내므로
    /// 상한이 없으면 장시간 녹화에서 메모리를 먹는다.
    #[test]
    fn stderr_tail_keeps_only_the_last_lines() {
        let tail: StderrTail = Arc::new(Mutex::new(VecDeque::new()));
        for index in 0..(super::STDERR_TAIL_LINES + 5) {
            let mut line = format!("line {index}").into_bytes();
            flush_stderr_line(&tail, &mut line);
        }

        assert_eq!(tail.lock().len(), super::STDERR_TAIL_LINES);
        let text = stderr_tail_text(&tail);
        assert!(text.starts_with("line 5"), "{text}");
        assert!(
            text.ends_with(&format!("line {}", super::STDERR_TAIL_LINES + 4)),
            "{text}"
        );
    }
}
