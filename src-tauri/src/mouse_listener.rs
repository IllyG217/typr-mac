use std::sync::{Arc, Mutex};
use rdev::{grab, Button, Event, EventType};
use tauri::{AppHandle, Emitter};

pub struct MouseConfig {
    pub push_to_talk_btn: Option<u32>,
    pub hands_free_btn: Option<u32>,
    /// When Some("push-to-talk") or Some("hands-free"), the next side-button
    /// press is captured as the new binding for that action.
    pub capturing: Option<String>,
    /// True while the PTT mouse button is physically held down.
    /// Guards against phantom release events and race conditions.
    ptt_held: bool,
    /// Timestamp (ms since epoch) when the PTT button was pressed.
    /// Used to debounce rapid press/release pairs.
    ptt_pressed_at: Option<u64>,
}

impl MouseConfig {
    pub fn new(ptt: Option<u32>, hf: Option<u32>) -> Self {
        Self {
            push_to_talk_btn: ptt,
            hands_free_btn: hf,
            capturing: None,
            ptt_held: false,
            ptt_pressed_at: None,
        }
    }
}

/// Map an rdev Button to a u32 storage code.
/// Only Unknown (side) buttons are bindable — L/R/Middle return None.
fn btn_code(btn: &Button) -> Option<u32> {
    match btn {
        Button::Unknown(n) => Some(*n as u32),
        _ => None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Spawns the global mouse listener on a dedicated background thread.
/// rdev::grab is blocking and must run outside the Tokio runtime.
/// Using grab (instead of listen) lets us suppress bound button events so
/// they never reach the OS — this prevents side buttons from triggering
/// browser-back, browser-forward, or other default OS actions.
pub fn start(
    app: AppHandle,
    config: Arc<Mutex<MouseConfig>>,
    on_ptt_press:   Arc<dyn Fn() + Send + Sync + 'static>,
    on_ptt_release: Arc<dyn Fn() + Send + Sync + 'static>,
    on_hf_press:    Arc<dyn Fn() + Send + Sync + 'static>,
    on_captured:    Arc<dyn Fn(String, u32) + Send + Sync + 'static>,
) {
    std::thread::spawn(move || {
        if let Err(e) = grab(move |event: Event| -> Option<Event> {
            handle_event(
                &event, &app, &config,
                &on_ptt_press, &on_ptt_release, &on_hf_press, &on_captured,
            )
        }) {
            log::error!("Mouse grab error: {:?}", e);
        }
    });
}

/// Minimum hold duration (ms) before a PTT press is treated as intentional.
/// Filters out phantom click events some mice/drivers generate.
const PTT_DEBOUNCE_MS: u64 = 80;

/// Returns None to suppress the event (OS never sees it),
/// or Some(event) to pass it through unchanged.
fn handle_event(
    event: &Event,
    app: &AppHandle,
    config: &Arc<Mutex<MouseConfig>>,
    on_ptt_press:   &Arc<dyn Fn() + Send + Sync + 'static>,
    on_ptt_release: &Arc<dyn Fn() + Send + Sync + 'static>,
    on_hf_press:    &Arc<dyn Fn() + Send + Sync + 'static>,
    on_captured:    &Arc<dyn Fn(String, u32) + Send + Sync + 'static>,
) -> Option<Event> {
    match &event.event_type {
        EventType::ButtonPress(btn) => {
            let code = match btn_code(btn) {
                Some(c) => c,
                None => return Some(event.clone()), // L/R/Middle — pass through
            };
            let mut cfg = config.lock().unwrap();

            // ── Capture mode ─────────────────────────────────────────────────
            if let Some(action) = cfg.capturing.take() {
                match action.as_str() {
                    "push-to-talk" => cfg.push_to_talk_btn = Some(code),
                    "hands-free"   => cfg.hands_free_btn   = Some(code),
                    _ => {}
                }
                drop(cfg);
                on_captured(action.clone(), code);
                let _ = app.emit("mouse-shortcut-set", serde_json::json!({
                    "action": action,
                    "button": code,
                }));
                // Suppress the binding click so it doesn't also trigger an action
                return None;
            }

            // ── Normal operation ──────────────────────────────────────────────
            let ptt = cfg.push_to_talk_btn;
            let hf  = cfg.hands_free_btn;

            if Some(code) == ptt {
                if !cfg.ptt_held {
                    cfg.ptt_held = true;
                    cfg.ptt_pressed_at = Some(now_ms());
                    drop(cfg);
                    log::debug!("Mouse PTT press (code {})", code);
                    on_ptt_press();
                } else {
                    drop(cfg);
                }
                // Suppress — prevent OS back/forward navigation
                None
            } else if Some(code) == hf {
                drop(cfg);
                on_hf_press();
                // Suppress — prevent OS back/forward navigation
                None
            } else {
                // Unbound side button — pass through normally
                Some(event.clone())
            }
        }

        EventType::ButtonRelease(btn) => {
            let code = match btn_code(btn) {
                Some(c) => c,
                None => return Some(event.clone()), // L/R/Middle — pass through
            };
            let mut cfg = config.lock().unwrap();
            let ptt = cfg.push_to_talk_btn;
            let hf  = cfg.hands_free_btn;

            if Some(code) == ptt {
                if cfg.ptt_held {
                    let held_ms = cfg.ptt_pressed_at
                        .map(|t| now_ms().saturating_sub(t))
                        .unwrap_or(0);

                    if held_ms >= PTT_DEBOUNCE_MS {
                        // Real release — stop recording
                        cfg.ptt_held = false;
                        cfg.ptt_pressed_at = None;
                        drop(cfg);
                        log::debug!("Mouse PTT release after {}ms — stopping", held_ms);
                        on_ptt_release();
                    } else {
                        // Phantom quick-release from driver/hardware — ignore it.
                        // Keep ptt_held = true so recording stays open.
                        // Reset pressed_at so the NEXT real release is measured from now.
                        cfg.ptt_pressed_at = Some(now_ms());
                        drop(cfg);
                        log::debug!("Mouse PTT phantom release ({}ms) — ignoring, recording continues", held_ms);
                    }
                } else {
                    drop(cfg);
                }
                // Suppress PTT release as well
                None
            } else if Some(code) == hf {
                drop(cfg);
                // Suppress HF release to prevent OS navigation
                None
            } else {
                // Unbound side button — pass through
                Some(event.clone())
            }
        }

        _ => Some(event.clone()),
    }
}
