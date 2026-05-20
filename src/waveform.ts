import { listen } from "@tauri-apps/api/event";

// ── Canvas setup ────────────────────────────────────────────────────────────
const canvas = document.getElementById("canvas") as HTMLCanvasElement;
const ctx = canvas.getContext("2d")!;
const W = canvas.width;   // 286
const H = canvas.height;  // 46

const NUM_BARS = 44;
const BAR_W   = 4;
const GAP     = Math.round((W - NUM_BARS * BAR_W) / (NUM_BARS - 1)); // ≈ 2px

// Current audio energy (0–1), updated by audio-level events.
let targetLevel = 0;
// Per-bar smoothed heights, interpolated toward targetLevel each frame.
let smoothedLevels: number[] = new Array(NUM_BARS).fill(0);

// ── State ───────────────────────────────────────────────────────────────────
let isTranscribing = false;
let wavePhase = 0;  // Used for the transcribing sine animation

// ── Gradients (created once) ─────────────────────────────────────────────────
function makeGrad(colorBottom: string, colorTop: string): CanvasGradient {
  const g = ctx.createLinearGradient(0, H, 0, 0);
  g.addColorStop(0, colorBottom);
  g.addColorStop(1, colorTop);
  return g;
}
const gradRecording    = makeGrad("#6d28d9", "#c084fc");
const gradTranscribing = makeGrad("#b45309", "#fcd34d");

// ── Draw one frame ──────────────────────────────────────────────────────────
function draw(levels: number[]) {
  ctx.clearRect(0, 0, W, H);
  const grad = isTranscribing ? gradTranscribing : gradRecording;

  for (let i = 0; i < NUM_BARS; i++) {
    const barH = Math.max(2, levels[i] * H);
    const x    = i * (BAR_W + GAP);
    const y    = H - barH;

    ctx.fillStyle = grad;
    ctx.beginPath();
    // Rounded top corners
    const r = Math.min(BAR_W / 2, barH / 2);
    ctx.moveTo(x + r, y);
    ctx.lineTo(x + BAR_W - r, y);
    ctx.arcTo(x + BAR_W, y, x + BAR_W, y + r, r);
    ctx.lineTo(x + BAR_W, H);
    ctx.lineTo(x, H);
    ctx.arcTo(x, y, x + r, y, r);
    ctx.closePath();
    ctx.fill();
  }
}

// ── Animation loop ───────────────────────────────────────────────────────────
function tick(timestamp: number) {
  requestAnimationFrame(tick);

  if (isTranscribing) {
    // Gentle rolling sine wave — does not depend on audio-level events
    wavePhase += 0.07;
    const levels = Array.from({ length: NUM_BARS }, (_, i) => {
      const wave = Math.sin(wavePhase + i * 0.38) * 0.25 + 0.32;
      return wave;
    });
    draw(levels);
  } else {
    // Decay targetLevel each frame so bars drop when mic goes quiet
    targetLevel *= 0.88;
    // Lerp each bar toward its individual target (targetLevel + wave variation)
    for (let i = 0; i < NUM_BARS; i++) {
      const wave = Math.sin(timestamp * 0.004 + i * 0.45) * 0.25;
      const target = Math.max(0, targetLevel * (0.75 + wave));
      smoothedLevels[i] += (target - smoothedLevels[i]) * 0.25;
    }
    draw(smoothedLevels);
  }
}

requestAnimationFrame(tick);

// ── Receive audio-level events (RMS, 0–1) ────────────────────────────────────
listen<number>("audio-level", (event) => {
  if (!isTranscribing) {
    targetLevel = Math.min(1, Math.sqrt(event.payload) * 2.5);
  }
});

// ── Receive recording-state events ───────────────────────────────────────────
const statusText = document.getElementById("status-text")!;

listen<string>("recording-state", (event) => {
  const state = event.payload;

  if (state === "Transcribing") {
    isTranscribing = true;
    wavePhase = 0;
    document.body.classList.add("transcribing");
    statusText.textContent = "Transcribing";
  } else if (state === "Recording") {
    isTranscribing = false;
    document.body.classList.remove("transcribing");
    statusText.textContent = "Recording";
  } else {
    // Ready — window is about to be hidden, reset history for next recording
    isTranscribing = false;
    document.body.classList.remove("transcribing");
    statusText.textContent = "Recording";
    targetLevel = 0;
    smoothedLevels.fill(0);
  }
});
