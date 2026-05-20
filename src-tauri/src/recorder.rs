use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioRecorder;
use crate::cleanup::cleanup_text;
use crate::paste::paste_text;
use crate::settings::Settings;
use crate::transcribe_local;
use crate::transcribe_groq;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum RecordingState {
    Ready,
    Recording,
    Transcribing,
}

/// Show the waveform window, positioned at the bottom-centre of the primary monitor.
fn show_waveform(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("waveform") {
        // Position above the taskbar / dock at the horizontal centre of the primary monitor.
        if let Ok(Some(monitor)) = win.primary_monitor() {
            let scale = monitor.scale_factor();
            let screen_w = monitor.size().width as f64 / scale;
            let screen_h = monitor.size().height as f64 / scale;
            let win_w = 320.0_f64;
            let win_h = 90.0_f64;
            let x = (screen_w - win_w) / 2.0;
            let y = screen_h - win_h - 80.0; // 80 px clears the Windows taskbar
            let _ = win.set_position(tauri::LogicalPosition::new(x, y));
        }
        let _ = win.show();
    }
}

/// Hide the waveform window.
fn hide_waveform(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("waveform") {
        let _ = win.hide();
    }
}

/// Decode a PNG byte slice into a Tauri Image (embedded at compile time).
fn decode_png_icon(data: &[u8]) -> tauri::image::Image<'static> {
    use image::GenericImageView;
    let img = image::load_from_memory(data).expect("Failed to decode tray icon PNG");
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8().into_raw();
    tauri::image::Image::new_owned(rgba, width, height)
}

/// Update the system tray icon and tooltip to reflect the current recording state.
fn update_tray(app: &AppHandle, state: &RecordingState) {
    let Some(tray) = app.tray_by_id("main-tray") else { return };

    let icon = match state {
        RecordingState::Ready => {
            decode_png_icon(include_bytes!("../icons/tray-idle.png"))
        }
        RecordingState::Recording => {
            decode_png_icon(include_bytes!("../icons/tray-recording.png"))
        }
        RecordingState::Transcribing => {
            decode_png_icon(include_bytes!("../icons/tray-transcribing.png"))
        }
    };

    let tooltip = match state {
        RecordingState::Ready => "Typr — Ready",
        RecordingState::Recording => "Typr — Recording...",
        RecordingState::Transcribing => "Typr — Transcribing...",
    };

    let _ = tray.set_icon(Some(icon));
    let _ = tray.set_tooltip(Some(tooltip));
}

pub struct Recorder {
    state: Arc<Mutex<RecordingState>>,
    audio_recorder: Arc<Mutex<AudioRecorder>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Ready)),
            audio_recorder: Arc::new(Mutex::new(AudioRecorder::new())),
        }
    }

    pub fn get_state(&self) -> RecordingState {
        self.state.lock().unwrap().clone()
    }

    pub fn start_recording(&self, app: &AppHandle, mic_name: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if *state != RecordingState::Ready {
            return Err("Already recording or transcribing".to_string());
        }

        let mut recorder = self.audio_recorder.lock().unwrap();
        if let Err(e) = recorder.start(mic_name) {
            let msg = format!("Microphone error: {}", e);
            let _ = app.emit("typr-error", &msg);
            return Err(msg);
        }

        // Grab the shared level Arc before releasing the lock
        let level_arc = recorder.latest_level.clone();

        *state = RecordingState::Recording;
        let _ = app.emit("recording-state", RecordingState::Recording);
        update_tray(app, &RecordingState::Recording);
        show_waveform(app);

        // Spawn a Tauri async task that polls the audio level and emits events.
        // This runs in the proper Tokio context, unlike the cpal callback thread.
        let state_arc = self.state.clone();
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                // Stop when no longer recording
                {
                    let s = state_arc.lock().unwrap();
                    if *s != RecordingState::Recording {
                        break;
                    }
                }
                let level = *level_arc.lock().unwrap();
                let _ = app_clone.emit("audio-level", level);
                tokio::time::sleep(tokio::time::Duration::from_millis(33)).await;
            }
        });

        Ok(())
    }

    pub async fn stop_and_transcribe(
        &self,
        app: &AppHandle,
        settings: &Settings,
        app_dir: &PathBuf,
        groq_api_key: &str,
    ) -> Result<String, String> {
        // Stop recording
        {
            let mut state = self.state.lock().unwrap();
            if *state != RecordingState::Recording {
                return Err("Not currently recording".to_string());
            }
            *state = RecordingState::Transcribing;
            let _ = app.emit("recording-state", RecordingState::Transcribing);
            update_tray(app, &RecordingState::Transcribing);
        }

        let temp_path = app_dir.join("temp_recording.wav");

        // Helper to reset state to Ready and optionally surface an error toast.
        let reset = |app: &AppHandle, state_lock: &Arc<Mutex<RecordingState>>, err: Option<&str>| {
            let mut s = state_lock.lock().unwrap();
            *s = RecordingState::Ready;
            let _ = app.emit("recording-state", RecordingState::Ready);
            update_tray(app, &RecordingState::Ready);
            hide_waveform(app);
            if let Some(msg) = err {
                let _ = app.emit("typr-error", msg);
            }
        };

        // Save audio
        {
            let mut recorder = self.audio_recorder.lock().unwrap();
            if let Err(e) = recorder.stop_and_save(&temp_path) {
                let msg = format!("Audio capture error: {}", e);
                reset(app, &self.state, Some(&msg));
                return Err(msg);
            }
        }

        // Transcribe
        let raw_text = match settings.engine.as_str() {
            "local" => {
                let model_path = app_dir.join(transcribe_local::model_filename(&settings.whisper_model));
                match transcribe_local::transcribe_local(app, &model_path, &temp_path).await {
                    Ok(t) => t,
                    Err(e) => {
                        reset(app, &self.state, Some(&e));
                        return Err(e);
                    }
                }
            }
            "cloud" => {
                match transcribe_groq::transcribe_groq(groq_api_key, &temp_path).await {
                    Ok(t) => t,
                    Err(e) => {
                        reset(app, &self.state, Some(&e));
                        return Err(e);
                    }
                }
            }
            _ => {
                let msg = format!("Unknown engine: {}", settings.engine);
                reset(app, &self.state, Some(&msg));
                return Err(msg);
            }
        };

        // Cleanup temp file
        let _ = std::fs::remove_file(&temp_path);

        // Clean up text
        let cleaned = cleanup_text(&raw_text);

        // Auto-paste
        if !cleaned.is_empty() {
            if let Err(e) = paste_text(&cleaned) {
                let msg = format!("Paste failed: {}", e);
                reset(app, &self.state, Some(&msg));
                return Err(msg);
            }
        }

        // Reset state to Ready
        reset(app, &self.state, None);

        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_ready() {
        let recorder = Recorder::new();
        assert_eq!(recorder.get_state(), RecordingState::Ready);
    }
}
