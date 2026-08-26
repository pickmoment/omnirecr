use std::time::{Duration, Instant};

/// 2nd order Butterworth High-pass filter (80 Hz cutoff)
/// Removes low-frequency microphone rumble, AC hum (50/60Hz), and handling vibrations
#[derive(Debug, Clone)]
pub struct BiquadHighPass80Hz {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1_l: f32,
    x2_l: f32,
    y1_l: f32,
    y2_l: f32,
    x1_r: f32,
    x2_r: f32,
    y1_r: f32,
    y2_r: f32,
}

impl BiquadHighPass80Hz {
    pub fn new(sample_rate: f32) -> Self {
        let cutoff_hz = 80.0;
        let q = std::f32::consts::FRAC_1_SQRT_2; // 0.7071 Butterworth

        let omega = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate;
        let alpha = omega.sin() / (2.0 * q);
        let cos_omega = omega.cos();

        let b0 = (1.0 + cos_omega) / 2.0;
        let b1 = -(1.0 + cos_omega);
        let b2 = (1.0 + cos_omega) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_omega;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1_l: 0.0,
            x2_l: 0.0,
            y1_l: 0.0,
            y2_l: 0.0,
            x1_r: 0.0,
            x2_r: 0.0,
            y1_r: 0.0,
            y2_r: 0.0,
        }
    }

    #[inline]
    pub fn process_mono(&mut self, sample: f32) -> f32 {
        let y = self.b0 * sample + self.b1 * self.x1_l + self.b2 * self.x2_l
            - self.a1 * self.y1_l
            - self.a2 * self.y2_l;
        self.x2_l = self.x1_l;
        self.x1_l = sample;
        self.y2_l = self.y1_l;
        self.y1_l = y;
        y
    }

    #[inline]
    pub fn process_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        let y_l = self.b0 * left + self.b1 * self.x1_l + self.b2 * self.x2_l
            - self.a1 * self.y1_l
            - self.a2 * self.y2_l;
        self.x2_l = self.x1_l;
        self.x1_l = left;
        self.y2_l = self.y1_l;
        self.y1_l = y_l;

        let y_r = self.b0 * right + self.b1 * self.x1_r + self.b2 * self.x2_r
            - self.a1 * self.y1_r
            - self.a2 * self.y2_r;
        self.x2_r = self.x1_r;
        self.x1_r = right;
        self.y2_r = self.y1_r;
        self.y1_r = y_r;

        (y_l, y_r)
    }
}

/// Smart Studio Noise Gate with Hysteresis, Hold Buffer, and Smooth Gain Transition
#[derive(Debug, Clone)]
pub struct NoiseGate {
    open_threshold_linear: f32,
    close_threshold_linear: f32,
    envelope: f32,
    attack_coeff: f32,
    release_coeff: f32,
    current_gain: f32,
    hold_samples: usize,
    hold_counter: usize,
    is_open: bool,
    attenuation_floor: f32,
}

impl NoiseGate {
    pub fn new(threshold_db: f32, sample_rate: f32) -> Self {
        let open_threshold_linear = 10.0f32.powf(threshold_db / 20.0);
        // Hysteresis: close threshold is 3.5dB lower than open threshold to prevent chatter
        let close_threshold_linear = 10.0f32.powf((threshold_db - 3.5) / 20.0);

        let attack_time_sec = 0.003; // 3ms fast attack
        let release_time_sec = 0.120; // 120ms natural release
        let hold_time_sec = 0.045; // 45ms hold buffer for natural voice pauses

        let attack_coeff = (-1.0 / (attack_time_sec * sample_rate)).exp();
        let release_coeff = (-1.0 / (release_time_sec * sample_rate)).exp();
        let hold_samples = (hold_time_sec * sample_rate).round() as usize;

        // Attenuation floor at -36dB (0.0158), attenuates noise floor by >98% while avoiding harsh pumping
        let attenuation_floor = 10.0f32.powf(-36.0 / 20.0);

        Self {
            open_threshold_linear,
            close_threshold_linear,
            envelope: 0.0,
            attack_coeff,
            release_coeff,
            current_gain: 0.0,
            hold_samples,
            hold_counter: 0,
            is_open: false,
            attenuation_floor,
        }
    }

    pub fn set_threshold_db(&mut self, threshold_db: f32) {
        self.open_threshold_linear = 10.0f32.powf(threshold_db / 20.0);
        self.close_threshold_linear = 10.0f32.powf((threshold_db - 3.5) / 20.0);
    }

    #[inline]
    pub fn process_sample(&mut self, sample: f32) -> f32 {
        let abs_sample = sample.abs();

        // Smooth envelope follower with fast attack and natural decay
        if abs_sample > self.envelope {
            self.envelope = self.attack_coeff * self.envelope + (1.0 - self.attack_coeff) * abs_sample;
        } else {
            self.envelope = self.release_coeff * self.envelope + (1.0 - self.release_coeff) * abs_sample;
        }

        // Gate state machine with hysteresis and hold timer
        if self.envelope >= self.open_threshold_linear {
            self.is_open = true;
            self.hold_counter = self.hold_samples;
        } else if self.is_open {
            if self.envelope < self.close_threshold_linear {
                if self.hold_counter > 0 {
                    self.hold_counter -= 1;
                } else {
                    self.is_open = false;
                }
            } else {
                // In hysteresis band between close and open thresholds: maintain hold
                self.hold_counter = self.hold_samples;
            }
        }

        let target_gain = if self.is_open {
            1.0
        } else {
            self.attenuation_floor
        };

        // Smooth gain transition (anti-click)
        if target_gain > self.current_gain {
            self.current_gain = self.attack_coeff * self.current_gain + (1.0 - self.attack_coeff) * target_gain;
        } else {
            self.current_gain = self.release_coeff * self.current_gain + (1.0 - self.release_coeff) * target_gain;
        }

        sample * self.current_gain
    }
}

/// Silence Detector for Auto-Pause and Auto-Stop using real-time clock
#[derive(Debug, Clone)]
pub struct SilenceDetector {
    silence_threshold_linear: f32,
    silence_start_time: Option<Instant>,
    auto_pause_enabled: bool,
    auto_pause_duration: Duration,
    auto_stop_enabled: bool,
    auto_stop_duration: Duration,
    is_paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceAction {
    None,
    TriggerPause,
    TriggerResume,
    TriggerStop,
}

impl SilenceDetector {
    pub fn new(
        auto_pause_enabled: bool,
        auto_pause_secs: f32,
        auto_stop_enabled: bool,
        auto_stop_secs: f32,
    ) -> Self {
        let silence_threshold_db = -45.0; // Calibrated silence threshold
        let silence_threshold_linear = 10.0f32.powf(silence_threshold_db / 20.0);

        Self {
            silence_threshold_linear,
            silence_start_time: None,
            auto_pause_enabled,
            auto_pause_duration: Duration::from_secs_f32(auto_pause_secs.max(0.5)),
            auto_stop_enabled,
            auto_stop_duration: Duration::from_secs_f32(auto_stop_secs.max(1.0)),
            is_paused: false,
        }
    }

    pub fn set_threshold_db(&mut self, db: f32) {
        self.silence_threshold_linear = 10.0f32.powf(db / 20.0);
    }

    /// Process RMS level and determine if auto-pause/resume/stop should trigger
    pub fn process_level(&mut self, block_rms: f32) -> SilenceAction {
        if !self.auto_pause_enabled && !self.auto_stop_enabled {
            return SilenceAction::None;
        }

        let is_silent = block_rms < self.silence_threshold_linear;

        if is_silent {
            if self.silence_start_time.is_none() {
                self.silence_start_time = Some(Instant::now());
            }

            let elapsed = self.silence_start_time.unwrap().elapsed();

            if self.auto_stop_enabled && elapsed >= self.auto_stop_duration {
                return SilenceAction::TriggerStop;
            }

            if self.auto_pause_enabled && !self.is_paused && elapsed >= self.auto_pause_duration {
                self.is_paused = true;
                return SilenceAction::TriggerPause;
            }
        } else {
            self.silence_start_time = None;
            if self.is_paused {
                self.is_paused = false;
                return SilenceAction::TriggerResume;
            }
        }

        SilenceAction::None
    }
}

/// Robust fractional linear resampler with continuous phase and boundary history across stream chunks
#[derive(Debug, Clone)]
pub struct StereoLinearResampler {
    from_rate: f32,
    to_rate: f32,
    ratio: f32, // from_rate / to_rate
    input_pos: f32,
    prev_l: f32,
    prev_r: f32,
}

impl StereoLinearResampler {
    pub fn new(from_rate: f32, to_rate: f32) -> Self {
        let ratio = from_rate / to_rate;
        Self {
            from_rate,
            to_rate,
            ratio,
            input_pos: 0.0,
            prev_l: 0.0,
            prev_r: 0.0,
        }
    }

    /// Resamples an interleaved stereo slice [L, R, L, R, ...] into destination vector at target sample rate
    pub fn process_interleaved(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if (self.from_rate - self.to_rate).abs() < 1.0 {
            output.extend_from_slice(input);
            return;
        }

        let num_frames = input.len() / 2;
        if num_frames == 0 {
            return;
        }

        let mut pos = self.input_pos;
        while (pos as usize) < num_frames {
            let idx = pos as usize;
            let frac = pos - (idx as f32);

            let s0_l = if idx == 0 {
                self.prev_l
            } else {
                input[(idx - 1) * 2]
            };
            let s0_r = if idx == 0 {
                self.prev_r
            } else {
                input[(idx - 1) * 2 + 1]
            };

            let s1_l = input[idx * 2];
            let s1_r = input[idx * 2 + 1];

            let out_l = s0_l + (s1_l - s0_l) * frac;
            let out_r = s0_r + (s1_r - s0_r) * frac;

            output.push(out_l);
            output.push(out_r);

            pos += self.ratio;
        }

        self.input_pos = pos - (num_frames as f32);
        self.prev_l = input[(num_frames - 1) * 2];
        self.prev_r = input[(num_frames - 1) * 2 + 1];
    }
}

/// Convert linear RMS amplitude to dBFS
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.000001 {
        -60.0
    } else {
        (20.0 * linear.log10()).clamp(-60.0, 6.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highpass_80hz_attenuation() {
        let sample_rate = 48000.0;
        let mut hpf = BiquadHighPass80Hz::new(sample_rate);

        // 30Hz sine wave (below cutoff) vs 1000Hz sine wave (above cutoff)
        let num_samples = 4800; // 100ms
        let mut in_30hz_energy = 0.0f32;
        let mut out_30hz_energy = 0.0f32;

        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let sample = (2.0 * std::f32::consts::PI * 30.0 * t).sin();
            in_30hz_energy += sample * sample;
            let filtered = hpf.process_mono(sample);
            if i > 1000 {
                // steady state
                out_30hz_energy += filtered * filtered;
            }
        }

        let ratio_30hz = (out_30hz_energy / (in_30hz_energy * 0.79)).sqrt();
        // 30Hz should be significantly attenuated (>10dB attenuation, ratio < 0.3)
        assert!(ratio_30hz < 0.35, "30Hz signal must be attenuated by HPF, got ratio: {}", ratio_30hz);

        // Test 1000Hz signal passes through
        let mut hpf_1k = BiquadHighPass80Hz::new(sample_rate);
        let mut out_1k_energy = 0.0f32;
        let mut in_1k_energy = 0.0f32;
        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let sample = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            in_1k_energy += sample * sample;
            let filtered = hpf_1k.process_mono(sample);
            if i > 1000 {
                out_1k_energy += filtered * filtered;
            }
        }
        let ratio_1k = (out_1k_energy / (in_1k_energy * 0.79)).sqrt();
        assert!((ratio_1k - 1.0).abs() < 0.05, "1000Hz signal should pass with unity gain, got: {}", ratio_1k);
    }

    #[test]
    fn test_noise_gate_suppresses_low_amplitude_noise() {
        let sample_rate = 48000.0;
        let mut gate = NoiseGate::new(-40.0, sample_rate); // -40dB threshold (~0.01 linear)

        // Feed background hiss of amplitude 0.002 (-54dB)
        let mut output_sum = 0.0f32;
        for i in 0..4800 {
            let sample = if i % 2 == 0 { 0.002 } else { -0.002 };
            let out = gate.process_sample(sample);
            if i > 2000 {
                output_sum += out.abs();
            }
        }

        // Noise should be attenuated to near the floor (-36dB gain reduction)
        let avg_out = output_sum / 2800.0;
        assert!(avg_out < 0.0001, "Low level noise must be heavily attenuated by noise gate, got: {}", avg_out);
    }

    #[test]
    fn test_resampler_streaming_continuity() {
        let mut resampler = StereoLinearResampler::new(44100.0, 48000.0);
        let chunk_size = 256;
        let total_input_frames = 44100;
        let mut output = Vec::new();

        let mut input_buffer = Vec::with_capacity(chunk_size * 2);
        for frame in 0..total_input_frames {
            let t = frame as f32 / 44100.0;
            let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            input_buffer.push(s);
            input_buffer.push(s);

            if input_buffer.len() == chunk_size * 2 {
                resampler.process_interleaved(&input_buffer, &mut output);
                input_buffer.clear();
            }
        }
        if !input_buffer.is_empty() {
            resampler.process_interleaved(&input_buffer, &mut output);
        }

        let output_frames = output.len() / 2;
        // 44100 -> 48000 expected exactly ~48000 frames
        assert!(
            (output_frames as i32 - 48000).abs() <= 2,
            "Resampled output frames count mismatch: got {}, expected ~48000",
            output_frames
        );
    }
}
