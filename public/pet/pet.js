/* Samlu's flight choreography.
 *
 * Two animations share one bee: `pet://show` flies it in from the screen edge
 * to the pointer and drops a card, `pet://carry` ferries a dictation result
 * from the recording capsule to the pointer. Both use the same bezier + lateral
 * wander + banking model, so the bee moves the same way whatever it is doing. */

const invoke = window.__TAURI__?.core?.invoke || (async () => null);
const listen = window.__TAURI__?.event?.listen || (async () => () => {});

const el = {
  anchor: document.getElementById("anchor"),
  bee: document.getElementById("bee"),
  card: document.getElementById("card"),
  title: document.getElementById("title"),
  agent: document.getElementById("agent"),
  project: document.getElementById("project"),
  summary: document.getElementById("summary"),
  open: document.getElementById("open"),
};

const CARD_WIDTH = 320;
const CARD_ESTIMATED_HEIGHT = 152;
const LIFETIME = 9000;
const CARD_PADDING = 10;
const EXIT_MS = 160; // keep in step with --duration-exit

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

let flight = 0;
let generation = 0;
let timers = [];
let activePayload = null;
let dismissRemaining = 0;
let dismissStartedAt = 0;

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

invoke("get_appearance").then(applyAppearance).catch(() => {});
listen("appearance://changed", (event) => applyAppearance(event.payload)).catch(() => {});

function clearTimers() {
  timers.forEach(clearTimeout);
  timers = [];
}

function later(fn, ms) {
  timers.push(setTimeout(fn, ms));
}

function scheduleDismiss(mine, delay) {
  clearTimers();
  dismissRemaining = delay;
  dismissStartedAt = Date.now();
  later(async () => {
    if (mine !== generation) return;
    await hideCard();
    if (mine === generation) dismiss();
  }, delay);
}

const rand = (a, b) => a + Math.random() * (b - a);
const easeInOutQuad = (t) => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);
const easeOutCubic = (t) => 1 - Math.pow(1 - t, 3);
// Overshoots past the target and eases back — the bee's final bob.
const easeBack = (t) => {
  const q = t - 1;
  return 1 + 2.2 * q * q * q + 1.2 * q * q;
};

/* ------------------------------------------------------------------ motion */

/** Runs `step(progress)` for `duration` ms, cancelling any flight already in
 * the air. Resolves once it lands. */
function animate(duration, step) {
  const id = ++flight;
  return new Promise((resolve) => {
    if (reducedMotion.matches) {
      step(1);
      resolve();
      return;
    }
    const start = performance.now();
    const frame = (now) => {
      if (id !== flight) {
        resolve();
        return;
      }
      const t = Math.min(1, (now - start) / duration);
      step(t);
      if (t < 1) requestAnimationFrame(frame);
      else resolve();
    };
    requestAnimationFrame(frame);
  });
}

/** A cubic bezier from `start` to the origin, with jittered control points so
 * no two flights trace the same path. */
function makePath(start) {
  const [sx, sy] = start;
  const c1 = [sx * 0.7 + rand(-60, 60), sy * 0.55 + rand(-40, 90)];
  const c2 = [sx * 0.26 + rand(-120, 120), rand(-140, 60)];
  return (e) => {
    const u = 1 - e;
    return [
      u * u * u * sx + 3 * u * u * e * c1[0] + 3 * u * e * e * c2[0],
      u * u * u * sy + 3 * u * u * e * c1[1] + 3 * u * e * e * c2[1],
    ];
  };
}

/** Sideways drift layered on top of the path, damped at both ends so departure
 * and arrival stay clean. */
function makeWander() {
  const phase = [rand(0, 6.3), rand(0, 6.3)];
  const w1 = rand(4.5, 7);
  const w2 = rand(9, 13);
  return (e) =>
    (11 * Math.sin(e * w1 + phase[0]) + 5 * Math.sin(e * w2 + phase[1])) *
    Math.sin(Math.PI * Math.min(1, e * 1.15));
}

/** Flies the bee along `path`, banking into its own turns. `scale` and `fade`
 * shape the carry flight, which shrinks and fades as it arrives. */
function flyPath(start, duration, { ease = easeInOutQuad, scale, fade } = {}) {
  const path = makePath(start);
  const wander = makeWander();
  return animate(duration, (t) => {
    const e = ease(t);
    const p = path(e);
    const next = path(Math.min(1, e + 0.01));
    const dx = next[0] - p[0];
    const dy = next[1] - p[1];
    const len = Math.hypot(dx, dy) || 1;
    const wv = wander(e);
    const wn = wander(Math.min(1, e + 0.02));
    const x = p[0] + (-dy / len) * wv;
    const y = p[1] + (dx / len) * wv;
    const bank = Math.max(-15, Math.min(15, (wn - wv) * 2.4));
    let transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) rotate(${bank.toFixed(1)}deg)`;
    if (scale) transform += ` scale(${(1 - scale * e).toFixed(3)})`;
    el.bee.style.transform = transform;
    if (fade) el.bee.style.opacity = t > 0.88 ? ((1 - t) / 0.12).toFixed(2) : "1";
  });
}

/** Straight-line hop, used for the settle sequence after arrival. */
function hop(from, to, duration, ease) {
  return animate(duration, (t) => {
    const e = ease(t);
    const x = from[0] + (to[0] - from[0]) * e;
    const y = from[1] + (to[1] - from[1]) * e;
    el.bee.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px)`;
  });
}

/** Where the bee enters and leaves from: just past the top-right corner of the
 * display, measured relative to the anchor. */
function entryPoint(x, y) {
  return [window.innerWidth - x + 90, -y - 70];
}

/* -------------------------------------------------------------- delivery */

function placeCard(x, y) {
  // Flip to whichever side keeps the card on screen.
  const flipLeft = x + 38 + CARD_WIDTH > window.innerWidth;
  const flipUp = y + 34 + CARD_ESTIMATED_HEIGHT > window.innerHeight;
  el.card.style.left = flipLeft ? `${-(CARD_WIDTH + 26)}px` : "38px";
  el.card.style.top = flipUp ? `${-CARD_ESTIMATED_HEIGHT}px` : "34px";
}

async function showCard(payload) {
  delete el.card.dataset.leaving;
  el.card.dataset.persistent = String(Boolean(payload.persistent));
  el.card.style.removeProperty("--timeout-play-state");
  el.card.style.setProperty("--timeout", `${LIFETIME / 1000}s`);
  el.card.hidden = false;
  el.card.style.visibility = "hidden";

  // Measure the finished card in display-local coordinates, include the bee,
  // and then compact the native overlay to this union before it accepts input.
  const card = el.card.getBoundingClientRect();
  const bee = el.bee.getBoundingClientRect();
  const left = Math.max(0, Math.floor(Math.min(card.left, bee.left) - CARD_PADDING));
  const top = Math.max(0, Math.floor(Math.min(card.top, bee.top) - CARD_PADDING));
  const right = Math.min(
    window.innerWidth,
    Math.ceil(Math.max(card.right, bee.right) + CARD_PADDING),
  );
  const bottom = Math.min(
    window.innerHeight,
    Math.ceil(Math.max(card.bottom, bee.bottom) + CARD_PADDING),
  );

  try {
    await invoke("pet_set_interactive", {
      interactive: true,
      bounds: { x: left, y: top, width: right - left, height: bottom - top },
    });
    el.anchor.style.left = `${payload.x - left}px`;
    el.anchor.style.top = `${payload.y - top}px`;
  } catch (error) {
    console.error("[pet] could not create compact interaction surface", error);
  }
  el.card.style.visibility = "";
}

async function hideCard(immediate = false) {
  if (el.card.hidden) return;
  await invoke("pet_set_interactive", { interactive: false, bounds: null }).catch(() => {});
  el.card.dataset.leaving = "true";
  if (!immediate) {
    // Reduced motion swaps the travel for a cross-fade rather than removing
    // the exit, so the wait is the same either way.
    await new Promise((resolve) => setTimeout(resolve, EXIT_MS));
  }
  el.card.hidden = true;
}

async function deliver(payload) {
  const mine = ++generation;
  clearTimers();
  await hideCard(true);
  if (mine !== generation) return;
  activePayload = payload;

  el.anchor.style.left = `${payload.x}px`;
  el.anchor.style.top = `${payload.y}px`;
  el.title.textContent = payload.title || "Agent update";
  el.agent.textContent = payload.agent || "agent";
  el.project.textContent = payload.project || "";
  el.summary.textContent = payload.summary || "";
  el.open.textContent = payload.action || "Open";
  placeCard(payload.x, payload.y);

  el.card.hidden = true;
  el.bee.hidden = false;
  el.bee.style.opacity = "1";

  const entry = entryPoint(payload.x, payload.y);
  el.bee.style.transform = `translate(${entry[0]}px, ${entry[1]}px)`;

  await flyPath(entry, 850);
  if (mine !== generation) return;

  // Land, overshoot, drift back — the bee settling onto a surface.
  await hop([0, 0], [12, 6], 90, easeOutCubic);
  if (mine !== generation) return;
  await hop([12, 6], [-7, -4], 120, easeOutCubic);
  if (mine !== generation) return;
  await hop([-7, -4], [0, 0], 180, easeBack);
  if (mine !== generation) return;

  await showCard(payload);

  if (payload.persistent) return;
  scheduleDismiss(mine, LIFETIME);
}

async function carry(payload) {
  const mine = ++generation;
  clearTimers();
  await hideCard(true);

  el.anchor.style.left = `${payload.toX}px`;
  el.anchor.style.top = `${payload.toY}px`;
  el.bee.hidden = false;
  el.bee.style.opacity = "1";

  const start = [payload.fromX - payload.toX, payload.fromY - payload.toY];
  el.bee.style.transform = `translate(${start[0]}px, ${start[1]}px)`;
  await flyPath(start, payload.duration || 600, { scale: 0.42, fade: true });
  if (mine !== generation) return;
  hidePet();
  await invoke("pet_dismiss").catch(() => {});
}

function hidePet() {
  el.bee.hidden = true;
  el.bee.style.opacity = "1";
}

function dismiss() {
  generation += 1;
  flight += 1;
  clearTimers();
  hidePet();
  el.card.hidden = true;
  activePayload = null;
  invoke("pet_dismiss").catch(() => {});
}

el.card.addEventListener("mouseenter", () => {
  if (!timers.length) return;
  dismissRemaining = Math.max(0, dismissRemaining - (Date.now() - dismissStartedAt));
  clearTimers();
  el.card.style.setProperty("--timeout-play-state", "paused");
});
el.card.addEventListener("mouseleave", () => {
  if (!activePayload?.persistent && dismissRemaining > 0) {
    el.card.style.setProperty("--timeout-play-state", "running");
    scheduleDismiss(generation, dismissRemaining);
  }
});
document.getElementById("dismiss").addEventListener("click", dismiss);
el.open.addEventListener("click", () => {
  generation += 1;
  clearTimers();
  hidePet();
  el.card.hidden = true;
  invoke("pet_open_activity").catch(() => {});
});

async function boot() {
  await Promise.all([
    listen("pet://show", (event) => deliver(event.payload)),
    listen("pet://carry", (event) => carry(event.payload)),
  ]);
  await invoke("pet_ready");
}

boot().catch(console.error);
