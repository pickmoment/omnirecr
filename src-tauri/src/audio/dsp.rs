use std::time::{Duration, Instant};

/// 2nd order Butterworth High-pass filter (80 Hz cutoff)
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
        let y = self.b0 * sample + self.b1 * self.x1_l + self.b2 * self.x2_l - self.a1 * self.y1_l - self.a2 * self.y2_l;
        self.x2_l = self.x1_l;
        self.x1_l = sample;
        self.y2_l = self.y1_l;
        self.y1_l = y;
        y
    }

    #[inline]
    pub fn process_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        let y_l = self.b0 * left + self.b1 * self.x1_l + self.b2 * self.x2_l - self.a1 * self.y1_l - self.a2 * self.y2_l;
        self.x2_l = self.x1_l;
        self.x1_l = left;
        self.y2_l = self.y1_l;
        self.y1_l = y_l;

        let y_r = self.b0 * right + self.b1 * self.x1_r + self.b2 * self.x2_r - self.a1 * self.y1_r - self.a2 * self.y2_r;
        self.x2_r = self.x1_r;
        self.x1_r = right;
        self.y2_r = self.y1_r;
        self.y1_r = y_r;

        (y_l, y_r)
    }
}

/// Smart Noise Gate with smooth envelope gain ramping
#[derive(Debug, Clone)]
pub struct NoiseGate {
    threshold_linear: f32,
    envelope: f32,
    attack_coeff: f32,
    release_coeff: f32,
    current_gain: f32,
}

impl NoiseGate {
    pub fn new(threshold_db: f32, sample_rate: f32) -> Self {
        let threshold_linear = 10.0f32.powf(threshold_db / 20.0);
        let attack_time_sec = 0.005; // 5ms attack
        let release_time_sec = 0.100; // 100ms release

        let attack_coeff = (-1.0 / (attack_time_sec * sample_rate)).exp();
        let release_coeff = (-1.0 / (release_time_sec * sample_rate)).exp();

        Self {
            threshold_linear,
            envelope: 0.0,
            attack_coeff,
            release_coeff,
            current_gain: 0.0,
        }
    }

    pub fn set_threshold_db(&mut self, threshold_db: f32) {
        self.threshold_linear = 10.0f32.powf(threshold_db / 20.0);
    }

    #[inline]
    pub fn process_sample(&mut self, sample: f32) -> f32 {
        let abs_sample = sample.abs();
        if abs_sample > self.envelope {
            self.envelope = self.attack_coeff * self.envelope + (1.0 - self.attack_coeff) * abs_sample;
        } else {
            self.envelope = self.release_coeff * self.envelope + (1.0 - self.release_coeff) * abs_sample;
        }

        let target_gain = if self.envelope >= self.threshold_linear {
            1.0
        } else {
            0.0
        };

        // Smooth gain transition
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

/// Simple and fast linear interpolation stereo resampler
#[derive(Debug, Clone)]
pub struct StereoLinearResampler {
    from_rate: f32,
    to_rate: f32,
    ratio: f32, // from_rate / to_rate
    phase: f32,
    last_l: f32,
    last_r: f32,
}

impl StereoLinearResampler {
    pub fn new(from_rate: f32, to_rate: f32) -> Self {
        let ratio = from_rate / to_rate;
        Self {
            from_rate,
            to_rate,
            ratio,
            phase: 0.0,
            last_l: 0.0,
            last_r: 0.0,
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

        let mut i = 0;
        while i < num_frames {
            while self.phase < 1.0 && i < num_frames {
                let curr_l = input[i * 2];
                let curr_r = input[i * 2 + 1];

                let out_l = self.last_l + (curr_l - self.last_l) * self.phase;
                let out_r = self.last_r + (curr_r - self.last_r) * self.phase;

                output.push(out_l);
                output.push(out_r);

                self.phase += self.ratio;
            }

            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.last_l = input[i * 2];
                self.last_r = input[i * 2 + 1];
                i += 1;
            }
        }
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


