use std::sync::mpsc::Sender;

use screencapturekit::cm::CMSampleBufferExt;
use screencapturekit::prelude::*;

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
    pub fn start(sender: Sender<Vec<f32>>, sample_rate: u32) -> Result<Self, String> {
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
            .with_sample_rate(sample_rate as i32)
            .with_channel_count(2)
            .with_excludes_current_process_audio(true);

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
                    eprintln!(
                        "Unsupported ScreenCaptureKit audio format: subtype={}, bits={:?}",
                        format.media_subtype_string(),
                        format.audio_bits_per_channel()
                    );
                    return;
                }

                let Some(buffers) = sample.audio_buffer_list() else {
                    return;
                };
                let pcm = copy_stereo_f32(&buffers);
                if !pcm.is_empty() {
                    let _ = sender.send(pcm);
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
        let _ = self.stream.stop_capture();
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
