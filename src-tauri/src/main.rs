// windows_subsystem = "windows" is a Windows-only attribute — not used on macOS

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use simplelog::{CombinedLogger, Config, LevelFilter, SimpleLogger, WriteLogger};
use std::fs::File;

use keyring::Entry as KeyringEntry;
use typr_lib::audio;
use typr_lib::downloader;
use typr_lib::mouse_listener::{self, MouseConfig};
use typr_lib::recorder::{Recorder, RecordingState};
use typr_lib::settings::Settings;
use typr_lib::transcribe_local;

const KEYRING_SERVICE: &str = "typr";
const KEYRING_KEY: &str = "groq-api-key";

pub struct AppState {
    recorder: Recorder,
    settings: Mutex<Settings>,
    app_dir: PathBuf,
    mouse_config: std::sync::Arc<Mutex<MouseConfig>>,
}

fn get_app_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.typr.app")
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: Settings) -> Result<(), String> {
    settings.save(&state.app_dir)?;
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
fn set_hotkey(app: tauri::AppHandle, state: State<AppState>, shortcut: String) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Validate the new shortcut string before committing
    let _parsed: tauri_plugin_global_shortcut::Shortcut = shortcut
        .parse()
        .map_err(|_| format!("Invalid shortcut: {}", shortcut))?;

    // Unregister all existing shortcuts then re-register with new combo
    app.global_shortcut().unregister_all().map_err(|e| e.to_string())?;

    let handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut.as_str(), move |_app, sc, event| {
            log::debug!("Hotkey event: {:?} state={:?}", sc, event.state);
            let handle = handle.clone();
            let st = handle.state::<AppState>();
            let mode = st.settings.lock().unwrap().recording_mode.clone();
            match event.state {
                ShortcutState::Pressed => {
                    tauri::async_runtime::spawn(async move {
                        let st = handle.state::<AppState>();
                        match mode.as_str() {
                            "toggle" => {
                                match do_toggle_recording(&handle, st.inner()).await {
                                    Ok(r) => log::info!("Toggle: {}", r),
                                    Err(e) => log::error!("Toggle error: {}", e),
                                }
                            }
                            "push-to-talk" => {
                                if st.recorder.get_state() == RecordingState::Ready {
                                    let mic = st.settings.lock().unwrap().microphone.clone();
                                    let _ = st.recorder.start_recording(&handle, &mic);
                                }
                            }
                            _ => {}
                        }
                    });
                }
                ShortcutState::Released => {
                    if mode == "push-to-talk" {
                        tauri::async_runtime::spawn(async move {
                            let st = handle.state::<AppState>();
                            if st.recorder.get_state() == RecordingState::Recording {
                                let settings = st.settings.lock().unwrap().clone();
                                let api_key = get_api_key().unwrap_or_default();
                                let _ = st.recorder.stop_and_transcribe(&handle, &settings, &st.app_dir, &api_key).await;
                            }
                        });
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;

    // Persist the new hotkey
    let mut settings = state.settings.lock().unwrap();
    settings.hotkey = shortcut.clone();
    settings.save(&state.app_dir).map_err(|e| e.to_string())?;
    log::info!("Hotkey changed to: {}", shortcut);
    Ok(())
}

#[tauri::command]
fn get_api_key() -> Result<String, String> {
    let entry = KeyringEntry::new(KEYRING_SERVICE, KEYRING_KEY).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(key) => Ok(key),
        Err(keyring::Error::NoEntry) => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn set_api_key(key: String) -> Result<(), String> {
    let entry = KeyringEntry::new(KEYRING_SERVICE, KEYRING_KEY).map_err(|e| e.to_string())?;
    if key.is_empty() {
        let _ = entry.delete_credential();
    } else {
        entry.set_password(&key).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn list_microphones() -> Vec<audio::MicDevice> {
    audio::list_microphones()
}

#[tauri::command]
fn get_recording_state(state: State<AppState>) -> RecordingState {
    state.recorder.get_state()
}

#[tauri::command]
fn check_model_downloaded(state: State<AppState>, model_size: String) -> bool {
    let model_file = transcribe_local::model_filename(&model_size);
    state.app_dir.join(&model_file).exists()
}

#[tauri::command]
async fn download_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model_size: String,
) -> Result<(), String> {
    let url = transcribe_local::model_download_url(&model_size);
    let model_file = transcribe_local::model_filename(&model_size);
    let dest = state.app_dir.join(&model_file);
    downloader::download_model(app, &url, &dest).await
}

#[tauri::command]
async fn toggle_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    do_toggle_recording(&app, &state).await
}

/// Shared logic for toggle recording, used by both the Tauri command and hotkey/tray handlers.
async fn do_toggle_recording(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<String, String> {
    let current_state = state.recorder.get_state();
    match current_state {
        RecordingState::Ready => {
            let mic = state.settings.lock().unwrap().microphone.clone();
            state.recorder.start_recording(app, &mic)?;
            Ok("recording".to_string())
        }
        RecordingState::Recording => {
            let settings = state.settings.lock().unwrap().clone();
            let api_key = get_api_key().unwrap_or_default();
            let result = state
                .recorder
                .stop_and_transcribe(app, &settings, &state.app_dir, &api_key)
                .await?;
            Ok(result)
        }
        RecordingState::Transcribing => {
            let _ = app.emit("typr-error", "Still transcribing — please wait");
            Err("Currently transcribing, please wait".to_string())
        }
    }
}

#[tauri::command]
fn start_mouse_capture(state: State<AppState>, action: String) -> Result<(), String> {
    state.mouse_config.lock().unwrap().capturing = Some(action);
    Ok(())
}

#[tauri::command]
fn clear_mouse_shortcut(state: State<AppState>, action: String) -> Result<(), String> {
    let mut cfg = state.mouse_config.lock().unwrap();
    let mut settings = state.settings.lock().unwrap();
    match action.as_str() {
        "push-to-talk" => { cfg.push_to_talk_btn = None; settings.push_to_talk_mouse_btn = None; }
        "hands-free"   => { cfg.hands_free_btn   = None; settings.hands_free_mouse_btn   = None; }
        _ => {}
    }
    settings.save(&state.app_dir).map_err(|e| e.to_string())
}

/// Decode a PNG byte slice into a Tauri Image.
fn decode_png_icon(data: &[u8]) -> tauri::image::Image<'static> {
    use image::GenericImageView;
    let img = image::load_from_memory(data).expect("Failed to decode tray icon PNG");
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8().into_raw();
    tauri::image::Image::new_owned(rgba, width, height)
}

fn main() {
    let app_dir = get_app_dir();
    // Ensure the config directory exists before anything tries to write to it
    if let Err(e) = std::fs::create_dir_all(&app_dir) {
        eprintln!("[Typr] Warning: could not create app dir {:?}: {}", app_dir, e);
        // Logger not yet initialised; eprintln is the only option here
    }

    // Initialise logging: file always, stderr only in debug builds
    let log_path = app_dir.join("typr.log");
    let mut loggers: Vec<Box<dyn simplelog::SharedLogger>> = vec![
        WriteLogger::new(
            LevelFilter::Info,
            Config::default(),
            File::options().create(true).append(true).open(&log_path)
                .expect("Failed to open log file"),
        ),
    ];
    #[cfg(debug_assertions)]
    loggers.push(SimpleLogger::new(LevelFilter::Debug, Config::default()));
    let _ = CombinedLogger::init(loggers);

    let settings = Settings::load(&app_dir);
    let initial_hotkey = settings.hotkey.clone();
    let mouse_config = std::sync::Arc::new(Mutex::new(MouseConfig::new(
        settings.push_to_talk_mouse_btn,
        settings.hands_free_mouse_btn,
    )));

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            recorder: Recorder::new(),
            settings: Mutex::new(settings),
            app_dir,
            mouse_config,
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            get_api_key,
            set_api_key,
            set_hotkey,
            list_microphones,
            get_recording_state,
            check_model_downloaded,
            download_model,
            toggle_recording,
            start_mouse_capture,
            clear_mouse_shortcut,
        ])
        .setup(move |app| {
            // ── System Tray ──────────────────────────────────────────────────
            let idle_icon = decode_png_icon(include_bytes!("../icons/tray-idle.png"));

            let settings_item =
                MenuItem::with_id(app, "settings", "Open Settings", true, None::<&str>)?;
            let quit_item =
                MenuItem::with_id(app, "quit", "Quit Typr", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &quit_item])?;

            TrayIconBuilder::with_id("main-tray")
                .icon(idle_icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Typr — Ready")
                .on_tray_icon_event(|tray, event| {
                    // Left-click toggles recording
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle().clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app.state::<AppState>();
                            match do_toggle_recording(&app, state.inner()).await {
                                Ok(result) => log::info!("Tray toggle: {}", result),
                                Err(e) => log::error!("Tray toggle error: {}", e),
                            }
                        });
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => {
                        log::info!("Quit requested from tray menu");
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            log::info!("System tray created");

            // ── Global Hotkey ────────────────────────────────────────────────
            let handle = app.handle().clone();
            log::info!("Registering global shortcut: {}", initial_hotkey);

            match app.global_shortcut().on_shortcut(
                initial_hotkey.as_str(),
                move |_app, shortcut, event| {
                    log::debug!("Hotkey event: {:?} state={:?}", shortcut, event.state);
                    let handle = handle.clone();
                    let state = handle.state::<AppState>();
                    let mode = state.settings.lock().unwrap().recording_mode.clone();
                    log::debug!("Recording mode: {}", mode);

                    match event.state {
                        ShortcutState::Pressed => {
                            tauri::async_runtime::spawn(async move {
                                let state = handle.state::<AppState>();
                                match mode.as_str() {
                                    "toggle" => {
                                        match do_toggle_recording(&handle, state.inner()).await {
                                            Ok(result) => log::info!("Toggle result: {}", result),
                                            Err(e) => log::error!("Toggle error: {}", e),
                                        }
                                    }
                                    "push-to-talk" => {
                                        let current = state.recorder.get_state();
                                        if current == RecordingState::Ready {
                                            let mic = state
                                                .settings
                                                .lock()
                                                .unwrap()
                                                .microphone
                                                .clone();
                                            match state.recorder.start_recording(&handle, &mic) {
                                                Ok(_) => log::info!("Recording started"),
                                                Err(e) => log::error!("Start recording error: {}", e),
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            });
                        }
                        ShortcutState::Released => {
                            if mode == "push-to-talk" {
                                tauri::async_runtime::spawn(async move {
                                    let state = handle.state::<AppState>();
                                    let current = state.recorder.get_state();
                                    if current == RecordingState::Recording {
                                        let settings =
                                            state.settings.lock().unwrap().clone();
                                        let api_key = get_api_key().unwrap_or_default();
                                        match state
                                            .recorder
                                            .stop_and_transcribe(
                                                &handle,
                                                &settings,
                                                &state.app_dir,
                                                &api_key,
                                            )
                                            .await
                                        {
                                            Ok(result) => log::info!("Transcription done: {} chars", result.len()),
                                            Err(e) => log::error!("Transcription error: {}", e),
                                        }
                                    }
                                });
                            }
                        }
                    }
                },
            ) {
                Ok(_) => log::info!("Global shortcut registered: {}", initial_hotkey),
                Err(e) => log::error!("Failed to register global shortcut: {}", e),
            }

            // ── Global Mouse Listener ────────────────────────────────────────
            let mouse_cfg = std::sync::Arc::clone(&app.state::<AppState>().mouse_config);
            let h1 = app.handle().clone();
            let h2 = app.handle().clone();
            let h3 = app.handle().clone();
            let h4 = app.handle().clone();

            mouse_listener::start(
                app.handle().clone(),
                mouse_cfg,
                // on_ptt_press — start recording
                std::sync::Arc::new(move || {
                    let h = h1.clone();
                    tauri::async_runtime::spawn(async move {
                        let st = h.state::<AppState>();
                        if st.recorder.get_state() == RecordingState::Ready {
                            let mic = st.settings.lock().unwrap().microphone.clone();
                            if let Err(e) = st.recorder.start_recording(&h, &mic) {
                                log::error!("Mouse PTT start: {}", e);
                            }
                        }
                    });
                }),
                // on_ptt_release — stop and transcribe
                std::sync::Arc::new(move || {
                    let h = h2.clone();
                    tauri::async_runtime::spawn(async move {
                        let st = h.state::<AppState>();
                        if st.recorder.get_state() == RecordingState::Recording {
                            let settings = st.settings.lock().unwrap().clone();
                            let api_key = get_api_key().unwrap_or_default();
                            if let Err(e) = st.recorder
                                .stop_and_transcribe(&h, &settings, &st.app_dir, &api_key)
                                .await
                            {
                                log::error!("Mouse PTT stop: {}", e);
                            }
                        }
                    });
                }),
                // on_hf_press — toggle recording
                std::sync::Arc::new(move || {
                    let h = h3.clone();
                    tauri::async_runtime::spawn(async move {
                        let st = h.state::<AppState>();
                        if let Err(e) = do_toggle_recording(&h, st.inner()).await {
                            log::error!("Mouse HF toggle: {}", e);
                        }
                    });
                }),
                // on_captured — persist updated button to settings
                std::sync::Arc::new(move |action: String, button: u32| {
                    let st = h4.state::<AppState>();
                    let mut settings = st.settings.lock().unwrap();
                    match action.as_str() {
                        "push-to-talk" => settings.push_to_talk_mouse_btn = Some(button),
                        "hands-free"   => settings.hands_free_mouse_btn   = Some(button),
                        _ => {}
                    }
                    if let Err(e) = settings.save(&st.app_dir) {
                        log::error!("Mouse capture save error: {}", e);
                    }
                }),
            );
            log::info!("Mouse listener started");

            Ok(())
        })
        // ── Settings window: hide on close, don't quit the app ────────────
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
