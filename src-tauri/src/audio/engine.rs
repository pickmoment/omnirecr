use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::io::Write;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::dsp::{
    linear_to_db, BiquadHighPass80Hz, NoiseGate, SilenceAction, SilenceDetector,
    StereoLinearResampler,
};
use crate::types::Settings;

/// 실시간 캡처 콜백과 믹싱 워커 사이 링버퍼가 들고 있을 청크 수 상한.
///
/// cpal/ScreenCaptureKit 콜백 한 번은 보통 5~20ms 분량이라 64개는 0.6~1.3초에
/// 해당한다. 워커가 어차피 250ms 넘는 큐를 잘라내므로 이 정도면 정상 동작에는
/// 절대 닿지 않고, FFmpeg 파이프가 막혔을 때 메모리 사용량만 묶어 준다.
const CAPTURE_RING_CHUNKS: usize = 64;

/// 이 개수를 넘게 버리면 한 번 경고한다(매번 찍으면 로그가 폭발한다).
const DROPPED_CHUNK_WARN_THRESHOLD: u64 = 32;

/// 출력 오디오 스테레오 f32 한 프레임의 바이트 수(f32 × 2채널).
const OUTPUT_FRAME_BYTES: usize = 8;

/// 벽시계보다 이만큼 이상 뒤처지면 무음으로 메운다.
const SILENCE_PAD_TRIGGER_MS: f64 = 200.0;

/// 무음으로 메운 뒤 남겨 두는 여유. 0 으로 메우면 곧 도착할 실제 프레임이
/// 타임라인을 앞질러 버린다.
const SILENCE_PAD_MARGIN_MS: f64 = 50.0;

/// 이만큼 프레임이 오지 않으면 캡처가 멈춘 것으로 보고 VU · 무음 감지기에 무음을 넣는다.
/// 정상 캡처는 10ms 안팎 간격으로 청크를 주므로 청크 사이의 빈틈에는 걸리지 않는다.
const CAPTURE_STALL_SILENCE_MS: u64 = 250;

/// 캡처 정지가 이만큼 이어지면 세션당 한 번 경고를 남긴다(진단용).
const CAPTURE_STALL_WARN_MS: u64 = 2000;

/// 캡처가 프레임을 주고 있는지 추적한다. 워커 루프가 매 회차 `observe()` 를 부른다.
struct CaptureStallTracker {
    last_frames_at: Instant,
    warned: bool,
}

/// `CaptureStallTracker::observe()` 의 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureFlow {
    /// 이번 회차에 프레임을 처리했다 — 실제 레벨로 판정한다.
    Frames,
    /// 청크 사이의 짧은 빈틈 — 아직 판정을 바꾸지 않는다.
    Gap,
    /// 캡처가 멈췄다 — 무음으로 판정한다. `first_warning` 은 이 정지 구간에서 첫 경고 시점.
    Stalled { first_warning: bool },
}

impl CaptureStallTracker {
    fn new(now: Instant) -> Self {
        Self {
            last_frames_at: now,
            warned: false,
        }
    }

    fn observe(&mut self, frames_processed: usize, now: Instant) -> CaptureFlow {
        if frames_processed > 0 {
            self.last_frames_at = now;
            self.warned = false;
            return CaptureFlow::Frames;
        }
        let since = now.saturating_duration_since(self.last_frames_at);
        if since < Duration::from_millis(CAPTURE_STALL_SILENCE_MS) {
            return CaptureFlow::Gap;
        }
        let first_warning = !self.warned && since >= Duration::from_millis(CAPTURE_STALL_WARN_MS);
        if first_warning {
            self.warned = true;
        }
        CaptureFlow::Stalled { first_warning }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEngineEvent {
    AutoPause,
    AutoResume,
    AutoStop,
    /// 캡처나 인코딩이 죽어 더 이상 녹음이 진행되지 않는 상태.
    ///
    /// 세션당 **한 번만** 발행된다(`FatalReporter` 가 보장). 장치 오류는 콜백마다
    /// 계속 올라오기 때문에 그대로 흘리면 리스너가 같은 세션을 여러 번 정리하려
    /// 들어 상태가 꼬인다.
    Fatal(String),
}

/// 출력 오디오 타임라인을 어떻게 유지할지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineMode {
    /// 화면 녹화. 오디오 스트림은 항상 벽시계와 같은 길이여야 한다.
    ///
    /// FFmpeg 의 `-shortest` 는 가장 짧은 입력에서 인코딩을 끝내는데, 화면 입력
    /// (`gdigrab`/`avfoundation`/`x11grab`)은 스스로 끝나지 않으므로 `-shortest`
    /// 가 유일한 크로스플랫폼 정지 레버다(빼면 stdin EOF 로도 FFmpeg 이 끝나지
    /// 않아 강제 kill → moov 미기록으로 MP4 가 재생 불가가 된다).
    /// 그래서 오디오가 짧아지면 그만큼 **영상 뒷부분이 잘려 나간다** — 일시정지
    /// 구간과 캡처가 멈춘 구간을 무음으로 메워 그런 일이 없게 한다.
    WallClock,
    /// 오디오 전용 녹음. 일시정지한 시간은 결과 파일에 담지 않는다 —
    /// 사용자가 기대하는 동작이고, 맞춰야 할 영상 트랙도 없다.
    SkipPaused,
}

/// 치명적 실패를 세션당 딱 한 번만 올리는 게이트.
pub struct FatalReporter {
    /// `mpsc::Sender` 는 `Sync` 가 아니라 여러 스레드(실시간 오디오 콜백,
    /// ScreenCaptureKit 디스패치 큐, 믹싱 워커)에서 공유하려면 잠금이 필요하다.
    sender: Mutex<Sender<AudioEngineEvent>>,
    reported: AtomicBool,
    message: Mutex<Option<String>>,
}

impl FatalReporter {
    fn new(sender: Sender<AudioEngineEvent>) -> Self {
        Self {
            sender: Mutex::new(sender),
            reported: AtomicBool::new(false),
            message: Mutex::new(None),
        }
    }

    /// 첫 보고면 메시지를 그대로 돌려주고, 이미 보고했으면 `None`.
    fn claim(&self, message: String) -> Option<String> {
        if self
            .reported
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }
        log::error!("오디오 캡처/인코딩 치명적 실패: {message}");
        *self.message.lock() = Some(message.clone());
        Some(message)
    }

    /// 녹음 중에 발견한 실패 — 이벤트로 올려 세션을 정리하게 한다.
    pub fn report(&self, message: impl Into<String>) {
        let Some(message) = self.claim(message.into()) else {
            return;
        };
        if self
            .sender
            .lock()
            .send(AudioEngineEvent::Fatal(message))
            .is_err()
        {
            log::warn!("치명적 실패를 리스너에 전달하지 못했다(리스너가 이미 종료됨).");
        }
    }

    /// 정지 중에 발견한 실패 — 기록만 하고 이벤트는 올리지 않는다.
    ///
    /// 이미 정리 중인 세션을 리스너가 한 번 더 정리하려 들면 상태가 꼬인다.
    /// 대신 `AudioCaptureEngine::fatal_message()` 로 정지 경로가 읽어 가
    /// `stop()` 의 Err 메시지에 합친다.
    fn record_only(&self, message: impl Into<String>) {
        self.claim(message.into());
    }

    fn message(&self) -> Option<String> {
        self.message.lock().clone()
    }
}

/// 실시간 캡처 콜백 → 믹싱 워커 사이의 **유계** 링버퍼.
///
/// 예전에는 무한 `std::sync::mpsc::channel` 이었다. 소비자(FFmpeg 파이프)가 막히면
/// 콜백이 밀어 넣는 프레임이 그대로 쌓여 메모리를 무한히 먹었다 — 장시간 녹화 중
/// 앱이 OOM 으로 죽던 경로다. 가득 차면 **가장 오래된** 청크를 버린다: 실시간
/// 오디오에서 의미 있는 건 최신 프레임이고, 워커도 250ms 넘는 큐는 잘라낸다.
pub struct CaptureRing {
    chunks: Mutex<VecDeque<Vec<f32>>>,
    dropped: AtomicU64,
}

impl CaptureRing {
    fn new() -> Self {
        Self {
            chunks: Mutex::new(VecDeque::with_capacity(CAPTURE_RING_CHUNKS)),
            dropped: AtomicU64::new(0),
        }
    }

    /// 실시간 오디오 콜백에서 호출된다. **절대 블로킹하지 않는다** — 잠금 구간은
    /// `push_back`/`pop_front` 뿐이고, 밀려난 버퍼의 해제는 잠금을 놓은 뒤에 한다.
    pub fn push(&self, chunk: Vec<f32>) {
        let evicted = {
            let mut chunks = self.chunks.lock();
            let evicted = if chunks.len() >= CAPTURE_RING_CHUNKS {
                chunks.pop_front()
            } else {
                None
            };
            chunks.push_back(chunk);
            evicted
        };
        if evicted.is_some() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        drop(evicted);
    }

    fn pop(&self) -> Option<Vec<f32>> {
        self.chunks.lock().pop_front()
    }

    fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// cpal 장치에서 기본 설정을 읽을 때 출력(루프백) 장치인지 입력 장치인지.
#[derive(Clone, Copy)]
enum DeviceRole {
    /// 시스템 출력 장치의 루프백(WASAPI). 기본 설정은 출력 설정에서 읽는다.
    /// macOS 는 시스템 소리를 ScreenCaptureKit 으로 잡으므로 이 경로가 없다.
    #[cfg(not(target_os = "macos"))]
    SystemLoopback,
    /// 마이크 등 실제 입력 장치.
    Microphone,
}

#[inline]
fn frames_to_process(
    mic_enabled: bool,
    system_enabled: bool,
    mic_samples: usize,
    sys_samples: usize,
) -> usize {
    if mic_enabled {
        mic_samples / 2
    } else if system_enabled {
        sys_samples / 2
    } else {
        0
    }
}

#[inline]
fn interleave_stereo_f32(data: &[f32], channels: usize) -> Vec<f32> {
    let mut stereo = Vec::with_capacity((data.len() / channels) * 2);
    for frame in data.chunks_exact(channels) {
        if channels == 1 {
            stereo.push(frame[0]);
            stereo.push(frame[0]);
        } else {
            stereo.push(frame[0]);
            stereo.push(frame[1]);
        }
    }
    stereo
}

#[inline]
fn interleave_stereo_i16(data: &[i16], channels: usize) -> Vec<f32> {
    let mut stereo = Vec::with_capacity((data.len() / channels) * 2);
    for frame in data.chunks_exact(channels) {
        if channels == 1 {
            let sample = frame[0] as f32 / 32768.0;
            stereo.push(sample);
            stereo.push(sample);
        } else {
            stereo.push(frame[0] as f32 / 32768.0);
            stereo.push(frame[1] as f32 / 32768.0);
        }
    }
    stereo
}

/// cpal 입력 스트림 하나를 만들어 재생까지 시작한다.
///
/// 실패를 `None` 으로 삼키지 않고 전부 `Err` 로 올린다. 예전에는 장치가 없거나
/// 샘플 포맷이 지원되지 않으면 조용히 스트림 없이 계속 돌았고, 사용자는 녹음이
/// 되는 줄 알고 끝까지 진행한 뒤 무음 파일을 받았다.
fn start_capture_stream(
    device: &cpal::Device,
    role: DeviceRole,
    ring: Arc<CaptureRing>,
    reporter: Arc<FatalReporter>,
    label: &'static str,
) -> Result<(cpal::Stream, f32), String> {
    let default_cfg = match role {
        #[cfg(not(target_os = "macos"))]
        DeviceRole::SystemLoopback => device.default_output_config(),
        DeviceRole::Microphone => device.default_input_config(),
    }
    .map_err(|error| format!("{label} 장치의 기본 오디오 설정을 읽을 수 없습니다: {error}"))?;

    let rate = default_cfg.sample_rate().0;
    let format = default_cfg.sample_format();
    let channel_count = default_cfg.channels();
    let channels = channel_count as usize;

    if channels == 0 || rate == 0 {
        return Err(format!(
            "{label} 장치가 보고한 오디오 설정이 잘못되었습니다(채널 {channels}, {rate}Hz)."
        ));
    }

    let config = StreamConfig {
        channels: channel_count,
        sample_rate: SampleRate(rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = match format {
        SampleFormat::F32 => {
            let err_reporter = reporter.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| ring.push(interleave_stereo_f32(data, channels)),
                move |error| {
                    // 장치가 사라지거나 백엔드가 스트림을 끊으면 여기로 온다.
                    // 예전에는 eprintln 만 하고 계속 "녹음 중"이었다.
                    err_reporter.report(format!("{label} 캡처 스트림이 중단되었습니다: {error}"));
                },
                None,
            )
        }
        SampleFormat::I16 => {
            let err_reporter = reporter.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &_| ring.push(interleave_stereo_i16(data, channels)),
                move |error| {
                    err_reporter.report(format!("{label} 캡처 스트림이 중단되었습니다: {error}"));
                },
                None,
            )
        }
        other => {
            return Err(format!(
                "{label} 장치의 샘플 포맷 {other:?} 은 지원하지 않습니다. 시스템 오디오 설정에서 16bit 정수 또는 32bit 부동소수 포맷으로 바꿔 주세요."
            ));
        }
    }
    .map_err(|error| format!("{label} 캡처 스트림을 만들 수 없습니다: {error}"))?;

    stream
        .play()
        .map_err(|error| format!("{label} 캡처 스트림을 시작할 수 없습니다: {error}"))?;

    Ok((stream, rate as f32))
}

/// 시스템 소리 루프백 스트림 준비(macOS 는 ScreenCaptureKit 을 쓰므로 제외).
#[cfg(not(target_os = "macos"))]
fn prepare_system_stream(
    host: &cpal::Host,
    enabled: bool,
    ring: &Arc<CaptureRing>,
    reporter: &Arc<FatalReporter>,
    fallback_rate: f32,
) -> Result<(Option<cpal::Stream>, f32), String> {
    if !enabled {
        return Ok((None, fallback_rate));
    }
    let device = host
        .default_output_device()
        .ok_or_else(|| "시스템 소리를 캡처할 기본 출력 장치를 찾을 수 없습니다.".to_string())?;
    let (stream, rate) = start_capture_stream(
        &device,
        DeviceRole::SystemLoopback,
        ring.clone(),
        reporter.clone(),
        "시스템 소리",
    )?;
    Ok((Some(stream), rate))
}

/// 마이크 입력 스트림 준비.
fn prepare_mic_stream(
    host: &cpal::Host,
    enabled: bool,
    ring: &Arc<CaptureRing>,
    reporter: &Arc<FatalReporter>,
    fallback_rate: f32,
) -> Result<(Option<cpal::Stream>, f32), String> {
    if !enabled {
        return Ok((None, fallback_rate));
    }
    let device = host
        .default_input_device()
        .ok_or_else(|| "기본 마이크 입력 장치를 찾을 수 없습니다.".to_string())?;
    let (stream, rate) = start_capture_stream(
        &device,
        DeviceRole::Microphone,
        ring.clone(),
        reporter.clone(),
        "마이크",
    )?;
    Ok((Some(stream), rate))
}

/// 캡처 준비 실패를 `start()` 로 돌려주고 워커를 끝낸다.
///
/// 이 함수가 돌아간 뒤 워커가 반환되면서 `ffmpeg_stdin` 이 drop 되어 FFmpeg 은
/// EOF 를 받는다. 자식 프로세스 수거는 세션(`recorder::audio` / `recorder::screen`)
/// 이 `abort_ffmpeg` 로 처리한다.
fn fail_setup(
    ready_tx: &Sender<Result<(), String>>,
    worker_finished: &Arc<AtomicBool>,
    error: String,
) {
    if ready_tx.send(Err(error)).is_err() {
        log::warn!("캡처 준비 실패를 start() 에 전달하지 못했다(호출자가 이미 포기함).");
    }
    worker_finished.store(true, Ordering::SeqCst);
}

pub struct AudioCaptureEngine {
    is_running: Arc<AtomicBool>,
    /// 사용자가 명시적으로 누른 일시정지.
    manual_paused: Arc<AtomicBool>,
    /// 무음 감지가 자동으로 건 일시정지.
    ///
    /// 실효 일시정지 = 수동 OR 자동. 두 사유를 하나의 플래그로 합쳐 두면 무음이
    /// 끝났을 때 오는 자동 재개가 사용자의 수동 일시정지까지 풀어 버려, 사용자가
    /// 멈춰 둔 줄 아는 동안 녹음이 계속된다.
    auto_paused: Arc<AtomicBool>,
    sys_vu_level: Arc<Mutex<f32>>,
    mic_vu_level: Arc<Mutex<f32>>,
    /// 워커가 루프를 빠져나와 FFmpeg stdin 을 닫고 종료했음을 알리는 완료 플래그.
    worker_finished: Arc<AtomicBool>,
    thread_handle: Mutex<Option<thread::JoinHandle<()>>>,
    reporter: Arc<FatalReporter>,
}

impl AudioCaptureEngine {
    /// 캡처 스트림을 열고 믹싱 워커를 띄운다.
    ///
    /// cpal `Stream` 은 `Send` 가 아니라 워커 스레드 안에서만 만들 수 있다. 그래서
    /// 워커가 준비를 마친 뒤 결과를 되돌려 주고, 이 함수는 그 결과를 기다린 다음
    /// 반환한다 — 장치 없음 · 미지원 포맷 · 스트림 생성/재생 실패를 **여기서**
    /// `Err` 로 낼 수 있게 하는 것이 핵심이다.
    pub fn start(
        settings: &Settings,
        mut ffmpeg_stdin: ChildStdin,
        event_sender: Sender<AudioEngineEvent>,
        timeline: TimelineMode,
    ) -> Result<Self, String> {
        let is_running = Arc::new(AtomicBool::new(true));
        let manual_paused = Arc::new(AtomicBool::new(false));
        let auto_paused = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::new(AtomicBool::new(false));
        let sys_vu_level = Arc::new(Mutex::new(-60.0f32));
        let mic_vu_level = Arc::new(Mutex::new(-60.0f32));
        let reporter = Arc::new(FatalReporter::new(event_sender.clone()));

        let running_clone = is_running.clone();
        let manual_paused_clone = manual_paused.clone();
        let auto_paused_clone = auto_paused.clone();
        let worker_finished_clone = worker_finished.clone();
        let sys_vu_clone = sys_vu_level.clone();
        let mic_vu_clone = mic_vu_level.clone();
        let reporter_clone = reporter.clone();

        let system_enabled = settings.system_audio_enabled;
        let sys_gain = settings.system_audio_volume;
        let mic_enabled = settings.mic_audio_enabled;
        let mic_gain = settings.mic_audio_volume;

        let noise_gate_enabled = settings.noise_gate_enabled;
        let noise_gate_db = settings.noise_gate_threshold_db;
        let hpf_enabled = settings.highpass_filter_enabled;

        let auto_pause_enabled = settings.auto_pause_enabled;
        let auto_pause_sec = settings.auto_pause_seconds;
        let auto_stop_enabled = settings.auto_stop_enabled;
        let auto_stop_sec = settings.auto_stop_seconds;

        let target_sample_rate_hz = settings.audio_sample_rate;
        let target_sample_rate = target_sample_rate_hz as f32;

        // 오디오 전용 녹음은 캡처 소스가 하나도 없으면 결과물이 존재할 수 없다.
        // 예전에는 그냥 시작해서 FFmpeg 에 아무 것도 보내지 않았고, 0바이트 파일이
        // 정상 결과처럼 히스토리에 올라갔다. (화면 녹화는 무음 트랙이 정상 결과이니
        // 여기서 막지 않는다 — `TimelineMode::WallClock` 이 무음으로 메운다.)
        if timeline == TimelineMode::SkipPaused && !system_enabled && !mic_enabled {
            return Err(
                "녹음할 오디오 입력이 없습니다. 설정에서 시스템 소리나 마이크 중 하나를 켜 주세요."
                    .to_string(),
            );
        }

        let sys_ring = Arc::new(CaptureRing::new());
        let mic_ring = Arc::new(CaptureRing::new());

        #[cfg(target_os = "macos")]
        let mac_system_capture = if system_enabled {
            Some(crate::audio::macos::MacSystemAudioCapture::start(
                sys_ring.clone(),
                settings.system_audio_include_own_app,
                reporter.clone(),
            )?)
        } else {
            None
        };

        let (ready_tx, ready_rx) = channel::<Result<(), String>>();

        let sys_ring_worker = sys_ring.clone();
        let mic_ring_worker = mic_ring.clone();

        let handle = thread::spawn(move || {
            let host = cpal::default_host();

            // 1. 시스템 소리 캡처
            #[cfg(target_os = "macos")]
            let (sys_stream_guard, sys_actual_rate) = (
                mac_system_capture,
                crate::audio::macos::SYSTEM_AUDIO_SAMPLE_RATE_HZ as f32,
            );

            #[cfg(not(target_os = "macos"))]
            let (sys_stream_guard, sys_actual_rate) = match prepare_system_stream(
                &host,
                system_enabled,
                &sys_ring_worker,
                &reporter_clone,
                target_sample_rate,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    fail_setup(&ready_tx, &worker_finished_clone, error);
                    return;
                }
            };

            // 2. 마이크 캡처
            let (mic_stream_guard, mic_actual_rate) = match prepare_mic_stream(
                &host,
                mic_enabled,
                &mic_ring_worker,
                &reporter_clone,
                target_sample_rate,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    fail_setup(&ready_tx, &worker_finished_clone, error);
                    return;
                }
            };

            if ready_tx.send(Ok(())).is_err() {
                log::warn!("캡처 준비 완료를 start() 에 전달하지 못했다 — 녹음을 시작하지 않는다.");
                worker_finished_clone.store(true, Ordering::SeqCst);
                return;
            }

            let mut sys_resampler = StereoLinearResampler::new(sys_actual_rate, target_sample_rate);
            let mut mic_resampler = StereoLinearResampler::new(mic_actual_rate, target_sample_rate);

            let mut noise_gate_l = NoiseGate::new(noise_gate_db, target_sample_rate);
            let mut noise_gate_r = NoiseGate::new(noise_gate_db, target_sample_rate);
            let mut hpf_mic = BiquadHighPass80Hz::new(target_sample_rate);
            let mut silence_detector = SilenceDetector::new(
                auto_pause_enabled,
                auto_pause_sec,
                auto_stop_enabled,
                auto_stop_sec,
            );

            let mut sys_queue: VecDeque<f32> = VecDeque::with_capacity(48000);
            let mut mic_queue: VecDeque<f32> = VecDeque::with_capacity(48000);
            let mut resampled_temp: Vec<f32> = Vec::with_capacity(8192);
            let mut output_bytes: Vec<u8> = Vec::with_capacity(8192);

            let max_queue_samples = (target_sample_rate * 0.25).round() as usize * 2; // 250ms max buffer to prevent drift

            let frames_per_ms = target_sample_rate as f64 / 1000.0;
            let pad_trigger_frames = (frames_per_ms * SILENCE_PAD_TRIGGER_MS) as u64;
            let pad_margin_frames = (frames_per_ms * SILENCE_PAD_MARGIN_MS) as u64;
            // 한 번에 메울 수 있는 최대량(1초). 캡처가 몇 분 멈춰도 한 회차에
            // 수십 MB 를 할당하지 않고 3ms 루프를 돌며 따라잡는다.
            let pad_chunk_limit_frames = target_sample_rate as u64;
            let timeline_started = Instant::now();
            let mut frames_written: u64 = 0;
            let mut drop_warning_emitted = false;
            let mut pad_warning_emitted = false;
            let mut stall = CaptureStallTracker::new(Instant::now());

            while running_clone.load(Ordering::SeqCst) {
                let mut received_any = false;

                // 1. Drain incoming system audio
                while let Some(chunk) = sys_ring_worker.pop() {
                    received_any = true;
                    resampled_temp.clear();
                    sys_resampler.process_interleaved(&chunk, &mut resampled_temp);
                    sys_queue.extend(resampled_temp.drain(..));
                }

                // 2. Drain incoming mic audio
                while let Some(chunk) = mic_ring_worker.pop() {
                    received_any = true;
                    resampled_temp.clear();
                    mic_resampler.process_interleaved(&chunk, &mut resampled_temp);
                    mic_queue.extend(resampled_temp.drain(..));
                }

                if !drop_warning_emitted {
                    let dropped = sys_ring_worker.dropped_count() + mic_ring_worker.dropped_count();
                    if dropped > DROPPED_CHUNK_WARN_THRESHOLD {
                        drop_warning_emitted = true;
                        log::warn!(
                            "실시간 캡처 링버퍼가 가득 차 청크 {dropped}개를 버렸습니다 — FFmpeg 인코딩이 캡처 속도를 따라가지 못하고 있습니다."
                        );
                    }
                }

                // Limit queues to prevent drift (always aligned to stereo frames)
                if sys_queue.len() > max_queue_samples {
                    let excess = ((sys_queue.len() - max_queue_samples) / 2) * 2;
                    if excess > 0 {
                        sys_queue.drain(..excess);
                    }
                }
                if mic_queue.len() > max_queue_samples {
                    let excess = ((mic_queue.len() - max_queue_samples) / 2) * 2;
                    if excess > 0 {
                        mic_queue.drain(..excess);
                    }
                }

                let is_paused_now = manual_paused_clone.load(Ordering::SeqCst)
                    || auto_paused_clone.load(Ordering::SeqCst);
                // 일시정지 중 출력을 아예 빼면 오디오 타임라인이 그만큼 짧아진다.
                // 화면 녹화(`WallClock`)에서는 그 차이가 `-shortest` 를 통해 영상
                // 뒷부분 잘림 + A/V 어긋남으로 나타나므로, 프레임 수는 유지하고
                // 값만 0 으로 내보낸다.
                let emit_output = !is_paused_now || timeline == TimelineMode::WallClock;

                output_bytes.clear();
                let mut sys_rms_accum = 0.0f32;
                let mut mic_rms_accum = 0.0f32;
                let mut mixed_rms_accum = 0.0f32;
                let mut frames_processed = 0usize;

                if mic_enabled && system_enabled {
                    // The microphone is the sole master clock. System audio is mixed when available,
                    // but a separate ScreenCaptureKit callback must never create extra output frames.
                    let available_frames =
                        frames_to_process(true, true, mic_queue.len(), sys_queue.len());
                    if available_frames > 0 {
                        for _ in 0..available_frames {
                            let sys_l = sys_queue.pop_front().unwrap_or(0.0);
                            let sys_r = sys_queue.pop_front().unwrap_or(0.0);
                            let mic_l = mic_queue.pop_front().unwrap_or(0.0);
                            let mic_r = mic_queue.pop_front().unwrap_or(0.0);

                            sys_rms_accum += sys_l * sys_l + sys_r * sys_r;
                            mic_rms_accum += mic_l * mic_l + mic_r * mic_r;

                            let scaled_sys_l = sys_l * sys_gain;
                            let scaled_sys_r = sys_r * sys_gain;

                            let mut proc_mic_l = mic_l * mic_gain;
                            let mut proc_mic_r = mic_r * mic_gain;

                            if hpf_enabled {
                                let (fl, fr) = hpf_mic.process_stereo(proc_mic_l, proc_mic_r);
                                proc_mic_l = fl;
                                proc_mic_r = fr;
                            }

                            if noise_gate_enabled {
                                proc_mic_l = noise_gate_l.process_sample(proc_mic_l);
                                proc_mic_r = noise_gate_r.process_sample(proc_mic_r);
                            }

                            let mix_l = (scaled_sys_l + proc_mic_l).clamp(-1.0, 1.0);
                            let mix_r = (scaled_sys_r + proc_mic_r).clamp(-1.0, 1.0);

                            mixed_rms_accum += mix_l * mix_l + mix_r * mix_r;

                            if emit_output {
                                if is_paused_now {
                                    output_bytes.resize(output_bytes.len() + OUTPUT_FRAME_BYTES, 0);
                                } else {
                                    output_bytes.extend_from_slice(&mix_l.to_le_bytes());
                                    output_bytes.extend_from_slice(&mix_r.to_le_bytes());
                                }
                            }
                        }
                        frames_processed = available_frames;
                    }
                } else if mic_enabled {
                    // Only mic active: process all available mic frames directly
                    let available_frames = mic_queue.len() / 2;
                    if available_frames > 0 {
                        for _ in 0..available_frames {
                            let mic_l = mic_queue.pop_front().unwrap_or(0.0);
                            let mic_r = mic_queue.pop_front().unwrap_or(0.0);

                            mic_rms_accum += mic_l * mic_l + mic_r * mic_r;

                            let mut proc_mic_l = mic_l * mic_gain;
                            let mut proc_mic_r = mic_r * mic_gain;

                            if hpf_enabled {
                                let (fl, fr) = hpf_mic.process_stereo(proc_mic_l, proc_mic_r);
                                proc_mic_l = fl;
                                proc_mic_r = fr;
                            }

                            if noise_gate_enabled {
                                proc_mic_l = noise_gate_l.process_sample(proc_mic_l);
                                proc_mic_r = noise_gate_r.process_sample(proc_mic_r);
                            }

                            let mix_l = proc_mic_l.clamp(-1.0, 1.0);
                            let mix_r = proc_mic_r.clamp(-1.0, 1.0);

                            mixed_rms_accum += mix_l * mix_l + mix_r * mix_r;

                            if emit_output {
                                if is_paused_now {
                                    output_bytes.resize(output_bytes.len() + OUTPUT_FRAME_BYTES, 0);
                                } else {
                                    output_bytes.extend_from_slice(&mix_l.to_le_bytes());
                                    output_bytes.extend_from_slice(&mix_r.to_le_bytes());
                                }
                            }
                        }
                        frames_processed = available_frames;
                    }
                } else if system_enabled {
                    // Only system audio active
                    let available_frames = sys_queue.len() / 2;
                    if available_frames > 0 {
                        for _ in 0..available_frames {
                            let sys_l = sys_queue.pop_front().unwrap_or(0.0);
                            let sys_r = sys_queue.pop_front().unwrap_or(0.0);

                            sys_rms_accum += sys_l * sys_l + sys_r * sys_r;

                            let mix_l = (sys_l * sys_gain).clamp(-1.0, 1.0);
                            let mix_r = (sys_r * sys_gain).clamp(-1.0, 1.0);

                            mixed_rms_accum += mix_l * mix_l + mix_r * mix_r;

                            if emit_output {
                                if is_paused_now {
                                    output_bytes.resize(output_bytes.len() + OUTPUT_FRAME_BYTES, 0);
                                } else {
                                    output_bytes.extend_from_slice(&mix_l.to_le_bytes());
                                    output_bytes.extend_from_slice(&mix_r.to_le_bytes());
                                }
                            }
                        }
                        frames_processed = available_frames;
                    }
                }

                // 캡처가 프레임을 주지 않는 동안에도 레벨 판정은 계속되어야 한다.
                //
                // 예전에는 `frames_processed == 0` 이면 VU 와 무음 감지기를 건드리지
                // 않아 **마지막 블록 값이 그대로 얼어붙었다**. 시스템 오디오 캡처가
                // 발화 도중 끊기면(출력 장치 전환 · ScreenCaptureKit 드롭아웃) VU 는 계속
                // "소리 있음" 을 가리키고 무음 감지기는 무음을 재지 않아, 대본 자동 녹음이
                // 파일에 아무것도 쓰지 못하면서 끝나지도 않았다(하드캡까지 수 분 대기).
                // 프레임이 없다는 것은 결과 파일 기준으로 무음이므로 그렇게 다룬다 —
                // `SkipPaused` 는 아무것도 쓰지 않고, `WallClock` 은 아래에서 0 으로 메운다.
                let levels = match stall.observe(frames_processed, Instant::now()) {
                    CaptureFlow::Frames => {
                        let num_samples = (frames_processed * 2) as f32;
                        Some((
                            (sys_rms_accum / num_samples).sqrt(),
                            (mic_rms_accum / num_samples).sqrt(),
                            (mixed_rms_accum / num_samples).sqrt(),
                        ))
                    }
                    CaptureFlow::Gap => None,
                    CaptureFlow::Stalled { first_warning } => {
                        if first_warning {
                            log::warn!(
                                "오디오 캡처가 {CAPTURE_STALL_WARN_MS}ms 넘게 프레임을 주지 않습니다 — 무음으로 처리합니다."
                            );
                        }
                        Some((0.0, 0.0, 0.0))
                    }
                };

                if let Some((sys_rms, mic_rms, mixed_rms)) = levels {
                    *sys_vu_clone.lock() = linear_to_db(sys_rms);
                    *mic_vu_clone.lock() = linear_to_db(mic_rms);

                    let action = silence_detector.process_level(mixed_rms);
                    match action {
                        SilenceAction::TriggerPause => {
                            auto_paused_clone.store(true, Ordering::SeqCst);
                            if event_sender.send(AudioEngineEvent::AutoPause).is_err() {
                                log::warn!("자동 일시정지를 리스너에 전달하지 못했다.");
                            }
                        }
                        SilenceAction::TriggerResume => {
                            // **자동 사유만** 해제한다. 수동 일시정지까지 풀면
                            // 사용자가 멈춰 둔 줄 아는 동안 녹음이 계속된다.
                            auto_paused_clone.store(false, Ordering::SeqCst);
                            if event_sender.send(AudioEngineEvent::AutoResume).is_err() {
                                log::warn!("자동 재개를 리스너에 전달하지 못했다.");
                            }
                        }
                        SilenceAction::TriggerStop => {
                            if event_sender.send(AudioEngineEvent::AutoStop).is_err() {
                                log::warn!("자동 종료를 리스너에 전달하지 못했다.");
                            }
                        }
                        SilenceAction::None => {}
                    }
                }

                // 화면 녹화에서는 캡처가 아예 멈춘 구간(장치 정지 · 캡처 소스를
                // 모두 끈 설정 · 백엔드 드롭아웃)도 무음으로 메워야 한다.
                // 그러지 않으면 오디오가 벽시계보다 짧아져 `-shortest` 가 그만큼
                // 영상 뒤를 잘라낸다(캡처 소스를 다 끄면 영상이 통째로 사라졌다).
                if timeline == TimelineMode::WallClock {
                    let expected = (timeline_started.elapsed().as_secs_f64()
                        * target_sample_rate as f64) as u64;
                    let projected =
                        frames_written + (output_bytes.len() / OUTPUT_FRAME_BYTES) as u64;
                    if expected > projected + pad_trigger_frames {
                        let pad = (expected - projected - pad_margin_frames)
                            .min(pad_chunk_limit_frames) as usize;
                        output_bytes.resize(output_bytes.len() + pad * OUTPUT_FRAME_BYTES, 0);
                        if !pad_warning_emitted {
                            pad_warning_emitted = true;
                            log::warn!(
                                "오디오 캡처가 벽시계보다 뒤처져 무음으로 메우고 있습니다(A/V 동기 유지)."
                            );
                        }
                    }
                }

                if !output_bytes.is_empty() {
                    if let Err(error) = ffmpeg_stdin.write_all(&output_bytes) {
                        // 정지 중이라면 호출자가 FFmpeg 을 kill 해 파이프를 깬
                        // 것이므로 조용히 나간다. 녹음 중이면 인코더가 죽은 것이고
                        // 이후 오디오는 전부 유실되므로 치명적 실패로 올린다.
                        if running_clone.load(Ordering::SeqCst) {
                            reporter_clone
                                .report(format!("FFmpeg 오디오 파이프에 쓸 수 없습니다: {error}"));
                        }
                        break;
                    }
                    frames_written += (output_bytes.len() / OUTPUT_FRAME_BYTES) as u64;
                }

                // Adaptive sleep: if no data was received or processed, sleep briefly (3ms) to prevent busy looping
                if !received_any && frames_processed == 0 {
                    thread::sleep(Duration::from_millis(3));
                }
            }

            // 캡처를 먼저 멈춘 뒤 파이프를 닫는다.
            drop(mic_stream_guard);
            drop(sys_stream_guard);

            if let Err(error) = ffmpeg_stdin.flush() {
                // 마지막 flush 실패 = 결과 파일 꼬리가 유실됐다는 뜻이다.
                // 정지 중이라 이벤트로 올리지 않고 기록만 하고, `stop()` 이
                // Err 메시지에 합쳐 사용자에게 알린다.
                reporter_clone
                    .record_only(format!("FFmpeg 오디오 파이프를 비우지 못했습니다: {error}"));
            }
            drop(ffmpeg_stdin);
            worker_finished_clone.store(true, Ordering::SeqCst);
        });

        // 준비 결과를 기다린다. 장치 열기가 병리적으로 오래 걸릴 수 있으므로
        // 무한 대기하지 않고 유계로 기다린다 — 타임아웃이면 워커에 정지를 지시하고
        // 실패로 반환한다(워커는 준비를 마치는 즉시 정지 플래그를 보고 빠져나온다).
        let ready = ready_rx.recv_timeout(Duration::from_secs(15));
        match ready {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                is_running.store(false, Ordering::SeqCst);
                return Err(error);
            }
            Err(RecvTimeoutError::Timeout) => {
                is_running.store(false, Ordering::SeqCst);
                return Err(
                    "오디오 캡처 장치를 15초 안에 열지 못했습니다. 장치 연결 상태를 확인해 주세요."
                        .to_string(),
                );
            }
            Err(RecvTimeoutError::Disconnected) => {
                is_running.store(false, Ordering::SeqCst);
                return Err(
                    "오디오 캡처 준비 스레드가 결과를 남기지 못하고 종료했습니다.".to_string(),
                );
            }
        }

        Ok(Self {
            is_running,
            manual_paused,
            auto_paused,
            sys_vu_level,
            mic_vu_level,
            worker_finished,
            thread_handle: Mutex::new(Some(handle)),
            reporter,
        })
    }

    /// 사용자 일시정지. 자동 사유와 별도로 보관한다.
    pub fn pause(&self) {
        self.manual_paused.store(true, Ordering::SeqCst);
    }

    /// 사용자 재개. 사용자가 명시적으로 재개를 눌렀으면 자동 사유까지 함께 푼다 —
    /// 그러지 않으면 무음 자동 일시정지가 걸린 상태에서 재개를 눌러도 엔진이 계속
    /// 멈춰 있어 UI 는 "녹음 중"인데 파일에는 아무것도 안 들어간다.
    pub fn resume(&self) {
        self.auto_paused.store(false, Ordering::SeqCst);
        self.manual_paused.store(false, Ordering::SeqCst);
    }

    /// 캡처/인코딩이 죽었을 때 기록된 진단 메시지.
    pub fn fatal_message(&self) -> Option<String> {
        self.reporter.message()
    }

    /// 정지를 요청하고 워커가 FFmpeg stdin 을 닫을 때까지 **최대 `timeout` 만큼만**
    /// 기다린다. 유계 시간 안에 끝났으면 true.
    ///
    /// **왜 곧바로 `join()` 하지 않는가**: 워커는 FFmpeg stdin 에 블로킹 write 를
    /// 한다. FFmpeg 이 파이프를 비우지 못하는 상태(인코더 과부하 · 디스크 정지)면
    /// 워커는 커널 안에서 잠들어 정지 플래그를 볼 수 없다. 예전 코드는 그 상태에서
    /// 무한 `join()` 을 호출했기 때문에 "3초 타임아웃"에 도달하지도 못하고 정지
    /// 커맨드를 부른 스레드(동기 Tauri 커맨드 = 메인 스레드)가 영구히 멈췄다.
    /// 그래서 완료 플래그를 유계로 폴링하고, 안 끝나면 false 를 돌려준다. 호출자는
    /// false 를 받으면 FFmpeg 자식을 kill 해 파이프를 깨야 한다 — 막힌 write 가
    /// EPIPE 로 깨어나면서 워커가 스스로 종료한다.
    pub fn stop_within(&self, timeout: Duration) -> bool {
        self.is_running.store(false, Ordering::SeqCst);
        let finished = self.wait_finished(timeout);
        if finished {
            self.join_finished_worker();
        }
        finished
    }

    /// FFmpeg 을 kill 한 뒤 파이프가 깨져 빠져나온 워커를 수거한다.
    ///
    /// 유계 시간 안에 끝나지 않으면 스레드를 분리한 채 반환한다 — 프로세스를
    /// 무한정 붙잡고 있는 것보다 낫고, FFmpeg 이 이미 죽었으므로 워커는 곧 write
    /// 오류로 빠져나온다.
    pub fn reap_worker(&self, timeout: Duration) -> bool {
        let finished = self.wait_finished(timeout);
        if finished {
            self.join_finished_worker();
        } else {
            log::warn!("오디오 믹싱 워커가 유계 시간 안에 종료하지 않아 분리했습니다.");
            drop(self.thread_handle.lock().take());
        }
        finished
    }

    fn wait_finished(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.worker_finished.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    /// 완료 플래그가 이미 서 있을 때만 부른다. 그 시점의 `join()` 은 스레드
    /// 정리만 기다리므로 사실상 즉시 돌아온다.
    fn join_finished_worker(&self) {
        let handle = self.thread_handle.lock().take();
        if let Some(handle) = handle {
            if handle.join().is_err() {
                log::error!("오디오 믹싱 워커가 패닉으로 종료했습니다.");
            }
        }
    }

    pub fn get_vu_levels(&self) -> (f32, f32) {
        (*self.sys_vu_level.lock(), *self.mic_vu_level.lock())
    }
}

impl Drop for AudioCaptureEngine {
    /// 세션이 정상 정지 경로를 타지 않고 버려져도 워커가 영원히 남지 않게 한다.
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        frames_to_process, interleave_stereo_f32, interleave_stereo_i16, CaptureFlow, CaptureRing,
        CaptureStallTracker, CAPTURE_STALL_SILENCE_MS, CAPTURE_STALL_WARN_MS,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn microphone_is_the_dual_stream_master_clock() {
        assert_eq!(frames_to_process(true, true, 960, 0), 480);
        assert_eq!(frames_to_process(true, true, 0, 960), 0);
        assert_eq!(frames_to_process(true, true, 960, 960), 480);
    }

    #[test]
    fn system_audio_clocks_output_only_without_microphone() {
        assert_eq!(frames_to_process(false, true, 0, 960), 480);
        assert_eq!(frames_to_process(false, false, 960, 960), 0);
    }

    #[test]
    fn mono_capture_is_duplicated_into_both_channels() {
        assert_eq!(
            interleave_stereo_f32(&[0.5, -0.5], 1),
            vec![0.5, 0.5, -0.5, -0.5]
        );
        assert_eq!(
            interleave_stereo_i16(&[-32768, 0], 1),
            vec![-1.0, -1.0, 0.0, 0.0]
        );
    }

    /// 5.1 같은 다채널 입력은 앞 두 채널만 쓴다(뒤 채널을 섞으면 위상이 깨진다).
    #[test]
    fn multichannel_capture_keeps_the_first_two_channels() {
        assert_eq!(
            interleave_stereo_f32(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 3),
            vec![0.1, 0.2, 0.4, 0.5]
        );
    }

    /// 링버퍼가 가득 차면 가장 오래된 청크가 밀려나고, 버린 수가 집계된다.
    /// 이 정책이 없으면 FFmpeg 파이프가 막힌 동안 큐가 무한히 자란다.
    #[test]
    fn capture_ring_drops_the_oldest_chunk_when_full() {
        let ring = CaptureRing::new();
        for index in 0..(super::CAPTURE_RING_CHUNKS + 3) {
            ring.push(vec![index as f32]);
        }

        assert_eq!(ring.dropped_count(), 3);
        // 가장 오래된 3개(0,1,2)가 밀려나고 3번부터 남아 있다.
        assert_eq!(ring.pop(), Some(vec![3.0]));

        let mut remaining = 1;
        while ring.pop().is_some() {
            remaining += 1;
        }
        assert_eq!(remaining, super::CAPTURE_RING_CHUNKS);
    }

    /// 캡처가 프레임을 주지 않으면 레벨 판정은 **무음** 으로 이어져야 한다. 예전에는
    /// 이때 VU 와 무음 감지기를 건드리지 않아 마지막 블록 값이 얼어붙었고, 발화 도중
    /// 시스템 오디오 캡처가 끊기면 대본 자동 녹음이 끝나지도 기록하지도 않았다.
    #[test]
    fn capture_stall_is_reported_as_silence_after_the_grace_gap() {
        let start = Instant::now();
        let mut tracker = CaptureStallTracker::new(start);
        let ms = |offset: u64| start + Duration::from_millis(offset);

        assert_eq!(tracker.observe(480, ms(10)), CaptureFlow::Frames);
        // 청크 사이의 정상적인 빈틈은 판정을 바꾸지 않는다.
        assert_eq!(tracker.observe(0, ms(20)), CaptureFlow::Gap);
        assert_eq!(
            tracker.observe(0, ms(10 + CAPTURE_STALL_SILENCE_MS - 1)),
            CaptureFlow::Gap
        );
        // 유예를 넘기면 무음으로 판정하되, 경고는 아직 아니다.
        assert_eq!(
            tracker.observe(0, ms(10 + CAPTURE_STALL_SILENCE_MS)),
            CaptureFlow::Stalled { first_warning: false }
        );
        // 경고는 정지 구간마다 정확히 한 번.
        assert_eq!(
            tracker.observe(0, ms(10 + CAPTURE_STALL_WARN_MS)),
            CaptureFlow::Stalled { first_warning: true }
        );
        assert_eq!(
            tracker.observe(0, ms(10 + CAPTURE_STALL_WARN_MS + 500)),
            CaptureFlow::Stalled { first_warning: false }
        );
        // 프레임이 돌아오면 정상으로 복귀하고, 다음 정지에서 다시 경고할 수 있다.
        assert_eq!(tracker.observe(480, ms(5000)), CaptureFlow::Frames);
        assert_eq!(
            tracker.observe(0, ms(5000 + CAPTURE_STALL_WARN_MS)),
            CaptureFlow::Stalled { first_warning: true }
        );
    }
}
