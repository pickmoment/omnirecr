use std::sync::Arc;

use crate::audio::engine::{CaptureRing, FatalReporter};

use screencapturekit::cm::CMSampleBufferExt;

use screencapturekit::prelude::*;

pub const SYSTEM_AUDIO_SAMPLE_RATE_HZ: u32 = 48_000;

pub fn ensure_screen_capture_permission() -> Result<(), String> {
    let access = core_graphics::access::ScreenCaptureAccess;
    if access.preflight() || access.request() {
        return Ok(());
    }

    Err(
        "Screen & System Audio Recording permission is required. Enable OmniRec in System Settings > Privacy & Security, then quit and reopen the app."
            .to_string(),
    )
}

pub struct MacSystemAudioCapture {
    stream: SCStream,
}

impl MacSystemAudioCapture {
    /// `include_own_app_audio` 가 true 면 OmniRec 자신이 내는 소리도 캡처한다.
    ///
    /// 기본값(false)은 화면 녹화 중 앱 자체 소리가 되먹임되는 것을 막기 위한 것이지만,
    /// 앱 안의 웹뷰(Typecast 창)가 내는 TTS 낭독을 녹음하려면 반드시 포함시켜야 한다.
    /// 이 값이 false 인 채로 녹음하면 스피커로는 소리가 나는데 파일은 무음이 된다.
    ///
    /// `ring` 은 **유계** 링버퍼다. 예전에는 무한 `mpsc::Sender` 였고, FFmpeg 파이프가
    /// 막히면 이 콜백이 밀어 넣는 프레임이 그대로 쌓여 메모리를 무한히 먹었다.
    /// `reporter` 는 되돌릴 수 없는 실패(지원하지 않는 오디오 포맷)를 세션당 한 번
    /// 올린다 — 예전에는 eprintln 만 하고 무음 파일을 정상 결과처럼 내놨다.
    pub fn start(
        ring: Arc<CaptureRing>,
        include_own_app_audio: bool,
        reporter: Arc<FatalReporter>,
    ) -> Result<Self, String> {
        ensure_screen_capture_permission()?;
        let content = SCShareableContent::get().map_err(|error| {
            format!("Unable to access macOS screen and system audio content: {error}")
        })?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| "No display is available for system audio capture.".to_string())?;

        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();
        let configuration = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_captures_audio(true)
            .with_sample_rate(SYSTEM_AUDIO_SAMPLE_RATE_HZ as i32)
            .with_channel_count(2)
            .with_excludes_current_process_audio(!include_own_app_audio);

        let mut stream = SCStream::new(&filter, &configuration);
        let handler_id = stream.add_output_handler(
            move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
                if output_type != SCStreamOutputType::Audio {
                    return;
                }

                let Some(format) = sample.format_description() else {
                    return;
                };
                if !format.audio_is_float()
                    || format.audio_bits_per_channel() != Some(32)
                    || format.audio_is_big_endian()
                {
                    // 포맷이 안 맞으면 이 스트림에서는 영원히 안 맞는다 →
                    // 조용히 버리면 시스템 소리가 통째로 빠진 파일이 나온다.
                    reporter.report(format!(
                        "macOS 시스템 오디오 포맷을 해석할 수 없습니다(subtype={}, bits={:?}). 시스템 소리가 녹음되지 않습니다.",
                        format.media_subtype_string(),
                        format.audio_bits_per_channel()
                    ));
                    return;
                }

                let Some(buffers) = sample.audio_buffer_list() else {
                    return;
                };
                let pcm = copy_stereo_f32(&buffers);
                if !pcm.is_empty() {
                    ring.push(pcm);
                }
            },
            SCStreamOutputType::Audio,
        );

        if handler_id.is_none() {
            return Err("Failed to register the macOS system audio handler.".to_string());
        }

        stream
            .start_capture()
            .map_err(|error| format!("Failed to start macOS system audio capture: {error}"))?;

        Ok(Self { stream })
    }
}

impl Drop for MacSystemAudioCapture {
    fn drop(&mut self) {
        // 실패해도 되돌릴 방법이 없지만, 캡처가 안 멈추면 다음 녹음이 이상하게
        // 동작하므로 흔적은 남긴다.
        if let Err(error) = self.stream.stop_capture() {
            log::warn!("macOS 시스템 오디오 캡처를 멈추지 못했습니다: {error}");
        }
    }
}

fn copy_stereo_f32(buffers: &screencapturekit::cm::AudioBufferList) -> Vec<f32> {
    let Some(first) = buffers.get(0) else {
        return Vec::new();
    };

    if buffers.num_buffers() == 1 {
        if first.number_channels == 1 {
            let samples = first.data().chunks_exact(size_of::<f32>());
            let mut stereo = Vec::with_capacity(samples.len() * 2);
            for bytes in samples {
                let sample = f32::from_ne_bytes(bytes.try_into().unwrap());
                stereo.push(sample);
                stereo.push(sample);
            }
            return stereo;
        }
        return bytes_to_f32(first.data());
    }

    let Some(second) = buffers.get(1) else {
        return Vec::new();
    };
    let left = first.data();
    let right = second.data();
    let frames = (left.len() / size_of::<f32>()).min(right.len() / size_of::<f32>());
    let mut stereo = Vec::with_capacity(frames * 2);

    for frame in 0..frames {
        let offset = frame * size_of::<f32>();
        stereo.push(f32::from_ne_bytes(
            left[offset..offset + size_of::<f32>()].try_into().unwrap(),
        ));
        stereo.push(f32::from_ne_bytes(
            right[offset..offset + size_of::<f32>()].try_into().unwrap(),
        ));
    }

    stereo
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(size_of::<f32>())
        .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::bytes_to_f32;

    #[test]
    fn decodes_native_f32_samples() {
        let expected = [-1.0_f32, -0.25, 0.5, 1.0];
        let bytes: Vec<u8> = expected
            .iter()
            .flat_map(|sample| sample.to_ne_bytes())
            .collect();

        assert_eq!(bytes_to_f32(&bytes), expected);
    }
}
