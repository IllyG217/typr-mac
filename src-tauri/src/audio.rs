use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize)]
pub struct MicDevice {
    pub name: String,
    pub is_default: bool,
}

pub fn list_microphones() -> Vec<MicDevice> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let mut devices = Vec::new();
    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            if let Ok(name) = device.name() {
                devices.push(MicDevice {
                    is_default: name == default_name,
                    name,
                });
            }
        }
    }
    devices
}

/// Wrapper to make cpal::Stream usable across threads.
/// SAFETY: cpal::Stream on macOS (CoreAudio) is thread-safe in practice;
/// we only access it behind a Mutex to start/stop recording.
struct SendStream(#[allow(dead_code)] cpal::Stream);
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

pub struct AudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    stream: Option<SendStream>,
    source_sample_rate: u32,
    source_channels: u16,
    /// Latest RMS amplitude (0–1). Written by the cpal callback; read by the
    /// visualizer polling task in recorder.rs.
    pub latest_level: Arc<Mutex<f32>>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            source_sample_rate: 48000,
            source_channels: 1,
            latest_level: Arc::new(Mutex::new(0.0)),
        }
    }

    pub fn start(&mut self, mic_name: &str) -> Result<(), String> {
        // Clear any leftover samples from previous recording
        self.samples.lock().unwrap().clear();

        let host = cpal::default_host();

        let device = if mic_name == "default" {
            host.default_input_device()
                .ok_or("No default input device found")?
        } else {
            host.input_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.name().map(|n| n == mic_name).unwrap_or(false))
                .ok_or(format!("Microphone '{}' not found", mic_name))?
        };

        // Use the device's default config instead of forcing 16kHz
        let default_config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels();

        println!("[Typr] Mic config: {}Hz, {} channels", sample_rate, channels);

        self.source_sample_rate = sample_rate;
        self.source_channels = channels;

        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let samples = self.samples.clone();
        let latest_level = self.latest_level.clone();
        // Reset level to zero at the start of each recording
        *latest_level.lock().unwrap() = 0.0;

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Compute RMS and store it — do NOT call app.emit() here because
                    // cpal's callback thread has no Tokio runtime. The visualizer task
                    // in recorder.rs polls this value and emits from the right context.
                    if !data.is_empty() {
                        let sum_sq: f32 = data.iter().map(|&s| s * s).sum();
                        let rms = (sum_sq / data.len() as f32).sqrt();
                        *latest_level.lock().unwrap() = rms.clamp(0.0, 1.0);
                    }
                    let mut buf = samples.lock().unwrap();
                    buf.extend_from_slice(data);
                },
                |err| {
                    eprintln!("[Typr] Audio stream error: {}", err);
                },
                None,
            )
            .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(SendStream(stream));
        println!("[Typr] Audio recording started");
        Ok(())
    }

    pub fn stop_and_save(&mut self, output_path: &PathBuf) -> Result<PathBuf, String> {
        self.stream = None; // Drop stops the stream
        println!("[Typr] Audio recording stopped");

        let samples = self.samples.lock().unwrap();
        if samples.is_empty() {
            return Err("No audio captured".to_string());
        }

        println!("[Typr] Captured {} raw samples", samples.len());

        // Convert to mono if multi-channel
        let mono: Vec<f32> = if self.source_channels > 1 {
            samples
                .chunks(self.source_channels as usize)
                .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
                .collect()
        } else {
            samples.clone()
        };

        // Downsample to 16kHz for whisper.cpp
        let resampled = resample(&mono, self.source_sample_rate, 16000);
        println!("[Typr] Resampled to {} samples at 16kHz", resampled.len());

        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = WavWriter::create(output_path, spec).map_err(|e| e.to_string())?;
        for &sample in resampled.iter() {
            let amplitude = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(amplitude).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;

        drop(samples);
        self.samples.lock().unwrap().clear();

        println!("[Typr] WAV saved to {:?}", output_path);
        Ok(output_path.clone())
    }
}

/// Simple linear interpolation resampler
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < samples.len() {
            samples[idx] as f64 * (1.0 - frac) + samples[idx + 1] as f64 * frac
        } else {
            samples[idx.min(samples.len() - 1)] as f64
        };

        output.push(sample as f32);
    }

    output
}
