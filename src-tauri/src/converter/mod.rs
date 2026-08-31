use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

use crate::merger::{
    ffmpeg_base_command, remove_partial_output, run_ffmpeg_with_progress, ChildJobs,
    MergerController,
};
use crate::settings::SettingsManager;
use crate::types::{AudioConvertProgressPayload, AudioConvertTaskPayload};

/// 한 파일 변환의 진행 이벤트에 매번 들어가는 고정 정보.
struct FileProgress {
    file_index: usize,
    total_files: usize,
    file_name: String,
    output_file_path: String,
    total_time_secs: f64,
}

impl FileProgress {
    fn is_last(&self) -> bool {
        self.file_index + 1 == self.total_files
    }

    fn emit(
        &self,
        app_handle: &AppHandle,
        percent: f32,
        current_time_secs: f64,
        speed: &str,
        finished: bool,
        error: Option<String>,
    ) {
        let overall_percent = (((self.file_index as f64 + (percent as f64 / 100.0))
            / self.total_files as f64)
            * 100.0)
            .clamp(0.0, 100.0) as f32;

        let payload = AudioConvertProgressPayload {
            file_index: self.file_index,
            total_files: self.total_files,
            current_file_name: self.file_name.clone(),
            output_file_path: self.output_file_path.clone(),
            percent,
            overall_percent,
            current_time_secs,
            total_time_secs: self.total_time_secs,
            speed: speed.to_string(),
            finished,
            error,
        };

        if let Err(err) = app_handle.emit("conversion_progress", &payload) {
            eprintln!("conversion_progress 이벤트 발행 실패: {}", err);
        }
    }
}

pub struct AudioConverterController {
    /// 자식 프로세스를 작업 ID 별로 보관한다. 컨트롤러에 슬롯 하나만 두면 변환 요청이
    /// 겹칠 때 나중 요청이 앞 요청의 핸들을 덮어써 FFmpeg 이 고아로 남는다.
    jobs: ChildJobs,
}

impl AudioConverterController {
    pub fn new() -> Self {
        Self {
            jobs: ChildJobs::new(),
        }
    }

    pub fn cancel(&self) {
        self.jobs.cancel_all();
    }

    pub fn convert(
        &self,
        app_handle: AppHandle,
        task: AudioConvertTaskPayload,
        custom_ffmpeg_path: Option<String>,
    ) -> Result<Vec<String>, String> {
        if task.input_files.is_empty() {
            return Err("변환할 입력 파일이 지정되지 않았습니다.".to_string());
        }

        let ffmpeg_path = SettingsManager::find_ffmpeg(custom_ffmpeg_path.as_deref())?;
        let probes = MergerController::probe_files(task.input_files.clone(), custom_ffmpeg_path)?;

        let total_files = task.input_files.len();
        let target_fmt = task.target_format.to_lowercase();
        let ext = if target_fmt == "mp3" { "mp3" } else { "m4a" };

        let job = self.jobs.begin();
        let mut converted_files = Vec::new();
        // 실패한 파일을 모아 뒀다가 마지막에 Err 로 올린다. 조용히 건너뛰면 요청한 개수보다
        // 적은 목록이 "성공" 으로 돌아가고, 사용자는 어떤 파일이 왜 빠졌는지 알 수 없다.
        let mut failures: Vec<String> = Vec::new();

        for (idx, input_path_str) in task.input_files.iter().enumerate() {
            if job.is_cancelled() {
                break;
            }

            let input_path = Path::new(input_path_str);
            let file_stem = input_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let file_name = input_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // probe 결과가 없다 = 파일이 없거나 미디어로 열 수 없다. 변환을 시도해도
            // 실패할 입력이므로 여기서 이유를 확정해 기록한다.
            let Some(probe) = probes.iter().find(|p| &p.path == input_path_str) else {
                let message = format!(
                    "'{}': 파일을 찾을 수 없거나 미디어 정보를 읽을 수 없습니다.",
                    input_path_str
                );
                let progress = FileProgress {
                    file_index: idx,
                    total_files,
                    file_name: file_name.clone(),
                    output_file_path: String::new(),
                    total_time_secs: 0.0,
                };
                progress.emit(
                    &app_handle,
                    100.0,
                    0.0,
                    "0x",
                    progress.is_last(),
                    Some(message.clone()),
                );
                failures.push(message);
                continue;
            };

            // 진행률 계산용 길이. 0 이면 나눗셈이 깨지므로 1초로 둔다(진행률만 부정확해지고
            // 성공 판정은 종료 코드가 한다).
            let duration = if probe.duration_secs <= 0.0 {
                1.0
            } else {
                probe.duration_secs
            };

            // Determine output path
            let output_dir = if let Some(dir) = &task.output_dir {
                if !dir.trim().is_empty() {
                    PathBuf::from(dir)
                } else {
                    input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
                }
            } else {
                input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
            };

            // Determine output filename
            let mut output_path = output_dir.join(format!("{}.{}", file_stem, ext));
            // If output_path is same as input_path, append _converted
            if output_path == input_path {
                output_path = output_dir.join(format!("{}_converted.{}", file_stem, ext));
            }

            let progress = FileProgress {
                file_index: idx,
                total_files,
                file_name: file_name.clone(),
                output_file_path: output_path.to_string_lossy().to_string(),
                total_time_secs: duration,
            };

            // 출력 폴더 생성 실패를 삼키면 FFmpeg 이 "No such file or directory" 로 죽고
            // 원인이 사용자에게 전달되지 않는다.
            if let Err(err) = fs::create_dir_all(&output_dir) {
                let message = format!(
                    "'{}': 출력 폴더를 만들 수 없습니다 ({}): {}",
                    file_name,
                    output_dir.display(),
                    err
                );
                progress.emit(
                    &app_handle,
                    100.0,
                    0.0,
                    "0x",
                    progress.is_last(),
                    Some(message.clone()),
                );
                failures.push(message);
                continue;
            }

            let mut cmd = ffmpeg_base_command(&ffmpeg_path);
            cmd.arg("-i").arg(input_path_str);

            if target_fmt == "mp3" {
                cmd.arg("-c:a")
                    .arg("libmp3lame")
                    .arg("-b:a")
                    .arg(format!("{}k", task.bitrate));
            } else {
                // m4a / aac
                cmd.arg("-c:a")
                    .arg("aac")
                    .arg("-b:a")
                    .arg(format!("{}k", task.bitrate));
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

            let mut current_speed = "1.0x".to_string();

            let run_result = run_ffmpeg_with_progress(cmd, &job, |key, value| {
                if key == "speed" {
                    current_speed = value.to_string();
                } else if key == "out_time_us" {
                    if let Ok(us) = value.parse::<f64>() {
                        let current_secs = us / 1_000_000.0;
                        let percent = ((current_secs / duration) * 100.0).clamp(0.0, 99.0) as f32;
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
                    // progress=end 만 보고 완료를 알리지 않는다. FFmpeg 은 여기까지 찍고도
                    // 0 이 아닌 코드로 죽을 수 있고(muxer 오류, 디스크 꽉 참), 그러면
                    // 잘린 파일이 성공으로 보고된다. 완료 통보는 종료 코드 확인 뒤.
                    progress.emit(&app_handle, 100.0, duration, &current_speed, false, None);
                }
            });

            if job.is_cancelled() {
                remove_partial_output(&output_path);
                let message = "변환 작업이 사용자에 의해 취소되었습니다.".to_string();
                // finished 를 올려 진행 UI 가 켜진 채 남지 않게 한다.
                progress.emit(
                    &app_handle,
                    100.0,
                    duration,
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
                    let message = format!("'{}': {}", file_name, err);
                    progress.emit(
                        &app_handle,
                        100.0,
                        duration,
                        &current_speed,
                        progress.is_last(),
                        Some(message.clone()),
                    );
                    failures.push(message);
                    continue;
                }
            };

            if !run.status.success() {
                // 출력 파일이 생겼는지는 성공의 근거가 아니다 — 여기 오는 파일은 대부분
                // 헤더만 있고 재생되지 않는 껍데기다.
                remove_partial_output(&output_path);
                let message = run.failure_message(&format!("'{}' 변환에 실패했습니다", file_name));
                progress.emit(
                    &app_handle,
                    100.0,
                    duration,
                    &current_speed,
                    progress.is_last(),
                    Some(message.clone()),
                );
                failures.push(message);
                continue;
            }

            progress.emit(
                &app_handle,
                100.0,
                duration,
                &current_speed,
                progress.is_last(),
                None,
            );
            converted_files.push(output_path.to_string_lossy().to_string());
        }

        if job.is_cancelled() {
            return Err("변환 작업이 사용자에 의해 취소되었습니다.".to_string());
        }

        if !failures.is_empty() {
            return Err(format!(
                "{}개 파일 중 {}개 변환에 실패했습니다.\n{}",
                total_files,
                failures.len(),
                failures.join("\n")
            ));
        }

        Ok(converted_files)
    }
}
