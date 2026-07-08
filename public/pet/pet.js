/* Samlu mascot overlay — controller for the "thinking" widget.
 *
 * Runs inside the "pet" window only. Talks to the host over Tauri events.
 *   inbound : "pet://state" -> { state?, event?, phase?, source?, detail? }
 *   outbound: "pet://poke"  -> user clicked the mascot (bring Samlu's
 *                              settings window to the front)
 *
 * Adapted from the Kettles project (same author) — the animation/rendering
 * engine below is unchanged from there; the timer-specific controls and
 * mascot-preference system have been removed since Samlu ships one fixed
 * mascot and has no timer to control.
 *
 * Rendering: the sprite sheet is drawn with PIXEL background-position/size
 * (computed from the config + a target height), and the mascot animates every
 * frame. Both keep it reliably painted on the Windows WebView2 compositor.
 */

const tauri = window.__TAURI__ || {};
const listen = tauri.event?.listen || (async () => () => {});
const emit = tauri.event?.emit || (async () => {});
const getCurrentWindow =
  tauri.window?.getCurrentWindow ||
  tauri.webviewWindow?.getCurrentWebviewWindow ||
  null;

const el = {
  shell: document.getElementById("shell"),
  mascot: document.getElementById("mascot"),
  bubble: document.getElementById("bubble"),
  dot: document.getElementById("dot"),
  label: document.getElementById("label"),
  timer: document.getElementById("timer"),
  task: document.getElementById("task"),
  collapse: document.getElementById("collapse"),
  reopen: document.getElementById("reopen"),
};

window.addEventListener("error", (e) =>
  console.error("[pet]", e.error || e.message)
);

const PHASE_LABELS = {
  idle: "Watching",
  attention: "Needs you",
  success: "Done",
};

// ---------------------------------------------------------------------------
// runtime state
// ---------------------------------------------------------------------------

let cfg = null;
let scale = 1; // sheet px -> screen px
let baseState = "idle"; // looping state to return to after a one-shot
let current = "idle"; // animation currently on screen
let oneShot = false;
let frame = 0;
let lastTick = 0;
let phase = "idle";
let collapsed = false;
let dragging = false;
let isHovered = false; // true while cursor is over the mascot element
let lastX = null;
let lastY = null;
let lastActiveScale = null;

// ---------------------------------------------------------------------------
// boot
// ---------------------------------------------------------------------------

async function boot() {
  cfg = await fetch("./pet.config.json?t=" + Date.now()).then((r) => {
    if (!r.ok) throw new Error(`pet.config.json: ${r.status}`);
    return r.json();
  });

  scale = cfg.scale || 0.58;
  const ch = cfg.cell.height * scale;
  const cw = cfg.cell.width * scale;
  document.documentElement.style.setProperty("--mascot-width", `${cw}px`);
  document.documentElement.style.setProperty("--mascot-height", `${ch}px`);
  el.mascot.style.width = `${cw}px`;
  el.mascot.style.height = `${ch}px`;
  el.mascot.style.marginLeft = `${-cw / 2}px`;
  el.mascot.style.backgroundImage = `url("${cfg.spritesheet}")`;
  el.mascot.style.backgroundSize = `${cfg.sheet.cols * cw}px ${cfg.sheet.rows * ch}px`;

  await listen("pet://state", (e) => onSignal(e.payload || {}));

  wireInput();
  applyPhase("idle");
  syncAnimToPhase("idle");
  requestAnimationFrame(loop);
}

// ---------------------------------------------------------------------------
// inbound signals
// ---------------------------------------------------------------------------

function eventTarget(name) {
  const v = cfg.events[name];
  if (!v) return null;
  return typeof v === "string" ? { play: v } : v;
}

function onSignal(sig) {
  if (typeof sig.source === "string") el.task.textContent = sig.source || "No agent activity yet";
  if (typeof sig.detail === "string") el.timer.textContent = sig.detail || "";
  if (typeof sig.phase === "string") applyPhase(sig.phase);

  if (dragging) return; // If dragging, ignore incoming animation changes!

  let play = null;
  let then = null;

  if (sig.state && cfg.states[sig.state]) {
    play = sig.state;
  } else if (sig.event) {
    const t = eventTarget(sig.event);
    if (t) {
      play = cfg.states[t.play] ? t.play : null;
      then = t.then && cfg.states[t.then] ? t.then : null;
    }
  }

  if (cfg.flashOn && sig.event && cfg.flashOn.includes(sig.event)) pop();

  if (play) {
    if (then) baseState = then;
    applyState(play);
  } else if (typeof sig.phase === "string") {
    syncAnimToPhase(sig.phase);
  }
}

// ---------------------------------------------------------------------------
// phase
// ---------------------------------------------------------------------------

function applyPhase(next) {
  if (!PHASE_LABELS[next]) return;
  const changed = next !== phase;
  phase = next;
  el.shell.dataset.phase = next;
  el.label.textContent = PHASE_LABELS[next];
  // Surface a freshly-arrived "needs you" event even if the bubble is
  // collapsed — but only on the transition, so the user can still collapse
  // it again afterwards (this would otherwise fire every time a duplicate
  // signal arrives for the same session).
  if (changed && next === "attention" && collapsed) setCollapsed(false);
}

function syncAnimToPhase(p) {
  if (oneShot || dragging) return; // Do not overwrite active dragging animation!
  const target = cfg.phaseStates && cfg.phaseStates[p];
  if (target && cfg.states[target] && baseState !== target) applyState(target);
}

// ---------------------------------------------------------------------------
// animation state machine
// ---------------------------------------------------------------------------

function applyState(name) {
  if (!cfg.states[name]) return;
  if (cfg.states[name].loop) {
    baseState = name;
    setState(name);
  } else {
    playOneShot(name);
  }
}

function setState(name) {
  current = name;
  frame = 0;
  lastTick = 0;
  oneShot = false;
}

function playOneShot(name) {
  current = name;
  frame = 0;
  lastTick = 0;
  oneShot = true;
}

// ---------------------------------------------------------------------------
// render loop — runs continuously so the sprite is never left unpainted
// ---------------------------------------------------------------------------

function loop(now) {
  if (cfg) draw(now);
  requestAnimationFrame(loop);
}

function draw(now) {
  const s = cfg.states[current] || cfg.states.idle;
  const activeScale = scale * (s.scale || 1.0);

  if (activeScale !== lastActiveScale) {
    const ch = cfg.cell.height * activeScale;
    const cw = cfg.cell.width * activeScale;
    el.mascot.style.width = `${cw}px`;
    el.mascot.style.height = `${ch}px`;
    el.mascot.style.marginLeft = `${-cw / 2}px`;
    el.mascot.style.backgroundSize = `${cfg.sheet.cols * cw}px ${cfg.sheet.rows * ch}px`;

    document.documentElement.style.setProperty("--mascot-width", `${cw}px`);
    document.documentElement.style.setProperty("--mascot-height", `${ch}px`);

    lastActiveScale = activeScale;
    lastX = null;
    lastY = null;
  }

  const frameMs = 1000 / s.fps;

  // Frozen while collapsed: hold the current frame (no animation). The sprite
  // still gets its position written each rAF so the layer stays painted.
  // Also freeze loop animations at frame 0 when idle (not hovered, not dragging)
  // — this gives complete stillness when the user isn't interacting.
  const isIdle = !isHovered && !dragging && !oneShot && s.loop;
  if (isIdle) {
    // Pin to frame 0 — the mascot holds a still pose
    frame = 0;
  } else if (!collapsed && now - lastTick >= frameMs) {
    lastTick = now;
    frame += 1;
    if (frame >= s.frames) {
      frame = 0;
      if (oneShot) {
        setState(baseState); // one-shot finished -> settle into the loop
        return;
      }
    }
  }

  const col = (s.col || 0) + frame;
  const x = Math.round(-(col * cfg.cell.width * activeScale));
  const ch = cfg.cell.height * activeScale;
  const y = Math.round(-(s.row * ch));

  if (x !== lastX || y !== lastY) {
    el.mascot.style.backgroundPosition = `${x}px ${y}px`;
    lastX = x;
    lastY = y;
  }
}

function pop() {
  el.mascot.classList.remove("pop");
  void el.mascot.offsetWidth;
  el.mascot.classList.add("pop");
}

// ---------------------------------------------------------------------------
// collapse / expand
// ---------------------------------------------------------------------------

function setCollapsed(next) {
  if (collapsed === next) return;
  collapsed = next;
  el.shell.dataset.collapsed = String(next);
  if (next) {
    setState("idle"); // freeze on a calm pose, not mid-stride
  } else {
    lastTick = 0; // resume cleanly
    syncAnimToPhase(phase);
  }
}

// ---------------------------------------------------------------------------
// input: tap / drag
// ---------------------------------------------------------------------------

const DRAG_THRESHOLD = 4;

function wireInput() {
  let pressed = false;
  let dragged = false;
  let onMascot = false;
  let startX = 0;
  let startY = 0;
  let lastDragX = null;

  const beginDrag = (direction) => {
    dragging = true;
    isHovered = false; // cursor left mascot when drag started
    lastDragX = startX;
    el.shell.classList.add("dragging");
    applyState(direction === "left" ? "running_left" : "running_right");
    getCurrentWindow?.()?.startDragging?.();
  };

  const endDrag = () => {
    if (!dragging) return;
    dragging = false;
    lastDragX = null;
    el.shell.classList.remove("dragging");
    syncAnimToPhase(phase); // settle back to the phase's looping state
  };

  el.shell.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    if (e.target.closest("button")) return; // let buttons handle their own clicks
    pressed = true;
    dragged = false;
    onMascot = e.target === el.mascot;
    startX = e.screenX;
    startY = e.screenY;
  });

  window.addEventListener("mousemove", (e) => {
    if (dragging && (e.buttons & 1) === 0) {
      endDrag();
      return;
    }
    if (dragging) {
      const currentX = e.screenX;
      if (lastDragX !== null) {
        if (currentX < lastDragX - 1) {
          applyState("running_left");
        } else if (currentX > lastDragX + 1) {
          applyState("running_right");
        }
      }
      lastDragX = currentX;
      return;
    }
    if (!pressed) return;
    if (Math.hypot(e.screenX - startX, e.screenY - startY) > DRAG_THRESHOLD) {
      pressed = false;
      dragged = true;
      const direction = (e.screenX < startX) ? "left" : "right";
      beginDrag(direction);
    }
  });

  window.addEventListener("mouseenter", (e) => {
    if (dragging && (e.buttons & 1) === 0) {
      endDrag();
    }
  });

  window.addEventListener("mouseup", (e) => {
    if (e.button !== 0) return;
    if (dragging) endDrag();
    if (pressed && !dragged && onMascot) {
      setCollapsed(!collapsed); // Toggle collapsed state on left click
    }
    pressed = false;
  });

  el.collapse.addEventListener("click", (e) => {
    e.stopPropagation();
    setCollapsed(true);
  });

  el.reopen.addEventListener("click", (e) => {
    e.stopPropagation();
    setCollapsed(false);
  });

  // Mascot interaction animations
  el.mascot.addEventListener("mouseenter", () => {
    if (collapsed || dragging) return;
    isHovered = true;
    applyState("waving"); // Hovering always plays the wave
  });

  el.mascot.addEventListener("mouseleave", () => {
    isHovered = false;
    if (!dragging && !oneShot) {
      // Return to the phase's looping state (e.g. "waiting" when idle)
      syncAnimToPhase(phase);
    }
  });

  el.mascot.addEventListener("click", () => {
    if (dragging) return;
    const target = eventTarget("click");
    if (target && cfg.states[target.play]) {
      applyState(target.play);
    }
    // A click always brings Samlu's settings window to the front, in
    // addition to whatever animation it plays.
    pokeApp();
  });

  // Reset dragging/hover states on window focus/blur
  window.addEventListener("blur", () => {
    isHovered = false;
    if (dragging) endDrag();
  });
  window.addEventListener("focus", () => {
    if (dragging) endDrag();
  });
}

function pokeApp() {
  emit("pet://poke", { at: Date.now() });
}

boot().catch((err) => console.error("[pet] boot failed", err));
