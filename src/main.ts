import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface Settings {
  microphone: string;
  engine: string;
  whisperModel: string;
  recordingMode: string;
  hotkey: string;
  pushToTalkMouseBtn: number | null;
  handsFreeMouseBtn: number | null;
}

interface MicDevice {
  name: string;
  is_default: boolean;
}

interface DownloadProgress {
  downloaded: number;
  total: number;
  percent: number;
}

// DOM elements
const statusDot = document.getElementById("status-dot")!;
const statusText = document.getElementById("status-text")!;
const micSelect = document.getElementById("mic-select") as HTMLSelectElement;
const engineLocal = document.getElementById("engine-local")!;
const engineCloud = document.getElementById("engine-cloud")!;
const localSettings = document.getElementById("local-settings")!;
const cloudSettings = document.getElementById("cloud-settings")!;
const modelSelect = document.getElementById("model-select") as HTMLSelectElement;
const downloadBtn = document.getElementById("download-btn")!;
const downloadProgress = document.getElementById("download-progress")!;
const progressFill = document.getElementById("progress-fill")!;
const groqKey = document.getElementById("groq-key") as HTMLInputElement;
const modeToggle = document.getElementById("mode-toggle")!;
const modePtt = document.getElementById("mode-ptt")!;
const hotkeyText = document.getElementById("hotkey-text")!;
const pttMouseText = document.getElementById("ptt-mouse-text")!;
const pttMouseBind = document.getElementById("ptt-mouse-bind")!;
const pttMouseClear = document.getElementById("ptt-mouse-clear")!;
const hfMouseText = document.getElementById("hf-mouse-text")!;
const hfMouseBind = document.getElementById("hf-mouse-bind")!;
const hfMouseClear = document.getElementById("hf-mouse-clear")!;

// ── Mouse button label helper ────────────────────────────────────────────────
// rdev only surfaces side/extra buttons as Unknown(n) — Left/Right/Middle are
// filtered out entirely. Windows maps side buttons as:
//   Unknown(1) = X1 / Back   (physical button 4 on most 6-button mice)
//   Unknown(2) = X2 / Forward (physical button 5 on most 6-button mice)
//   Unknown(3+) = additional gaming mouse buttons
function mouseButtonLabel(code: number): string {
  switch (code) {
    case 1: return "Back Button (4)";
    case 2: return "Forward Button (5)";
    default: return `Side Button ${code + 3}`;
  }
}

// Section navigation
const navItems = document.querySelectorAll(".nav-item");
const sections = document.querySelectorAll(".content-section");

navItems.forEach((item) => {
  item.addEventListener("click", () => {
    const target = item.getAttribute("data-section");
    navItems.forEach((n) => n.classList.remove("active"));
    sections.forEach((s) => s.classList.remove("active"));
    item.classList.add("active");
    document.getElementById(`section-${target}`)?.classList.add("active");
  });
});

// Window drag — titlebar and sidebar empty space
const titlebar = document.getElementById("titlebar")!;
const sidebar = document.getElementById("sidebar")!;
const appWindow = getCurrentWindow();

titlebar.addEventListener("mousedown", (e) => {
  if ((e.target as HTMLElement).closest("button, select, input, a, .nav-item")) return;
  appWindow.startDragging();
});

sidebar.addEventListener("mousedown", (e) => {
  if ((e.target as HTMLElement).closest("button, select, input, a, .nav-item")) return;
  appWindow.startDragging();
});

let currentSettings: Settings;

async function loadSettings() {
  currentSettings = await invoke<Settings>("get_settings");

  // Populate mic dropdown
  const mics = await invoke<MicDevice[]>("list_microphones");
  micSelect.innerHTML = "";
  mics.forEach((mic) => {
    const option = document.createElement("option");
    option.value = mic.name;
    option.textContent = mic.name + (mic.is_default ? " (default)" : "");
    micSelect.appendChild(option);
  });
  micSelect.value = currentSettings.microphone;

  // Engine
  setEngine(currentSettings.engine);

  // Model
  modelSelect.value = currentSettings.whisperModel;
  await checkModelStatus();

  // Groq key — loaded from OS keychain, not settings JSON
  try {
    groqKey.value = await invoke<string>("get_api_key");
  } catch {
    groqKey.value = "";
  }

  // Recording mode
  setRecordingMode(currentSettings.recordingMode);

  // Hotkey
  hotkeyText.textContent = currentSettings.hotkey.replace("CmdOrCtrl", "Ctrl");

  // Mouse shortcuts
  pttMouseText.textContent = currentSettings.pushToTalkMouseBtn != null
    ? mouseButtonLabel(currentSettings.pushToTalkMouseBtn) : "None";
  hfMouseText.textContent = currentSettings.handsFreeMouseBtn != null
    ? mouseButtonLabel(currentSettings.handsFreeMouseBtn) : "None";
}

function setEngine(engine: string) {
  currentSettings.engine = engine;
  engineLocal.classList.toggle("active", engine === "local");
  engineCloud.classList.toggle("active", engine === "cloud");
  localSettings.classList.toggle("hidden", engine !== "local");
  cloudSettings.classList.toggle("hidden", engine !== "cloud");
}

function setRecordingMode(mode: string) {
  currentSettings.recordingMode = mode;
  modeToggle.classList.toggle("active", mode === "toggle");
  modePtt.classList.toggle("active", mode === "push-to-talk");
}

async function checkModelStatus() {
  const downloaded = await invoke<boolean>("check_model_downloaded", {
    modelSize: modelSelect.value,
  });
  downloadBtn.textContent = downloaded ? "\u2713" : "Download";
  (downloadBtn as HTMLButtonElement).disabled = downloaded;
}

async function saveSettings() {
  currentSettings.microphone = micSelect.value;
  currentSettings.whisperModel = modelSelect.value;
  await invoke("save_settings", { settings: currentSettings });
}

async function saveApiKey() {
  try {
    await invoke("set_api_key", { key: groqKey.value });
  } catch (e) {
    showToast(`Failed to save API key: ${e}`);
  }
}

// Event listeners
engineLocal.addEventListener("click", () => {
  setEngine("local");
  saveSettings();
});

engineCloud.addEventListener("click", () => {
  setEngine("cloud");
  saveSettings();
});

micSelect.addEventListener("change", () => saveSettings());

modelSelect.addEventListener("change", async () => {
  await checkModelStatus();
  saveSettings();
});

downloadBtn.addEventListener("click", async () => {
  (downloadBtn as HTMLButtonElement).disabled = true;
  downloadProgress.classList.remove("hidden");
  progressFill.style.width = "0%";

  try {
    await invoke("download_model", { modelSize: modelSelect.value });
    downloadBtn.textContent = "\u2713";
  } catch (e) {
    downloadBtn.textContent = "Retry";
    (downloadBtn as HTMLButtonElement).disabled = false;
    showToast(`Download failed: ${e}`);
  }
  downloadProgress.classList.add("hidden");
});

groqKey.addEventListener("change", () => saveApiKey());

modeToggle.addEventListener("click", () => {
  setRecordingMode("toggle");
  saveSettings();
});

modePtt.addEventListener("click", () => {
  setRecordingMode("push-to-talk");
  saveSettings();
});

// Listen for recording state changes
listen<string>("recording-state", (event) => {
  const state = event.payload;
  statusDot.className = "";
  if (state === "Recording") {
    statusDot.classList.add("recording");
    statusText.textContent = "Recording...";
  } else if (state === "Transcribing") {
    statusDot.classList.add("transcribing");
    statusText.textContent = "Transcribing...";
  } else {
    statusDot.classList.add("ready");
    statusText.textContent = "Ready";
  }
});

// Listen for download progress
listen<DownloadProgress>("download-progress", (event) => {
  const { percent } = event.payload;
  progressFill.style.width = `${percent}%`;
});

// ── Hotkey rebinding ─────────────────────────────────────────────────────────

let hotkeyListening = false;
let hotkeyKeydownHandler: ((e: KeyboardEvent) => void) | null = null;

hotkeyText.addEventListener("click", () => {
  if (hotkeyListening) return;
  hotkeyListening = true;
  hotkeyText.textContent = "Press keys…";
  hotkeyText.classList.add("listening");

  hotkeyKeydownHandler = async (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();

    // Ignore lone modifier key presses
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

    // Build the Tauri shortcut string
    const parts: string[] = [];
    if (e.ctrlKey || e.metaKey) parts.push("CmdOrCtrl");
    if (e.shiftKey) parts.push("Shift");
    if (e.altKey) parts.push("Alt");
    parts.push(e.key.length === 1 ? e.key.toUpperCase() : e.key);
    const shortcut = parts.join("+");

    // Stop listening
    hotkeyListening = false;
    hotkeyText.classList.remove("listening");
    document.removeEventListener("keydown", hotkeyKeydownHandler!, true);
    hotkeyKeydownHandler = null;

    try {
      await invoke("set_hotkey", { shortcut });
      hotkeyText.textContent = shortcut.replace("CmdOrCtrl", "Ctrl");
    } catch (err) {
      // Restore old display on failure
      hotkeyText.textContent = currentSettings.hotkey.replace("CmdOrCtrl", "Ctrl");
      showToast(`Could not set hotkey: ${err}`);
    }
  };

  document.addEventListener("keydown", hotkeyKeydownHandler, true);
});

// ── Toast notifications ──────────────────────────────────────────────────────

const toastContainer = document.getElementById("toast-container")!;
const MAX_TOASTS = 3;

function showToast(msg: string) {
  // Trim oldest if at limit
  while (toastContainer.children.length >= MAX_TOASTS) {
    toastContainer.firstElementChild?.remove();
  }

  const toast = document.createElement("div");
  toast.className = "toast";
  toast.innerHTML = `
    <svg class="toast-icon" viewBox="0 0 14 14" fill="none" xmlns="http://www.w3.org/2000/svg">
      <circle cx="7" cy="7" r="6" stroke="currentColor" stroke-width="1.2"/>
      <path d="M7 4v3.5M7 9.5v.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
    </svg>
    <span class="toast-msg">${msg}</span>
  `;
  toastContainer.appendChild(toast);

  setTimeout(() => {
    toast.classList.add("toast-out");
    toast.addEventListener("animationend", () => toast.remove(), { once: true });
  }, 4000);
}

listen<string>("typr-error", (event) => showToast(event.payload));

// ── Mouse shortcut binding ───────────────────────────────────────────────────

pttMouseBind.addEventListener("click", async () => {
  pttMouseText.textContent = "Click a mouse button…";
  pttMouseText.classList.add("listening");
  try {
    await invoke("start_mouse_capture", { action: "push-to-talk" });
  } catch (e) {
    pttMouseText.textContent = currentSettings.pushToTalkMouseBtn != null
      ? `Mouse ${currentSettings.pushToTalkMouseBtn}` : "None";
    pttMouseText.classList.remove("listening");
    showToast(`Failed to start capture: ${e}`);
  }
});

hfMouseBind.addEventListener("click", async () => {
  hfMouseText.textContent = "Click a mouse button…";
  hfMouseText.classList.add("listening");
  try {
    await invoke("start_mouse_capture", { action: "hands-free" });
  } catch (e) {
    hfMouseText.textContent = currentSettings.handsFreeMouseBtn != null
      ? `Mouse ${currentSettings.handsFreeMouseBtn}` : "None";
    hfMouseText.classList.remove("listening");
    showToast(`Failed to start capture: ${e}`);
  }
});

pttMouseClear.addEventListener("click", async () => {
  try {
    await invoke("clear_mouse_shortcut", { action: "push-to-talk" });
    currentSettings.pushToTalkMouseBtn = null;
    pttMouseText.textContent = "None";
  } catch (e) {
    showToast(`Failed to clear: ${e}`);
  }
});

hfMouseClear.addEventListener("click", async () => {
  try {
    await invoke("clear_mouse_shortcut", { action: "hands-free" });
    currentSettings.handsFreeMouseBtn = null;
    hfMouseText.textContent = "None";
  } catch (e) {
    showToast(`Failed to clear: ${e}`);
  }
});

listen<{ action: string; button: number }>("mouse-shortcut-set", (event) => {
  const { action, button } = event.payload;
  const label = mouseButtonLabel(button);
  if (action === "push-to-talk") {
    pttMouseText.textContent = label;
    pttMouseText.classList.remove("listening");
    currentSettings.pushToTalkMouseBtn = button;
  } else {
    hfMouseText.textContent = label;
    hfMouseText.classList.remove("listening");
    currentSettings.handsFreeMouseBtn = button;
  }
});

// Initialize
loadSettings();
