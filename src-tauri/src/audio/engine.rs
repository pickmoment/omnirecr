use std::collections::VecDeque;
use std::io::Write;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use parking_lot::Mutex;

use crate::audio::dsp::{
    linear_to_db, BiquadHighPass80Hz, NoiseGate, SilenceAction, SilenceDetector, StereoLinearResampler,
};
use crate::types::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEngineEvent {
    AutoPause,
    AutoResume,
    AutoStop,
}

pub struct AudioCaptureEngine {
    is_running: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    sys_vu_level: Arc<Mutex<f32>>,
    mic_vu_level: Arc<Mutex<f32>>,
    thread_handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl AudioCaptureEngine {
    pub fn start(
        settings: &Settings,
        mut ffmpeg_stdin: ChildStdin,
        event_sender: Sender<AudioEngineEvent>,
    ) -> Result<Self, String> {
        let is_running = Arc::new(AtomicBool::new(true));
        let is_paused = Arc::new(AtomicBool::new(false));
        let sys_vu_level = Arc::new(Mutex::new(-60.0f32));
        let mic_vu_level = Arc::new(Mutex::new(-60.0f32));

        let running_clone = is_running.clone();
        let paused_clone = is_paused.clone();
        let sys_vu_clone = sys_vu_level.clone();
        let mic_vu_clone = mic_vu_level.clone();

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

        let (sys_tx, sys_rx) = channel::<Vec<f32>>();

        #[cfg(target_os = "macos")]
        let mac_system_capture = if system_enabled {
            Some(crate::audio::macos::MacSystemAudioCapture::start(
                sys_tx.clone(),
                target_sample_rate_hz,
            )?)
        } else {
            None
        };

        let handle = thread::spawn(move || {
            let host = cpal::default_host();

            let (mic_tx, mic_rx) = channel::<Vec<f32>>();

            #[cfg(target_os = "macos")]
            let sys_actual_rate = target_sample_rate;
            #[cfg(not(target_os = "macos"))]
            let mut sys_actual_rate = target_sample_rate;
            let mut mic_actual_rate = target_sample_rate;

            // 1. Platform system-output capture
            #[cfg(target_os = "macos")]
            let _sys_stream = mac_system_capture;

            #[cfg(not(target_os = "macos"))]
            let _sys_stream = if system_enabled {
                if let Some(device) = host.default_output_device() {
                    let default_cfg = device.default_output_config().ok();
                    let rate = default_cfg.as_ref().map(|c| c.sample_rate().0).unwrap_or(48000);
                    let format = default_cfg.as_ref().map(|c| c.sample_format()).unwrap_or(SampleFormat::F32);
                    let channels = default_cfg.as_ref().map(|c| c.channels()).unwrap_or(2) as usize;
                    sys_actual_rate = rate as f32;

                    let config = StreamConfig {
                        channels: channels as u16,
                        sample_rate: SampleRate(rate),
                        buffer_size: cpal::BufferSize::Default,
                    };

                    let tx = sys_tx.clone();
                    let stream_res = match format {
                        SampleFormat::F32 => {
                            device.build_input_stream(
                                &config,
                                move |data: &[f32], _: &_| {
                                    let mut stereo_data = Vec::with_capacity((data.len() / channels) * 2);
                                    for frame in data.chunks_exact(channels) {
                                        if channels == 1 {
                                            stereo_data.push(frame[0]);
                                            stereo_data.push(frame[0]);
                                        } else {
                                            stereo_data.push(frame[0]);
                                            stereo_data.push(frame[1]);
                                        }
                                    }
                                    let _ = tx.send(stereo_data);
                                },
                                |err| eprintln!("WASAPI loopback error: {}", err),
                                None,
                            )
                        }
                        SampleFormat::I16 => {
                            device.build_input_stream(
                                &config,
                                move |data: &[i16], _: &_| {
                                    let mut stereo_data = Vec::with_capacity((data.len() / channels) * 2);
                                    for frame in data.chunks_exact(channels) {
                                        if channels == 1 {
                                            let s = frame[0] as f32 / 32768.0;
                                            stereo_data.push(s);
                                            stereo_data.push(s);
                                        } else {
                                            stereo_data.push(frame[0] as f32 / 32768.0);
                                            stereo_data.push(frame[1] as f32 / 32768.0);
                                        }
                                    }
                                    let _ = tx.send(stereo_data);
                                },
                                |err| eprintln!("WASAPI loopback error: {}", err),
                                None,
                            )
                        }
                        _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
                    };

                    if let Ok(stream) = stream_res {
                        let _ = stream.play();
                        Some(stream)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // 2. Microphone Input Capture
            let _mic_stream = if mic_enabled {
                if let Some(device) = host.default_input_device() {
                    let default_cfg = device.default_input_config().ok();
                    let rate = default_cfg.as_ref().map(|c| c.sample_rate().0).unwrap_or(48000);
                    let format = default_cfg.as_ref().map(|c| c.sample_format()).unwrap_or(SampleFormat::F32);
                    let channels = default_cfg.as_ref().map(|c| c.channels()).unwrap_or(1) as usize;
                    mic_actual_rate = rate as f32;

                    let config = StreamConfig {
                        channels: channels as u16,
                        sample_rate: SampleRate(rate),
                        buffer_size: cpal::BufferSize::Default,
                    };

                    let tx = mic_tx.clone();
                    let stream_res = match format {
                        SampleFormat::F32 => {
                            device.build_input_stream(
                                &config,
                                move |data: &[f32], _: &_| {
                                    let mut stereo_data = Vec::with_capacity((data.len() / channels) * 2);
                                    for frame in data.chunks_exact(channels) {
                                        if channels == 1 {
                                            stereo_data.push(frame[0]);
                                            stereo_data.push(frame[0]);
                                        } else {
                                            stereo_data.push(frame[0]);
                                            stereo_data.push(frame[1]);
                                        }
                                    }
                                    let _ = tx.send(stereo_data);
                                },
                                |err| eprintln!("Mic input error: {}", err),
                                None,
                            )
                        }
                        SampleFormat::I16 => {
                            device.build_input_stream(
                                &config,
                                move |data: &[i16], _: &_| {
                                    let mut stereo_data = Vec::with_capacity((data.len() / channels) * 2);
                                    for frame in data.chunks_exact(channels) {
                                        if channels == 1 {
                                            let s = frame[0] as f32 / 32768.0;
                                            stereo_data.push(s);
                                            stereo_data.push(s);
                                        } else {
                                            stereo_data.push(frame[0] as f32 / 32768.0);
                                            stereo_data.push(frame[1] as f32 / 32768.0);
                                        }
                                    }
                                    let _ = tx.send(stereo_data);
                                },
                                |err| eprintln!("Mic input error: {}", err),
                                None,
                            )
                        }
                        _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
                    };

                    if let Ok(stream) = stream_res {
                        let _ = stream.play();
                        Some(stream)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let mut sys_resampler = StereoLinearResampler::new(sys_actual_rate, target_sample_rate);
            let mut mic_resampler = StereoLinearResampler::new(mic_actual_rate, target_sample_rate);

            let mut noise_gate_l = NoiseGate::new(noise_gate_db, target_sample_rate);
            let mut noise_gate_r = NoiseGate::new(noise_gate_db, target_sample_rate);
            let mut hpf = BiquadHighPass80Hz::new(target_sample_rate);
            let mut silence_detector = SilenceDetector::new(
                auto_pause_enabled,
                auto_pause_sec,
                auto_stop_enabled,
                auto_stop_sec,
            );

            // 20ms block frames for stable, low-latency streaming
            let block_frames = (target_sample_rate * 0.020).round() as usize;
            let block_duration = Duration::from_millis(20);
            let max_queue_samples = block_frames * 2 * 10; // Max 200ms buffering to prevent drift

            let mut sys_queue: VecDeque<f32> = VecDeque::with_capacity(block_frames * 8);
            let mut mic_queue: VecDeque<f32> = VecDeque::with_capacity(block_frames * 8);
            let mut resampled_temp: Vec<f32> = Vec::with_capacity(block_frames * 4);
            let mut output_bytes: Vec<u8> = Vec::with_capacity(block_frames * 2 * 4);

            let mut next_tick = Instant::now() + block_duration;

            while running_clone.load(Ordering::SeqCst) {
                // Drain system audio into resampler
                while let Ok(chunk) = sys_rx.try_recv() {
                    resampled_temp.clear();
                    sys_resampler.process_interleaved(&chunk, &mut resampled_temp);
                    sys_queue.extend(resampled_temp.drain(..));
                }

                // Drain mic audio into resampler
                while let Ok(chunk) = mic_rx.try_recv() {
                    resampled_temp.clear();
                    mic_resampler.process_interleaved(&chunk, &mut resampled_temp);
                    mic_queue.extend(resampled_temp.drain(..));
                }

                // Limit queue sizes to prevent latency buildup
                if sys_queue.len() > max_queue_samples {
                    let excess = sys_queue.len() - max_queue_samples;
                    sys_queue.drain(..excess);
                }
                if mic_queue.len() > max_queue_samples {
                    let excess = mic_queue.len() - max_queue_samples;
                    mic_queue.drain(..excess);
                }

                let is_paused_now = paused_clone.load(Ordering::SeqCst);

                let frames_to_process = block_frames;
                let mut sys_rms_accum = 0.0f32;
                let mut mic_rms_accum = 0.0f32;
                let mut mixed_rms_accum = 0.0f32;

                output_bytes.clear();

                for _ in 0..frames_to_process {
                    let sys_l = sys_queue.pop_front().unwrap_or(0.0);
                    let sys_r = sys_queue.pop_front().unwrap_or(sys_l);

                    let mic_l = mic_queue.pop_front().unwrap_or(0.0);
                    let mic_r = mic_queue.pop_front().unwrap_or(mic_l);

                    sys_rms_accum += sys_l * sys_l + sys_r * sys_r;
                    mic_rms_accum += mic_l * mic_l + mic_r * mic_r;

                    let scaled_sys_l = sys_l * sys_gain;
                    let scaled_sys_r = sys_r * sys_gain;

                    let mut scaled_mic_l = mic_l * mic_gain;
                    let mut scaled_mic_r = mic_r * mic_gain;

                    if noise_gate_enabled {
                        scaled_mic_l = noise_gate_l.process_sample(scaled_mic_l);
                        scaled_mic_r = noise_gate_r.process_sample(scaled_mic_r);
                    }

                    let mut mix_l = scaled_sys_l + scaled_mic_l;
                    let mut mix_r = scaled_sys_r + scaled_mic_r;

                    if hpf_enabled {
                        let (fl, fr) = hpf.process_stereo(mix_l, mix_r);
                        mix_l = fl;
                        mix_r = fr;
                    }

                    mix_l = mix_l.clamp(-1.0, 1.0);
                    mix_r = mix_r.clamp(-1.0, 1.0);

                    mixed_rms_accum += mix_l * mix_l + mix_r * mix_r;

                    if is_paused_now {
                        mix_l = 0.0;
                        mix_r = 0.0;
                    }

                    output_bytes.extend_from_slice(&mix_l.to_le_bytes());
                    output_bytes.extend_from_slice(&mix_r.to_le_bytes());
                }

                let num_samples = (frames_to_process * 2) as f32;
                let sys_rms = (sys_rms_accum / num_samples).sqrt();
                let mic_rms = (mic_rms_accum / num_samples).sqrt();
                let mixed_rms = (mixed_rms_accum / num_samples).sqrt();

                *sys_vu_clone.lock() = linear_to_db(sys_rms);
                *mic_vu_clone.lock() = linear_to_db(mic_rms);

                let action = silence_detector.process_level(mixed_rms);
                match action {
                    SilenceAction::TriggerPause => {
                        paused_clone.store(true, Ordering::SeqCst);
                        let _ = event_sender.send(AudioEngineEvent::AutoPause);
                    }
                    SilenceAction::TriggerResume => {
                        paused_clone.store(false, Ordering::SeqCst);
                        let _ = event_sender.send(AudioEngineEvent::AutoResume);
                    }
                    SilenceAction::TriggerStop => {
                        let _ = event_sender.send(AudioEngineEvent::AutoStop);
                    }
                    SilenceAction::None => {}
                }

                if !output_bytes.is_empty() {
                    if ffmpeg_stdin.write_all(&output_bytes).is_err() {
                        break;
                    }
                }

                // Wall-clock pacing
                let now = Instant::now();
                if next_tick > now {
                    thread::sleep(next_tick - now);
                }
                next_tick += block_duration;
                if next_tick < Instant::now() {
                    next_tick = Instant::now() + block_duration;
                }
            }

            let _ = ffmpeg_stdin.flush();
            drop(ffmpeg_stdin);
        });

        Ok(Self {
            is_running,
            is_paused,
            sys_vu_level,
            mic_vu_level,
            thread_handle: Mutex::new(Some(handle)),
        })
    }

    pub fn pause(&self) {
        self.is_paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.lock().take() {
            let _ = handle.join();
        }
    }

    pub fn get_vu_levels(&self) -> (f32, f32) {
        (*self.sys_vu_level.lock(), *self.mic_vu_level.lock())
    }
}
