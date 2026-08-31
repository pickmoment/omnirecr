use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::settings::SettingsManager;
use crate::types::{MediaProbeInfo, MergeProgressPayload, MergeTaskPayload};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 진단용으로 들고 있는 FFmpeg stderr 마지막 줄 수. 전체를 모으지 않는다 —
/// 긴 인코딩에서 로그가 수십 MB 로 자라 메모리를 잡아먹는다. 실패 원인은 거의 항상
/// 마지막 몇 줄에 있다.
const STDERR_TAIL_LINES: usize = 20;

/// 재인코딩 시 통일하는 출력 프레임레이트.
const REENCODE_FPS: u32 = 30;

/// 재인코딩 시 통일하는 오디오 샘플레이트.
const REENCODE_SAMPLE_RATE: u32 = 48000;

/// 진행 중인 FFmpeg 작업의 자식 프로세스를 작업 ID 별로 보관하는 레지스트리.
///
/// 컨트롤러마다 `Option<Child>` 슬롯 하나와 취소 플래그 하나만 두면, 두 요청이 겹칠 때
/// 나중 요청이 앞 요청의 핸들을 덮어쓴다. 덮어쓰인 `Child` 는 kill/wait 없이 그냥 drop 되어
/// FFmpeg 이 고아 프로세스로 남아 같은 출력 파일을 계속 쓰고(결과물이 뒤섞인다), 취소는
/// 한쪽만 죽인다. 그래서 자식 핸들과 취소 플래그를 모두 작업 ID 별로 둔다 —
/// `cancel_all()` 은 살아 있는 모든 작업에 전파하고, 각 작업은 자기 자식만 수거한다.
pub struct ChildJobs {
    inner: Mutex<ChildJobsInner>,
}

#[derive(Default)]
struct ChildJobsInner {
    next_id: u64,
    /// 발급됐고 아직 끝나지 않은 작업 ID. 자식이 아직 spawn 되지 않은 작업도 여기 들어 있어야
    /// spawn 과 cancel 사이의 경쟁에서 취소가 유실되지 않는다.
    live: HashSet<u64>,
    cancelled: HashSet<u64>,
    children: HashMap<u64, Child>,
}

impl ChildJobs {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ChildJobsInner::default()),
        }
    }

    /// 새 작업을 시작하고 수명 가드를 돌려준다. 가드가 drop 될 때 남은 자식과 취소 플래그가
    /// 함께 정리되므로, `?` 로 중간에 빠져나가도 상태가 남지 않는다.
    pub fn begin(&self) -> JobGuard<'_> {
        let mut guard = self.inner.lock();
        guard.next_id += 1;
        let id = guard.next_id;
        guard.live.insert(id);
        drop(guard);
        JobGuard { jobs: self, id }
    }

    /// 살아 있는 모든 작업을 취소한다. 자식 핸들은 맵에서 빼지 않는다 — 소유한 작업이
    /// `wait` 로 수거해야 좀비 프로세스가 남지 않는다.
    pub fn cancel_all(&self) {
        let mut guard = self.inner.lock();
        let live: Vec<u64> = guard.live.iter().copied().collect();
        for id in live {
            guard.cancelled.insert(id);
        }
        for (id, child) in guard.children.iter_mut() {
            if let Err(err) = child.kill() {
                eprintln!("FFmpeg 작업 {} 종료 요청 실패: {}", id, err);
            }
        }
    }

    fn attach(&self, id: u64, child: Child) -> bool {
        let mut guard = self.inner.lock();
        if let Some(mut previous) = guard.children.insert(id, child) {
            // 같은 작업 ID 에 자식이 두 번 붙는 건 논리 오류다. 그냥 drop 하면 그 프로세스가
            // 고아로 남으므로 여기서 죽이고 수거한다.
            eprintln!(
                "FFmpeg 작업 {}: 이전 자식 프로세스가 남아 있어 강제 수거합니다.",
                id
            );
            if let Err(err) = previous.kill() {
                eprintln!("FFmpeg 작업 {} 이전 자식 종료 실패: {}", id, err);
            }
            if let Err(err) = previous.wait() {
                eprintln!("FFmpeg 작업 {} 이전 자식 수거 실패: {}", id, err);
            }
        }
        if !guard.cancelled.contains(&id) {
            return true;
        }
        // begin 과 attach 사이에 취소가 들어온 경우. 이 자리에서 죽여 두지 않으면
        // 사용자가 취소한 뒤에도 FFmpeg 이 끝까지 인코딩을 마친다.
        if let Some(child) = guard.children.get_mut(&id) {
            if let Err(err) = child.kill() {
                eprintln!("FFmpeg 작업 {} 종료 요청 실패: {}", id, err);
            }
        }
        false
    }

    fn is_cancelled(&self, id: u64) -> bool {
        self.inner.lock().cancelled.contains(&id)
    }

    fn take(&self, id: u64) -> Option<Child> {
        self.inner.lock().children.remove(&id)
    }

    fn end(&self, id: u64) {
        let mut guard = self.inner.lock();
        guard.live.remove(&id);
        guard.cancelled.remove(&id);
        let orphan = guard.children.remove(&id);
        drop(guard);

        // 정상 경로에서는 작업이 이미 take + wait 로 수거했다. 여기 남아 있다면 중간에
        // 에러로 빠져나온 경로이므로 지금 죽이고 수거한다.
        if let Some(mut child) = orphan {
            if let Err(err) = child.kill() {
                eprintln!("FFmpeg 작업 {} 종료 요청 실패: {}", id, err);
            }
            if let Err(err) = child.wait() {
                eprintln!("FFmpeg 작업 {} 수거 실패: {}", id, err);
            }
        }
    }
}

/// 작업 수명 가드. 자기 작업 ID 의 자식만 다룬다 — 다른 작업의 프로세스를 건드리지 않는다.
pub struct JobGuard<'a> {
    jobs: &'a ChildJobs,
    id: u64,
}

impl JobGuard<'_> {
    pub fn is_cancelled(&self) -> bool {
        self.jobs.is_cancelled(self.id)
    }

    fn attach(&self, child: Child) -> bool {
        self.jobs.attach(self.id, child)
    }

    fn take(&self) -> Option<Child> {
        self.jobs.take(self.id)
    }
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        self.jobs.end(self.id);
    }
}

/// FFmpeg 한 번의 실행 결과. 종료 상태를 버리지 않고 호출자에게 그대로 넘긴다.
pub struct FfmpegRun {
    pub status: ExitStatus,
    pub stderr_tail: String,
}

impl FfmpegRun {
    fn status_text(&self) -> String {
        match self.status.code() {
            Some(code) => format!("종료 코드 {}", code),
            None => format!("비정상 종료 ({})", self.status),
        }
    }

    /// 실패 진단 메시지. stderr 마지막 줄들을 함께 담아 원인(코덱 없음, 디스크 꽉 참,
    /// 잘못된 입력)이 UI 에서 바로 보이게 한다.
    pub fn failure_message(&self, context: &str) -> String {
        let tail = self.stderr_tail.trim();
        if tail.is_empty() {
            format!("{} (FFmpeg {})", context, self.status_text())
        } else {
            format!("{} (FFmpeg {})\n{}", context, self.status_text(), tail)
        }
    }
}

/// FFmpeg 을 실행하고 `-progress pipe:1` 의 `key=value` 줄을 `on_progress` 로 넘긴다.
///
/// 두 가지가 이 함수의 존재 이유다.
/// 1. stderr 를 전용 스레드가 계속 비운다. 파이프를 잡아 두고 읽지 않으면 64KB 버퍼가 차는
///    순간 FFmpeg 이 쓰기에서 블록되어 인코딩이 영구 정지한다.
/// 2. `ExitStatus` 를 버리지 않는다. 출력 파일 존재만으로 성공을 판정하면 잘려서 재생조차
///    안 되는 파일이 "성공" 으로 히스토리에 올라간다.
pub fn run_ffmpeg_with_progress<F>(
    mut cmd: Command,
    job: &JobGuard<'_>,
    mut on_progress: F,
) -> Result<FfmpegRun, String>
where
    F: FnMut(&str, &str),
{
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("FFmpeg 프로세스 실행 실패: {}", e))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("FFmpeg 진행률 출력을 가져올 수 없습니다.")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("FFmpeg 오류 출력을 가져올 수 없습니다.")?;

    let tail: Arc<Mutex<VecDeque<String>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let tail_writer = Arc::clone(&tail);
    let stderr_pump = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let mut guard = tail_writer.lock();
            if guard.len() == STDERR_TAIL_LINES {
                guard.pop_front();
            }
            guard.push_back(line);
        }
    });

    // attach 가 false 를 돌려주면 이미 취소된 작업이라 자식이 방금 kill 됐다. 그래도
    // 아래에서 wait 까지 진행해야 프로세스를 수거한다.
    job.attach(child);

    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        if job.is_cancelled() {
            break;
        }
        let Ok(line) = line else { break };
        if let Some((key, value)) = line.split_once('=') {
            on_progress(key.trim(), value.trim());
        }
    }

    let status = match job.take() {
        Some(mut child) => child
            .wait()
            .map_err(|e| format!("FFmpeg 종료 상태를 확인할 수 없습니다: {}", e))?,
        None => {
            return Err("FFmpeg 프로세스 핸들이 사라져 종료 상태를 확인할 수 없습니다.".to_string())
        }
    };

    if let Err(err) = stderr_pump.join() {
        eprintln!("FFmpeg stderr 수집 스레드가 비정상 종료했습니다: {:?}", err);
    }

    let stderr_tail = {
        let guard = tail.lock();
        guard.iter().cloned().collect::<Vec<_>>().join("\n")
    };

    Ok(FfmpegRun {
        status,
        stderr_tail,
    })
}

/// 실패·취소로 쓰다 만 출력 파일을 지운다. 남겨 두면 재생 불가 파일이 결과물처럼 남아
/// 사용자가 그걸 정상 파일로 착각한다.
pub fn remove_partial_output(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => eprintln!("불완전한 출력 파일 삭제 실패 ({}): {}", path.display(), err),
    }
}

/// FFmpeg 공통 전역 옵션. `-nostats` 가 특히 중요하다 — 상태 줄은 `\n` 없이 `\r` 로만
/// 갱신되므로 그대로 두면 stderr 링버퍼가 줄바꿈 없는 거대한 한 줄을 무한히 키운다.
pub fn ffmpeg_base_command(ffmpeg_path: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg_path);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("warning")
        .arg("-y");
    cmd
}

/// fps 는 `30000/1001` 같은 유리수를 f64 로 환산한 값이라 소수 오차를 허용해 비교한다.
fn same_fps(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 0.01,
        (None, None) => true,
        _ => false,
    }
}

/// 병합 진행 이벤트에 매번 들어가는 고정 정보.
struct MergeProgress {
    total_time_secs: f64,
    is_direct_copy: bool,
}

impl MergeProgress {
    fn emit(
        &self,
        app_handle: &AppHandle,
        percent: f32,
        current_time_secs: f64,
        speed: &str,
        finished: bool,
        error: Option<String>,
    ) {
        let payload = MergeProgressPayload {
            percent,
            current_time_secs,
            total_time_secs: self.total_time_secs,
            is_direct_copy: self.is_direct_copy,
            speed: speed.to_string(),
            finished,
            error,
        };
        if let Err(err) = app_handle.emit("merge_progress", &payload) {
            eprintln!("merge_progress 이벤트 발행 실패: {}", err);
        }
    }
}

pub struct MergerController {
    jobs: ChildJobs,
}

impl MergerController {
    pub fn new() -> Self {
        Self {
            jobs: ChildJobs::new(),
        }
    }

    pub fn cancel(&self) {
        self.jobs.cancel_all();
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

            cmd.arg("-v")
                .arg("quiet")
                .arg("-print_format")
                .arg("json")
                .arg("-show_format")
                .arg("-show_streams")
                .arg(&file_str);

            let output = cmd
                .output()
                .map_err(|e| format!("Failed to run ffprobe: {}", e))?;
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
                        sample_rate = s["sample_rate"]
                            .as_str()
                            .and_then(|r| r.parse::<u32>().ok());
                        channels = s["channels"].as_u64().map(|c| c as u32);
                    }
                }
            }

            let file_name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let file_type = if is_video {
                "video".to_string()
            } else {
                "audio".to_string()
            };

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
        if task.input_files.len() < 2 {
            return Err("병합하려면 파일이 2개 이상 필요합니다.".to_string());
        }

        let ffmpeg_path = SettingsManager::find_ffmpeg(custom_ffmpeg_path.as_deref())?;
        let probes = Self::probe_files(task.input_files.clone(), custom_ffmpeg_path)?;

        // probe_files 는 열 수 없는 입력을 조용히 건너뛴다. 여기서 입력 순서대로 1:1 정렬을
        // 강제하지 않으면 두 가지 사고가 난다 — (1) 사용자가 고른 파일이 결과에서 조용히
        // 빠지고, (2) filter_complex 의 스트림 인덱스가 실제 `-i` 순서와 어긋나 엉뚱한
        // 입력이 합쳐진다.
        let mut ordered: Vec<&MediaProbeInfo> = Vec::with_capacity(task.input_files.len());
        let mut unreadable: Vec<&str> = Vec::new();
        for path in &task.input_files {
            match probes.iter().find(|p| &p.path == path) {
                Some(info) => ordered.push(info),
                None => unreadable.push(path.as_str()),
            }
        }
        if !unreadable.is_empty() {
            return Err(format!(
                "다음 입력 파일을 분석할 수 없어 병합을 중단했습니다 (파일이 없거나 미디어가 아닙니다): {}",
                unreadable.join(", ")
            ));
        }

        let total_duration: f64 = ordered.iter().map(|p| p.duration_secs).sum();
        let total_duration = if total_duration <= 0.0 {
            1.0
        } else {
            total_duration
        };

        let first = ordered[0];
        let is_video = first.file_type == "video";
        let target_fmt = task.output_format.to_lowercase();

        // 직접 복사(stream copy) 는 코덱·해상도가 같은 것만으로는 안전하지 않다. fps 나
        // 샘플레이트·채널 수가 다른 파일을 그대로 이어붙이면 컨테이너 타임스탬프가 어긋나
        // 재생 속도가 틀린(혹은 후반부가 안 들리는) 출력이 나온다. probe 로 같음을
        // 증명할 수 없으면(None) 재인코딩으로 내려간다.
        let is_direct_copy = if is_video {
            target_fmt == "mp4"
                && first.video_codec.is_some()
                && first.width.is_some()
                && first.height.is_some()
                && first.fps.is_some()
                && ordered.iter().all(|p| {
                    p.file_type == "video"
                        && p.video_codec == first.video_codec
                        && p.audio_codec == first.audio_codec
                        && p.width == first.width
                        && p.height == first.height
                        && same_fps(p.fps, first.fps)
                        && p.sample_rate == first.sample_rate
                        && p.channels == first.channels
                })
        } else {
            let container_matches_codec = (target_fmt == "mp3"
                && first.audio_codec.as_deref() == Some("mp3"))
                || (target_fmt == "m4a" && first.audio_codec.as_deref() == Some("aac"));

            container_matches_codec
                && first.sample_rate.is_some()
                && first.channels.is_some()
                && ordered.iter().all(|p| {
                    p.file_type == "audio"
                        && p.audio_codec == first.audio_codec
                        && p.sample_rate == first.sample_rate
                        && p.channels == first.channels
                })
        };

        let output_path = PathBuf::from(&task.output_path);
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("출력 폴더를 만들 수 없습니다 ({}): {}", parent.display(), e)
                })?;
            }
        }

        let mut cmd = ffmpeg_base_command(&ffmpeg_path);
        let mut temp_concat_file = None;

        if is_direct_copy {
            // Direct concat demuxer
            let temp_dir = std::env::temp_dir();
            let concat_file = temp_dir.join(format!(
                "omnirec_concat_{}.txt",
                chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
            ));
            let mut list_content = String::new();
            for file in &task.input_files {
                let escaped = file.replace('\\', "/").replace('\'', "'\\''");
                list_content.push_str(&format!("file '{}'\n", escaped));
            }
            fs::write(&concat_file, list_content)
                .map_err(|e| format!("임시 concat 목록 파일을 만들 수 없습니다: {}", e))?;

            temp_concat_file = Some(concat_file.clone());

            cmd.arg("-f")
                .arg("concat")
                .arg("-safe")
                .arg("0")
                .arg("-i")
                .arg(concat_file.to_string_lossy().to_string())
                .arg("-c")
                .arg("copy")
                .arg("-progress")
                .arg("pipe:1")
                .arg(output_path.to_string_lossy().to_string());
        } else {
            // Smart re-encoding fallback
            for file in &task.input_files {
                cmd.arg("-i").arg(file);
            }

            let num_inputs = ordered.len();

            if is_video {
                // libx264 + yuv420p 는 가로·세로가 2의 배수여야 한다(AGENTS.md 불변식 6).
                // 첫 입력이 홀수 크기(1079 등)일 때 그대로 쓰면 인코더가
                // "width/height not divisible by 2" 로 죽는다. 아래로 내림해 짝수로 맞춘다.
                let target_w = (first.width.unwrap_or(1920) / 2) * 2;
                let target_h = (first.height.unwrap_or(1080) / 2) * 2;
                if target_w == 0 || target_h == 0 {
                    return Err(format!(
                        "재인코딩 목표 해상도를 정할 수 없습니다: 첫 입력 해상도 {}x{} → 짝수 보정 {}x{}. \
                         libx264 + yuv420p 는 0 이 아닌 짝수 해상도가 필요합니다.",
                        first.width.unwrap_or(0),
                        first.height.unwrap_or(0),
                        target_w,
                        target_h
                    ));
                }

                let mut filter_complex = String::new();

                for (i, p) in ordered.iter().enumerate() {
                    filter_complex.push_str(&format!(
                        "[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps}[v{i}]; ",
                        i = i, w = target_w, h = target_h, fps = REENCODE_FPS
                    ));

                    if p.audio_codec.is_some() {
                        filter_complex.push_str(&format!(
                            "[{i}:a]aresample={sr},asetpts=N/SR/TB[a{i}]; ",
                            i = i,
                            sr = REENCODE_SAMPLE_RATE
                        ));
                    } else {
                        // anullsrc 는 duration 을 주지 않으면 끝나지 않는 무한 스트림이다.
                        // 그대로 concat 에 넣으면 이 세그먼트에서 EOF 가 오지 않아 다음
                        // 입력으로 영원히 넘어가지 못하고, 병합이 진행률 도중에 멈춘 채
                        // 사용자가 취소할 때까지 CPU 를 태운다(실측: timeout 없이 무한 대기).
                        // 그래서 probe 된 길이로 유한하게 자른다.
                        if p.duration_secs <= 0.0 {
                            return Err(format!(
                                "'{}' 의 길이를 알 수 없어 채울 무음 트랙의 길이를 정할 수 없습니다. \
                                 무한 길이 무음은 병합을 영구 정지시키므로 재인코딩을 중단합니다.",
                                p.file_name
                            ));
                        }
                        filter_complex.push_str(&format!(
                            "anullsrc=channel_layout=stereo:sample_rate={sr}:d={dur:.6},asetpts=N/SR/TB[a{i}]; ",
                            i = i, sr = REENCODE_SAMPLE_RATE, dur = p.duration_secs
                        ));
                    }
                }

                for i in 0..num_inputs {
                    filter_complex.push_str(&format!("[v{i}][a{i}]", i = i));
                }
                filter_complex.push_str(&format!("concat=n={}:v=1:a=1[outv][outa]", num_inputs));

                cmd.arg("-filter_complex")
                    .arg(filter_complex)
                    .arg("-map")
                    .arg("[outv]")
                    .arg("-map")
                    .arg("[outa]")
                    .arg("-c:v")
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
                    .arg("-progress")
                    .arg("pipe:1")
                    .arg(output_path.to_string_lossy().to_string());
            } else {
                // Audio merge
                let mut filter_complex = String::new();
                for (i, p) in ordered.iter().enumerate() {
                    // `[i:a]` 를 참조하기 전에 오디오 스트림 존재를 확인한다 —
                    // 없으면 FFmpeg 이 "Invalid file index" 로 죽고 원인이 드러나지 않는다.
                    if p.audio_codec.is_none() {
                        return Err(format!(
                            "'{}' 에는 오디오 스트림이 없어 오디오 병합에 넣을 수 없습니다.",
                            p.file_name
                        ));
                    }
                    filter_complex.push_str(&format!(
                        "[{i}:a]aresample={sr},asetpts=N/SR/TB[a{i}]; ",
                        i = i,
                        sr = REENCODE_SAMPLE_RATE
                    ));
                }
                for i in 0..num_inputs {
                    filter_complex.push_str(&format!("[a{}]", i));
                }
                filter_complex.push_str(&format!("concat=n={}:v=0:a=1[outa]", num_inputs));

                let ext = output_path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("m4a");
                let codec = if ext == "mp3" { "libmp3lame" } else { "aac" };

                cmd.arg("-filter_complex")
                    .arg(filter_complex)
                    .arg("-map")
                    .arg("[outa]")
                    .arg("-c:a")
                    .arg(codec)
                    .arg("-b:a")
                    .arg("256k")
                    .arg("-progress")
                    .arg("pipe:1")
                    .arg(output_path.to_string_lossy().to_string());
            }
        }

        let progress = MergeProgress {
            total_time_secs: total_duration,
            is_direct_copy,
        };

        let job = self.jobs.begin();
        let mut current_speed = "1.0x".to_string();

        let run_result = run_ffmpeg_with_progress(cmd, &job, |key, value| {
            if key == "speed" {
                current_speed = value.to_string();
            } else if key == "out_time_us" {
                if let Ok(us) = value.parse::<f64>() {
                    let current_secs = us / 1_000_000.0;
                    let percent = ((current_secs / total_duration) * 100.0).clamp(0.0, 99.0) as f32;
                    progress.emit(
                        &app_handle,
                        percent,
                        current_secs,
                        &current_speed,
                        false,
                        None,
                    );
                }
            } else if key == "progress" && value == "end" {
                // progress=end 를 finished 로 바로 올리지 않는다. FFmpeg 은 여기까지 찍고도
                // 0 이 아닌 코드로 죽을 수 있어서(디스크 꽉 참, 뒤늦은 muxer 오류) 완료
                // 통보는 종료 코드를 본 뒤에 한다.
                progress.emit(
                    &app_handle,
                    100.0,
                    total_duration,
                    &current_speed,
                    false,
                    None,
                );
            }
        });

        // 임시 concat 목록은 성공·실패·취소 모두에서 지운다.
        if let Some(temp) = temp_concat_file {
            match fs::remove_file(&temp) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => eprintln!(
                    "임시 concat 목록 파일 삭제 실패 ({}): {}",
                    temp.display(),
                    err
                ),
            }
        }

        if job.is_cancelled() {
            remove_partial_output(&output_path);
            let message = "병합 작업이 사용자에 의해 취소되었습니다.".to_string();
            // finished 를 올려 진행 UI 가 켜진 채 남지 않게 한다.
            progress.emit(
                &app_handle,
                0.0,
                0.0,
                &current_speed,
                true,
                Some(message.clone()),
            );
            return Err(message);
        }

        let run = match run_result {
            Ok(run) => run,
            Err(err) => {
                remove_partial_output(&output_path);
                progress.emit(
                    &app_handle,
                    0.0,
                    0.0,
                    &current_speed,
                    true,
                    Some(err.clone()),
                );
                return Err(err);
            }
        };

        if !run.status.success() {
            // 출력 파일이 있어도 성공이 아니다 — 여기 오는 대부분은 중간에 끊긴 파일이다.
            remove_partial_output(&output_path);
            let message = run.failure_message("파일 병합에 실패했습니다");
            progress.emit(
                &app_handle,
                0.0,
                0.0,
                &current_speed,
                true,
                Some(message.clone()),
            );
            return Err(message);
        }

        progress.emit(
            &app_handle,
            100.0,
            total_duration,
            &current_speed,
            true,
            None,
        );
        Ok(output_path.to_string_lossy().to_string())
    }
}
