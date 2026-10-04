const invoke = window.__TAURI__?.core?.invoke || (async () => null);
const listen = window.__TAURI__?.event?.listen || (async () => () => {});

const el = {
  presence: document.getElementById("presence"),
  title: document.getElementById("title"),
  detail: document.getElementById("detail"),
  elapsed: document.getElementById("elapsed"),
  waveform: document.getElementById("waveform"),
  capsule: document.getElementById("capsule"),
  recovery: document.getElementById("recovery"),
  recoveryTitle: document.getElementById("recovery-title"),
  recoveryDetail: document.getElementById("recovery-detail"),
  recoverySymbol: document.getElementById("recovery-symbol"),
  recoveryDiscard: document.getElementById("recovery-discard"),
  recoveryCopy: document.getElementById("recovery-copy"),
  recoveryPreview: document.getElementById("recovery-preview"),
  editor: document.getElementById("editor"),
  editorKicker: document.getElementById("editor-kicker"),
  editorTitle: document.getElementById("editor-title"),
  text: document.getElementById("text"),
  petCapsule: document.getElementById("pet-capsule"),
  petLevel: document.getElementById("pet-level"),
  petStatus: document.getElementById("pet-status"),
};

// States where Samlu itself stands in for the waveform capsule.
const PET_STATES = new Set(["listening", "processing", "delivering", "copied"]);
const PET_STATUS = {
  listening: "Listening",
  processing: "Transcribing",
  delivering: "Inserting",
  copied: "Couldn’t insert\nCopied — paste with ⌘V",
};

// The window is inset by body padding on every side; the surface fills what
// is left. Rust owns the window size and sends it with each state.
const BODY_INSET = 8;

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

let currentState = "idle";
let recordingStartedAt = 0;
let elapsedTimer = null;
let levelTarget = 0.04;
let levelCurrent = 0.04;
let animationFrame = 0;
let audioContext = null;
let visible = false;

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

async function loadAppearance() {
  try {
    applyAppearance(await invoke("get_appearance"));
  } catch (_) {
    applyAppearance("system");
  }
}

function startElapsed() {
  recordingStartedAt = Date.now();
  clearInterval(elapsedTimer);
  const update = () => {
    const seconds = Math.floor((Date.now() - recordingStartedAt) / 1000);
    el.elapsed.textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  };
  update();
  elapsedTimer = setInterval(update, 500);
}

function stopElapsed() {
  clearInterval(elapsedTimer);
  elapsedTimer = null;
  el.elapsed.textContent = "";
}

function animateLevel() {
  levelCurrent += (levelTarget - levelCurrent) * 0.28;
  const level = levelCurrent.toFixed(3);
  el.waveform.style.setProperty("--voice-level", level);
  // Samlu swells with your voice rather than drawing bars for it.
  el.petLevel.style.setProperty("--voice-level", level);
  levelTarget *= 0.9;
  animationFrame = requestAnimationFrame(animateLevel);
}

function tone(frequency, duration, volume = 0.025) {
  try {
    audioContext ||= new AudioContext();
    const oscillator = audioContext.createOscillator();
    const gain = audioContext.createGain();
    oscillator.type = "sine";
    oscillator.frequency.value = frequency;
    gain.gain.setValueAtTime(0.0001, audioContext.currentTime);
    gain.gain.exponentialRampToValueAtTime(volume, audioContext.currentTime + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, audioContext.currentTime + duration);
    oscillator.connect(gain).connect(audioContext.destination);
    oscillator.start();
    oscillator.stop(audioContext.currentTime + duration + 0.02);
  } catch (_) {
    // Audio feedback is supplementary; visual feedback is always complete.
  }
}

function playStateSound(state, enabled) {
  if (!enabled || state === currentState) return;
  if (state === "listening") tone(520, 0.08);
  if (state === "success") {
    tone(590, 0.07);
    setTimeout(() => tone(760, 0.11), 65);
  }
  if (state === "copied") tone(460, 0.09, 0.018);
  if (state === "error" || state === "recovery" || state === "failed") tone(280, 0.14, 0.02);
}

/* ------------------------------------------------------------------ surface */

/** Exactly one content layer is live at a time; the rest fade out in place
 * rather than being pulled out of the layout, so the surface never jumps. */
function setActiveLayer(active) {
  [el.petCapsule, el.capsule, el.recovery, el.editor].forEach((layer) => {
    layer.dataset.active = String(layer === active);
  });
}

/** Resizes the one surface to the bounds Rust picked for this state. The
 * window is already that big; growing the glass into it is what makes the
 * change read as a panel expanding instead of a new window appearing. */
function resizeSurface(payload) {
  if (!payload.width || !payload.height) return;
  el.presence.style.width = `${payload.width - BODY_INSET * 2}px`;
  el.presence.style.height = `${payload.height - BODY_INSET * 2}px`;
}

function render(payload) {
  const state = payload.state || "listening";
  playStateSound(state, payload.interfaceSounds);

  // A state arriving during the exit means the surface is wanted again.
  delete el.presence.dataset.leaving;
  const arriving = !visible;
  if (arriving) {
    // Genuine arrival: replay the entrance rather than letting the finished
    // fill state leave it silently on screen, and take the new size outright
    // instead of morphing out of whatever the last session ended on.
    el.presence.dataset.instant = "true";
    el.presence.style.animation = "none";
    void el.presence.offsetWidth;
    el.presence.style.animation = "";
    visible = true;
  }

  currentState = state;
  document.body.dataset.state = state;

  // The pet only stands in for the recording capsule; preview, recovery and
  // error states always use their full-size presentations.
  const pet = Boolean(payload.pet) && PET_STATES.has(state);
  document.body.dataset.pet = String(pet);

  el.title.textContent = payload.title || "Voice";
  el.detail.textContent = payload.detail || "";
  if (pet) el.petStatus.textContent = payload.detail || PET_STATUS[state] || "";

  if (state === "recovery" || state === "failed") {
    // A failed dictation can always be retried, but only has text to copy
    // or edit once transcription itself succeeded.
    const failed = state === "failed";
    const hasText = !failed || Boolean(payload.text);
    el.recoveryTitle.textContent = payload.title || "Result is safe";
    el.recoveryDetail.textContent = payload.detail || "Samlu could not insert into the original field.";
    el.recoveryDetail.title = payload.detail || "";
    el.recoverySymbol.textContent = failed ? "!" : "↗";
    el.recoveryDiscard.hidden = !failed;
    el.recoveryCopy.hidden = !hasText;
    el.recoveryPreview.hidden = !hasText;
  }

  if (state === "preview") {
    el.editorKicker.textContent = payload.mode || "Voice";
    el.editorTitle.textContent = payload.title || "Voice result";
    el.text.value = payload.text || "";
  }

  if (pet) setActiveLayer(el.petCapsule);
  else if (state === "recovery" || state === "failed") setActiveLayer(el.recovery);
  else if (state === "preview") setActiveLayer(el.editor);
  else setActiveLayer(el.capsule);

  resizeSurface(payload);
  if (arriving) {
    // Commit the size before transitions come back, so the next state change
    // morphs from here rather than from the previous session's bounds.
    void el.presence.offsetWidth;
    delete el.presence.dataset.instant;
  }

  if (state === "listening") startElapsed();
  else stopElapsed();

  if (state === "preview") {
    requestAnimationFrame(() => {
      el.text.focus();
      el.text.setSelectionRange(el.text.value.length, el.text.value.length);
    });
  }
}

/** Rust asks for the exit, waits out its duration, then hides the window. */
function playExit() {
  visible = false;
  el.presence.dataset.leaving = "true";
}

/* ------------------------------------------------------------------ gesture
 * The capsule can sit on top of exactly the thing you are dictating about, so
 * it can be picked up and moved. Screen coordinates, not client coordinates:
 * the window moves under the pointer, which would otherwise feed its own
 * motion back into the delta.
 */

let dragging = false;
let lastScreen = [0, 0];
let pendingDx = 0;
let pendingDy = 0;
let dragFrame = 0;
let glideFrame = 0;
let samples = [];

function flushDrag() {
  dragFrame = 0;
  if (!pendingDx && !pendingDy) return;
  const dx = pendingDx;
  const dy = pendingDy;
  pendingDx = 0;
  pendingDy = 0;
  invoke("voice_preview_drag", { dx, dy }).catch(() => {});
}

function queueDrag(dx, dy) {
  pendingDx += dx;
  pendingDy += dy;
  // One move per frame: the window cannot be repositioned faster than it can
  // be drawn, and a flood of calls only adds latency.
  if (!dragFrame) dragFrame = requestAnimationFrame(flushDrag);
}

function releaseVelocity() {
  if (samples.length < 2) return [0, 0];
  const last = samples[samples.length - 1];
  const first = samples[0];
  const seconds = (last.t - first.t) / 1000;
  if (seconds <= 0) return [0, 0];
  return [(last.x - first.x) / seconds, (last.y - first.y) / seconds];
}

/** Carries the release velocity on into a deceleration, so letting go of a
 * throw keeps moving instead of stopping dead under the finger. */
function glide(vx, vy) {
  cancelAnimationFrame(glideFrame);
  if (reducedMotion.matches) return;
  let speedX = vx;
  let speedY = vy;
  let last = performance.now();
  const decay = 0.998;
  const step = (now) => {
    const ms = Math.min(32, now - last);
    last = now;
    queueDrag((speedX * ms) / 1000, (speedY * ms) / 1000);
    const damping = Math.pow(decay, ms);
    speedX *= damping;
    speedY *= damping;
    if (Math.hypot(speedX, speedY) < 40) return;
    glideFrame = requestAnimationFrame(step);
  };
  glideFrame = requestAnimationFrame(step);
}

el.presence.addEventListener("pointerdown", (event) => {
  if (event.button !== 0) return;
  // Controls and the editable field win over the drag.
  if (event.target.closest("button, textarea, input")) return;
  dragging = true;
  lastScreen = [event.screenX, event.screenY];
  samples = [{ x: event.screenX, y: event.screenY, t: event.timeStamp }];
  cancelAnimationFrame(glideFrame);
  el.presence.dataset.dragging = "true";
  el.presence.setPointerCapture(event.pointerId);
});

el.presence.addEventListener("pointermove", (event) => {
  if (!dragging) return;
  queueDrag(event.screenX - lastScreen[0], event.screenY - lastScreen[1]);
  lastScreen = [event.screenX, event.screenY];
  samples.push({ x: event.screenX, y: event.screenY, t: event.timeStamp });
  if (samples.length > 5) samples.shift();
});

function endDrag(event) {
  if (!dragging) return;
  dragging = false;
  delete el.presence.dataset.dragging;
  el.presence.releasePointerCapture?.(event.pointerId);
  const [vx, vy] = releaseVelocity();
  if (Math.hypot(vx, vy) > 120) glide(vx, vy);
}

el.presence.addEventListener("pointerup", endDrag);
el.presence.addEventListener("pointercancel", endDrag);

/* ------------------------------------------------------------------ actions */

async function insertPreview() {
  const value = el.text.value.trim();
  if (!value) return;
  await invoke("voice_preview_accept", { text: value });
}

document.getElementById("stop-recording").addEventListener("click", () => {
  invoke("voice_stop").catch(console.error);
});

document.getElementById("cancel-recording").addEventListener("click", () => {
  invoke("voice_cancel").catch(console.error);
});

document.getElementById("insert-preview").addEventListener("click", () => {
  insertPreview().catch(console.error);
});

document.getElementById("cancel-preview").addEventListener("click", () => {
  invoke("voice_preview_cancel").catch(console.error);
});

document.getElementById("recovery-retry").addEventListener("click", () => {
  invoke("voice_recovery_retry").catch(console.error);
});

el.recoveryDiscard.addEventListener("click", () => {
  invoke("voice_cancel").catch(console.error);
});

document.getElementById("recovery-copy").addEventListener("click", () => {
  invoke("voice_recovery_copy").catch(console.error);
});

document.getElementById("recovery-preview").addEventListener("click", () => {
  invoke("voice_recovery_preview").catch(console.error);
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    event.preventDefault();
    if (currentState === "preview") invoke("voice_preview_cancel").catch(console.error);
    else invoke("voice_cancel").catch(console.error);
  } else if (event.key === "Enter" && event.metaKey && currentState === "preview") {
    event.preventDefault();
    insertPreview().catch(console.error);
  }
});

cancelAnimationFrame(animationFrame);
animateLevel();

async function boot() {
  await Promise.all([
    listen("voice://state", (event) => render(event.payload)),
    listen("voice://dismiss", playExit),
    listen("voice://level", (event) => {
      levelTarget = Math.max(0.025, Math.min(1, Number(event.payload) || 0));
    }),
    listen("appearance://changed", (event) => applyAppearance(event.payload)),
    loadAppearance(),
  ]);
  await invoke("voice_preview_ready");
}

boot().catch(console.error);
