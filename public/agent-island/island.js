const invoke = window.__TAURI__?.core?.invoke || (async () => null);
const listen = window.__TAURI__?.event?.listen || (async () => () => {});

const el = {
  stage: document.getElementById("stage"),
  island: document.getElementById("island"),
  notification: document.getElementById("notification"),
  title: document.getElementById("title"),
  agent: document.getElementById("agent"),
  project: document.getElementById("project"),
  summary: document.getElementById("summary"),
  track: document.getElementById("timeout-track"),
};

const EXIT_MS = 160; // keep in step with --duration-exit
const SWAP_MS = 110; // keep in step with .notification's transition
const ISLAND_HEIGHT = 130;
// Past this much travel the island is considered thrown away rather than
// nudged. Deliberately under half its height: by the time you have dragged a
// notification a third of the way back into the notch, you have decided.
const DISMISS_FRACTION = 0.34;

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

let dismissTimer = null;
let remaining = 0;
let startedAt = 0;
let visible = false;
let leaving = false;

/* ------------------------------------------------------------------ motion */

let settleFrame = 0;

/** Animates a scalar to `to` on a spring, starting from the value and velocity
 * it already has, so a gesture hands straight off into the animation with no
 * seam. Cancels whatever spring was already running. */
function spring(from, to, velocity, onFrame, { response = 0.35, damping = 1 } = {}) {
  cancelAnimationFrame(settleFrame);
  const w = (2 * Math.PI) / response;
  const stiffness = w * w;
  const drag = 2 * damping * w;
  let offset = from - to;
  let speed = velocity;
  let last = performance.now();

  return new Promise((resolve) => {
    if (reducedMotion.matches) {
      onFrame(to);
      resolve();
      return;
    }
    const step = (now) => {
      const elapsed = Math.min(0.032, (now - last) / 1000);
      last = now;
      // Substepped so a dropped frame cannot make the integrator explode.
      const count = Math.max(1, Math.ceil(elapsed / 0.004));
      const h = elapsed / count;
      for (let i = 0; i < count; i += 1) {
        speed += (-stiffness * offset - drag * speed) * h;
        offset += speed * h;
      }
      if (Math.abs(offset) < 0.4 && Math.abs(speed) < 14) {
        onFrame(to);
        resolve();
        return;
      }
      onFrame(to + offset);
      settleFrame = requestAnimationFrame(step);
    };
    settleFrame = requestAnimationFrame(step);
  });
}

/** Where a flick would come to rest if left to decelerate — the same
 * projection scroll views use, so a throw lands where the wrist aimed. */
function project(velocity, decelerationRate = 0.998) {
  return ((velocity / 1000) * decelerationRate) / (1 - decelerationRate);
}

/** Resistance past the boundary: the island follows a downward drag less and
 * less rather than stopping dead, because there is nothing below to reveal. */
function rubberband(overshoot, dimension, constant = 0.55) {
  return (overshoot * dimension * constant) / (dimension + constant * Math.abs(overshoot));
}

function setOffset(value) {
  el.island.style.setProperty("--drag-y", `${value.toFixed(2)}px`);
}

/* ---------------------------------------------------------------- lifetime */

function clearDismissTimer() {
  clearTimeout(dismissTimer);
  dismissTimer = null;
}

function scheduleDismiss(delay) {
  clearDismissTimer();
  remaining = delay;
  startedAt = Date.now();
  dismissTimer = setTimeout(dismiss, delay);
}

function pauseDismiss() {
  if (!dismissTimer) return;
  remaining = Math.max(0, remaining - (Date.now() - startedAt));
  clearDismissTimer();
  el.island.style.setProperty("--timeout-play-state", "paused");
}

function resumeDismiss() {
  if (el.island.dataset.persistent === "true" || remaining <= 0 || leaving) return;
  el.island.style.setProperty("--timeout-play-state", "running");
  scheduleDismiss(remaining);
}

/** Plays the exit, then asks the native side to hide the window. Ordering
 * matters: hiding first would make every dismissal a hard cut. */
async function dismiss() {
  if (leaving) return;
  leaving = true;
  clearDismissTimer();
  cancelAnimationFrame(settleFrame);
  el.stage.dataset.leaving = "true";
  await new Promise((resolve) => setTimeout(resolve, EXIT_MS));
  visible = false;
  await invoke("agent_island_dismiss").catch(() => {});
}

/* ------------------------------------------------------------------ render */

function writeContent(payload) {
  const kind = payload.kind || "completed";
  el.island.dataset.kind = kind;
  el.island.dataset.notch = String(Boolean(payload.hasNotch));
  el.island.dataset.persistent = String(Boolean(payload.persistent));
  el.title.textContent = payload.title || "Agent update";
  el.agent.textContent = payload.agent || "Agent";
  el.project.textContent = payload.project || "project";
  el.summary.textContent = payload.summary || "The agent has an update.";
}

function restartLifetime(payload) {
  const kind = payload.kind || "completed";
  const lifetime = kind === "turn_finished" ? 6000 : 7000;
  el.island.style.removeProperty("--timeout-play-state");
  el.island.style.setProperty("--timeout", `${lifetime / 1000}s`);
  // The depleting hairline lives on a pseudo-element, so the only way to
  // replay it for a second update is to hand it a fresh node.
  const fresh = el.track.cloneNode(true);
  el.track.replaceWith(fresh);
  el.track = fresh;
  clearDismissTimer();
  if (!payload.persistent) scheduleDismiss(lifetime);
}

function render(payload) {
  cancelAnimationFrame(settleFrame);
  setOffset(0);
  leaving = false;
  delete el.stage.dataset.leaving;

  if (visible) {
    // Already on screen: swap the contents in place instead of re-entering.
    el.notification.dataset.swapping = "true";
    setTimeout(() => {
      writeContent(payload);
      el.notification.dataset.swapping = "false";
      restartLifetime(payload);
    }, reducedMotion.matches ? 0 : SWAP_MS);
    return;
  }

  writeContent(payload);
  el.notification.dataset.swapping = "false";
  // Replay the descend for a genuine arrival by restarting the animation.
  el.stage.style.animation = "none";
  void el.stage.offsetWidth;
  el.stage.style.animation = "";
  visible = true;
  restartLifetime(payload);
}

/* ------------------------------------------------------------------ gesture */

let dragging = false;
let grabbedAt = 0;
let offset = 0;
let samples = [];

function velocityFromSamples() {
  if (samples.length < 2) return 0;
  const last = samples[samples.length - 1];
  const first = samples[0];
  const seconds = (last.t - first.t) / 1000;
  if (seconds <= 0) return 0;
  return (last.y - first.y) / seconds;
}

el.island.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || leaving) return;
  if (event.target.closest("button")) return; // let the actions win
  dragging = true;
  grabbedAt = event.clientY;
  samples = [{ y: event.clientY, t: event.timeStamp }];
  cancelAnimationFrame(settleFrame);
  el.island.dataset.dragging = "true";
  el.island.setPointerCapture(event.pointerId);
  pauseDismiss();
});

el.island.addEventListener("pointermove", (event) => {
  if (!dragging) return;
  const travel = event.clientY - grabbedAt;
  // Upward tracks the pointer exactly; downward resists, since the island is
  // already as far down as it goes.
  offset = travel < 0 ? travel : rubberband(travel, ISLAND_HEIGHT);
  setOffset(offset);
  samples.push({ y: event.clientY, t: event.timeStamp });
  if (samples.length > 5) samples.shift();
});

async function endDrag(event) {
  if (!dragging) return;
  dragging = false;
  delete el.island.dataset.dragging;
  el.island.releasePointerCapture?.(event.pointerId);

  const velocity = velocityFromSamples();
  const projected = offset + project(velocity);

  if (projected < -ISLAND_HEIGHT * DISMISS_FRACTION) {
    // Thrown back into the notch: carry the release velocity into the exit so
    // there is no seam between the finger and the animation.
    leaving = true;
    clearDismissTimer();
    const target = -(ISLAND_HEIGHT + 24);
    await spring(offset, target, velocity, setOffset, { response: 0.28 });
    el.stage.dataset.leaving = "true";
    await new Promise((resolve) => setTimeout(resolve, EXIT_MS));
    visible = false;
    await invoke("agent_island_dismiss").catch(() => {});
    return;
  }

  await spring(offset, 0, velocity, setOffset);
  offset = 0;
  resumeDismiss();
}

el.island.addEventListener("pointerup", endDrag);
el.island.addEventListener("pointercancel", endDrag);

/* ------------------------------------------------------------------ actions */

document.getElementById("dismiss").addEventListener("click", () => {
  dismiss();
});

document.getElementById("open").addEventListener("click", async () => {
  if (leaving) return;
  leaving = true;
  clearDismissTimer();
  el.stage.dataset.leaving = "true";
  await new Promise((resolve) => setTimeout(resolve, EXIT_MS));
  visible = false;
  await invoke("agent_island_open_activity").catch(console.error);
});

el.island.addEventListener("mouseenter", pauseDismiss);
el.island.addEventListener("mouseleave", () => {
  if (!dragging) resumeDismiss();
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) pauseDismiss();
  else resumeDismiss();
});

async function boot() {
  await listen("island://show", (event) => render(event.payload));
  await invoke("agent_island_ready");
}

boot().catch(console.error);
