const invoke = window.__TAURI__?.core?.invoke || (async () => null);
const listen = window.__TAURI__?.event?.listen || (async () => () => {});

const byId = (id) => document.getElementById(id);

const el = {
  stage: byId("stage"),
  island: byId("island"),
  body: byId("island-body"),
  notification: byId("notification"),
  slot: byId("agent-slot"),
  copy: byId("copy"),
  meta: byId("meta"),
  actions: byId("actions"),
  cardActions: byId("card-actions"),
  queueStep: byId("queue-step"),
  queueCount: byId("queue-count"),
  title: byId("title"),
  agent: byId("agent"),
  project: byId("project"),
  summary: byId("summary"),
  track: byId("timeout-track"),
  glyph: byId("glyph"),
  checkPath: byId("check-path"),
  arrow: byId("glyph-arrow"),
  burst: byId("burst"),
  lean: byId("buddy-lean"),
  buddy: byId("buddy"),
  mark: byId("buddy-mark"),
  foot: byId("piece-foot"),
};

const SWAP_MS = 110;
// The island is 130 tall for a plain notification and 176 while it carries
// approval cards (the window is 420 x 184 around it).
const ISLAND_HEIGHT_PLAIN = 130;
const ISLAND_HEIGHT_QUEUE = 176;
const NOTE_MS = 2500;
// Past this much travel the island is considered thrown away rather than
// nudged. Deliberately under half its height: by the time you have dragged a
// notification a third of the way back into the notch, you have decided.
const DISMISS_FRACTION = 0.34;
// Bloom grows out of, and is drawn back into, a shape the size of the camera
// housing (186 x 32 points) inside the 404 x 130 island.
const housingScale = () => `scale(0.46, ${(32 / islandHeight()).toFixed(3)})`;

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

/* Springs sampled into linear() where WebKit supports it (Safari 17.2+).
 * Older systems get a cubic-bezier with a similar overshoot; passing an
 * unsupported easing to animate() throws rather than falling back. */
const supportsLinear = Boolean(window.CSS?.supports?.("animation-timing-function", "linear(0, 1)"));
const EASE = {
  out: "cubic-bezier(0.23, 1, 0.32, 1)",
  inOut: "cubic-bezier(0.77, 0, 0.175, 1)",
  bloom: supportsLinear
    ? "linear(0, 0.0274, 0.0989, 0.1944, 0.3034, 0.4168, 0.5309, 0.6338, 0.726, 0.8082, 0.8749, 0.929, 0.9714, 1.0041, 1.0264, 1.0411, 1.0498, 1.0532, 1.053, 1.0501, 1.0454, 1.0398, 1.0338, 1.0278, 1.0219, 1.0167, 1.0121, 1.0081, 1.005, 1.0024, 1.0005, 0.999, 1)"
    : "cubic-bezier(0.3, 1.25, 0.5, 1)",
  pop: supportsLinear
    ? "linear(0, 0.0479, 0.1651, 0.3204, 0.4887, 0.6559, 0.7995, 0.9175, 1.0072, 1.0692, 1.1065, 1.1232, 1.124, 1.1133, 1.0959, 1.0753, 1.0543, 1.0349, 1.0182, 1.005, 0.9953, 0.9888, 0.9854, 0.9843, 0.9849, 0.9867, 0.9891, 0.9917, 0.9943, 0.9967, 0.9985, 1, 1)"
    : "cubic-bezier(0.34, 1.56, 0.64, 1)",
};

// Where each confetti piece flies on a completed task: angle (0 = right,
// positive is down), distance and spin. Kept to the right half so nothing
// leaves the window past the island's left edge.
const CONFETTI = [
  [-95, 30, 140], [-62, 40, -200], [-30, 46, 0], [-4, 42, 220], [22, 46, 160],
  [48, 40, 0], [74, 34, -180], [98, 28, 120], [-122, 22, 0], [118, 24, 90],
];

// Where each piece of the Samlu mark comes from when it snaps together, in
// the mark's own 240-unit space: the four quarters fly in from their corners
// and the small square drops in last.
const PIECES = [
  { id: "piece-qa", x: -70, y: -70, r: -90 },
  { id: "piece-sqb", x: 70, y: -70, r: 45 },
  { id: "piece-sqa", x: -70, y: 70, r: -45 },
  { id: "piece-qb", x: 70, y: 70, r: 90 },
  { id: "piece-foot", x: 0, y: -170, r: 0 },
];

let dismissTimer = null;
let remaining = 0;
let startedAt = 0;
let visible = false;
let leaving = false;
/** Counts renders. An exit or a flick that finds it changed while it awaited
 * knows a newer payload took the island back, and stops. */
let generation = 0;
/** The sequence number of the last payload rendered, handed back to the
 * native side so it can tell a stale hide from a current one. */
let lastSeq = 0;
let hovering = false;

/* ------------------------------------------------------------------ motion */

let running = [];

/** Plays a Web Animation and remembers it, so a new arrival can cancel
 * whatever the last one left behind (exits hold their end state until then,
 * which keeps a hidden island from flashing when its window reappears). */
function play(node, frames, options) {
  if (!node || typeof node.animate !== "function") return null;
  const animation = node.animate(frames, options);
  running.push(animation);
  if (options.fill !== "forwards" && options.fill !== "both") {
    animation.addEventListener("finish", () => {
      running = running.filter((item) => item !== animation);
    });
  }
  return animation;
}

function stopAll() {
  running.forEach((animation) => animation.cancel());
  running = [];
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const isBuddy = () => el.island.dataset.style === "buddy";
const hasQueue = () => el.island.dataset.queue === "true";
const islandHeight = () => (hasQueue() ? ISLAND_HEIGHT_QUEUE : ISLAND_HEIGHT_PLAIN);

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
  // Bloom narrows toward the notch as it is thrown up, as if drawn back in.
  const squeeze = !isBuddy() && value < 0 ? 1 - Math.min(1, -value / islandHeight()) * 0.16 : 1;
  el.island.style.setProperty("--drag-squeeze", squeeze.toFixed(4));
}

/* ------------------------------------------------------------- bloom style */

function arriveBloom(kind) {
  // The body grows out of the camera housing on a spring with a visible
  // overshoot; the content waits for the shape, then comes into focus.
  play(el.body, [{ transform: housingScale() }, { transform: "none" }], { duration: 500, easing: EASE.bloom });
  play(el.stage, [{ opacity: 0 }, { opacity: 1 }], { duration: 90, easing: "ease-out" });
  [el.slot, el.meta, el.summary, el.actions, el.cardActions, el.queueStep].forEach((node, i) => {
    play(
      node,
      [
        { opacity: 0, filter: "blur(6px)", transform: "translateY(-6px) scale(0.97)" },
        { opacity: 1, filter: "blur(0px)", transform: "none" },
      ],
      { duration: 340, delay: 80 + i * 45, easing: EASE.out, fill: "backwards" },
    );
  });
  accentBloom(kind, true);
}

function accentBloom(kind, celebrate) {
  if (kind === "completed") {
    play(el.glyph, [{ transform: "scale(0.72) rotate(-14deg)" }, { transform: "none" }], {
      duration: 530, delay: 120, easing: EASE.pop, fill: "backwards",
    });
    play(el.checkPath, [{ strokeDashoffset: 20 }, { strokeDashoffset: 0 }], {
      duration: 300, delay: 220, easing: EASE.out, fill: "backwards",
    });
    // Confetti only for a genuine arrival, never for a swap in place.
    if (celebrate) burst();
  } else if (kind === "needs_input") {
    play(el.glyph, [{ transform: "scale(0.8)" }, { transform: "none" }], {
      duration: 530, delay: 120, easing: EASE.pop, fill: "backwards",
    });
  } else {
    // Turns finish all day long: a small swoosh and nothing more.
    play(el.arrow, [{ transform: "translate(-5px, 5px)", opacity: 0 }, { transform: "none", opacity: 1 }], {
      duration: 420, delay: 150, easing: EASE.pop, fill: "backwards",
    });
  }
}

function burst() {
  el.burst.querySelectorAll(".confetti").forEach((node, i) => {
    const [angle, distance, spin] = CONFETTI[i % CONFETTI.length];
    const rad = (angle * Math.PI) / 180;
    const dx = Math.cos(rad) * distance;
    const dy = Math.sin(rad) * distance;
    play(
      node,
      [
        { opacity: 1, transform: "translate(0px, 0px) rotate(0deg) scale(0.6)" },
        { opacity: 1, transform: `translate(${dx}px, ${dy}px) rotate(${spin}deg) scale(1)`, offset: 0.55 },
        { opacity: 0, transform: `translate(${dx * 1.12}px, ${dy * 1.12 + 7}px) rotate(${spin * 1.3}deg) scale(0.8)` },
      ],
      { duration: 760, delay: 250 + i * 12, easing: "cubic-bezier(0.16, 1, 0.3, 1)" },
    );
  });
}

function exitBloom() {
  // Retraces the bloom: the content goes first, then the body is drawn back
  // up into the housing it came from. Quicker than the arrival.
  play(el.notification, [{ opacity: 1, filter: "blur(0px)" }, { opacity: 0, filter: "blur(4px)" }], {
    duration: 100, easing: "ease-out", fill: "forwards",
  });
  play(el.body, [{ transform: "none" }, { transform: housingScale() }], {
    duration: 230, delay: 30, easing: EASE.inOut, fill: "forwards",
  });
  play(el.stage, [{ opacity: 1 }, { opacity: 1, offset: 0.7 }, { opacity: 0 }], {
    duration: 260, easing: "linear", fill: "forwards",
  });
  return wait(270);
}

/* ------------------------------------------------------------- buddy style */

function assemble(lead) {
  PIECES.forEach((piece, i) => {
    const from = `translate(${piece.x}px, ${piece.y}px) rotate(${piece.r}deg) scale(0.4)`;
    const delay = lead + i * 45 + (piece.id === "piece-foot" ? 90 : 0);
    play(byId(piece.id), [{ opacity: 0, transform: from }, { opacity: 1, transform: "none" }], {
      duration: 530, delay, easing: EASE.pop, fill: "backwards",
    });
  });
}

function arriveBuddy(kind) {
  play(el.stage, [{ opacity: 0, transform: "translateY(-8px) scale(0.985)" }, { opacity: 1, transform: "none" }], {
    duration: 200, easing: EASE.out,
  });
  play(el.buddy, [{ opacity: 0, transform: "scale(0.86)" }, { opacity: 1, transform: "none" }], {
    duration: 530, delay: 40, easing: EASE.pop, fill: "backwards",
  });
  // Turns finish all day, so they skip the assembly and just appear.
  if (kind !== "turn_finished") assemble(110);
  [el.meta, el.summary, el.actions, el.cardActions, el.queueStep].forEach((node, i) => {
    play(node, [{ opacity: 0, transform: "translateY(-5px)" }, { opacity: 1, transform: "none" }], {
      duration: 300, delay: 90 + i * 45, easing: EASE.out, fill: "backwards",
    });
  });
  accentBuddy(kind, kind === "turn_finished" ? 160 : 520);
}

function accentBuddy(kind, lead) {
  restLean();
  if (kind === "completed") {
    play(
      el.mark,
      [
        { transform: "none" },
        { transform: "translateY(1px) scale(1.12, 0.88)", offset: 0.16 },
        { transform: "translateY(-7px) scale(0.94, 1.08)", offset: 0.44 },
        { transform: "translateY(0px) scale(1.08, 0.92)", offset: 0.7 },
        { transform: "none" },
      ],
      { duration: 620, delay: lead, easing: "ease-in-out" },
    );
    // The little square gets an extra bounce of its own on landing.
    play(el.foot, [{ transform: "none" }, { transform: "translateY(-46px)", offset: 0.4 }, { transform: "none" }], {
      duration: 420, delay: lead + 380, easing: "ease-out",
    });
    el.slot.querySelectorAll(".spark").forEach((node, i) => {
      play(
        node,
        [
          { opacity: 0, transform: "scale(0.3) rotate(0deg)" },
          { opacity: 1, transform: "scale(1.1) rotate(45deg)", offset: 0.45 },
          { opacity: 0, transform: "scale(0.6) rotate(90deg)" },
        ],
        { duration: 620, delay: lead + 220 + i * 70, easing: "ease-out" },
      );
    });
  } else if (kind === "needs_input") {
    // Two knocks on the glass; after that only the small square taps (in
    // CSS), like a foot keeping time while it waits.
    const knock = Math.max(0, lead - 260);
    play(
      el.buddy,
      [
        { transform: "none" },
        { transform: "rotate(-11deg) translateX(-1px)", offset: 0.2 },
        { transform: "rotate(4deg)", offset: 0.4 },
        { transform: "rotate(-11deg) translateX(-1px)", offset: 0.6 },
        { transform: "rotate(3deg)", offset: 0.8 },
        { transform: "none" },
      ],
      { duration: 700, delay: knock, easing: "ease-in-out" },
    );
    el.slot.querySelectorAll(".buddy-ripple").forEach((node, i) => {
      play(node, [{ opacity: 0.8, transform: "scale(0.9)" }, { opacity: 0, transform: "scale(1.5)" }], {
        duration: 440, delay: knock + 140 + i * 280, easing: "ease-out",
      });
    });
  } else {
    play(el.buddy, [{ transform: "none" }, { transform: "rotate(12deg) translateY(1px)", offset: 0.35 }, { transform: "none" }], {
      duration: 440, delay: lead, easing: "ease-in-out",
    });
    play(el.foot, [{ transform: "none" }, { transform: "translateY(-30px)", offset: 0.4 }, { transform: "none" }], {
      duration: 360, delay: lead + 120, easing: "ease-out",
    });
  }
}

function exitBuddy() {
  // The pieces lift off one after another as the island goes back up.
  PIECES.slice().reverse().forEach((piece, i) => {
    play(byId(piece.id), [{ opacity: 1, transform: "none" }, { opacity: 0, transform: "translateY(-60px)" }], {
      duration: 140, delay: i * 14, easing: "ease-out", fill: "forwards",
    });
  });
  play(el.stage, [{ opacity: 1, transform: "none" }, { opacity: 0, transform: "translateY(-14px) scale(0.985)" }], {
    duration: 170, delay: 30, easing: EASE.out, fill: "forwards",
  });
  return wait(210);
}

let boops = [];

function boop() {
  if (leaving) return;
  const now = performance.now();
  boops = boops.filter((time) => now - time < 2200);
  boops.push(now);
  if (reducedMotion.matches) return;
  if (boops.length >= 4) {
    // Booped enough: the mark comes apart, spins, and snaps back together.
    boops = [];
    play(
      el.mark,
      [
        { transform: "rotate(0deg)" },
        { transform: "rotate(-25deg) scale(0.92)", offset: 0.15 },
        { transform: "rotate(380deg) scale(1.06)", offset: 0.8 },
        { transform: "rotate(360deg)" },
      ],
      { duration: 720, easing: "ease-in-out" },
    );
    assemble(520);
    return;
  }
  play(el.buddy, [{ transform: "scale(1.16, 0.84)" }, { transform: "none" }], { duration: 530, easing: EASE.pop });
  // Each piece jumps a little away from the middle and springs back.
  PIECES.forEach((piece) => {
    const dx = Math.sign(piece.x) * 16;
    const dy = (piece.id === "piece-foot" ? 1 : Math.sign(piece.y)) * 16;
    play(byId(piece.id), [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], {
      duration: 530, easing: EASE.pop,
    });
  });
}

/* The mark leans toward the pointer while it is over the island, which is
 * all a non-activating window can see of it. */
const lean = { r: 0, x: 0, y: 0 };
let leanTarget = { r: 0, x: 0, y: 0 };
let leanFrame = 0;

function leanTo(r, x, y) {
  leanTarget = { r, x, y };
  if (leanFrame) return;
  const step = () => {
    const factor = reducedMotion.matches ? 1 : 0.18;
    lean.r += (leanTarget.r - lean.r) * factor;
    lean.x += (leanTarget.x - lean.x) * factor;
    lean.y += (leanTarget.y - lean.y) * factor;
    el.lean.style.transform = `translate(${lean.x.toFixed(2)}px, ${lean.y.toFixed(2)}px) rotate(${lean.r.toFixed(2)}deg)`;
    const settled =
      Math.abs(leanTarget.r - lean.r) < 0.03 &&
      Math.abs(leanTarget.x - lean.x) < 0.02 &&
      Math.abs(leanTarget.y - lean.y) < 0.02;
    leanFrame = settled ? 0 : requestAnimationFrame(step);
  };
  leanFrame = requestAnimationFrame(step);
}

function leanAt(clientX, clientY) {
  const rect = el.buddy.getBoundingClientRect();
  const dx = clientX - (rect.left + rect.width / 2);
  const dy = clientY - (rect.top + rect.height / 2);
  const clamp = (value, limit) => Math.max(-limit, Math.min(limit, value));
  leanTo(clamp(dx * 0.05, 9), clamp(dx * 0.02, 2.5), clamp(dy * 0.03, 2));
}

/** At rest the mark stands upright, except while an agent is waiting: then
 * it leans toward the button that answers it. */
function restLean() {
  if (el.island.dataset.kind === "needs_input") leanTo(5, 1, 0);
  else leanTo(0, 0, 0);
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

/** Plays the style's exit, then asks the native side to hide the window.
 * Ordering matters: hiding first would make every dismissal a hard cut. */
function playExit() {
  if (reducedMotion.matches) {
    play(el.stage, [{ opacity: 1 }, { opacity: 0 }], { duration: 140, easing: "ease-out", fill: "forwards" });
    return wait(150);
  }
  return isBuddy() ? exitBuddy() : exitBloom();
}

async function leave(command) {
  if (leaving) return;
  leaving = true;
  const turn = generation;
  clearDismissTimer();
  cancelAnimationFrame(settleFrame);
  await playExit();
  if (turn !== generation) return;
  visible = false;
  await invoke(command, { seq: lastSeq }).catch(console.error);
}

function dismiss() {
  return leave("agent_island_dismiss");
}

/* ------------------------------------------------------------------ render */

/** The queue the payload carries, or null when it is empty or absent. */
function activeQueue(payload) {
  const queue = payload?.queue;
  if (!queue || !Array.isArray(queue.cards) || queue.cards.length === 0) return null;
  const front = Math.min(Math.max(Number(queue.front) || 0, 0), queue.cards.length - 1);
  return { cards: queue.cards, front, card: queue.cards[front] };
}

/** Cards reuse the island's two visual moods: anything that needs an answer
 * wears the waiting accent; a handed-back turn is as quiet as a finished one. */
const CARD_MOOD = { approval: "needs_input", needs_input: "needs_input", handed_back: "turn_finished" };
const effectiveKind = (payload) => {
  const queue = activeQueue(payload);
  return queue ? CARD_MOOD[queue.card.kind] || "needs_input" : payload.kind || "completed";
};

let noteTimer = null;
let summaryText = "";

function setSummary(text) {
  summaryText = text;
  clearTimeout(noteTimer);
  noteTimer = null;
  el.summary.textContent = text;
}

/** A short message in place of the summary line, then the summary again. */
function showNote(note) {
  if (typeof note !== "string" || !note || !visible) return;
  clearTimeout(noteTimer);
  el.summary.textContent = note;
  el.summary.removeAttribute("title");
  noteTimer = setTimeout(() => {
    noteTimer = null;
    el.summary.textContent = summaryText;
    if (el.summary.dataset.full) el.summary.title = el.summary.dataset.full;
  }, NOTE_MS);
}

function button(label, className, onClick, title) {
  const node = document.createElement("button");
  node.type = "button";
  node.className = className;
  node.textContent = label;
  if (title) {
    node.title = title;
    node.setAttribute("aria-label", label);
  }
  node.addEventListener("click", onClick);
  return node;
}

function setCardButtons(disabled) {
  el.cardActions.querySelectorAll("button").forEach((node) => {
    node.disabled = disabled;
  });
}

/** Sends a card action, then leaves the island as it is: the next payload
 * decides what comes next. Buttons stay dead until that payload lands. */
function cardAction(cardId, action) {
  if (el.cardActions.dataset.busy === "true") return;
  el.cardActions.dataset.busy = "true";
  setCardButtons(true);
  const revive = () => {
    delete el.cardActions.dataset.busy;
    setCardButtons(false);
  };
  invoke("agent_card_action", { cardId, action })
    .then(() => {
      // View sends no new payload for a Claude card, which stays pending:
      // its buttons must not stay dead.
      if (action === "view") revive();
    })
    .catch((error) => {
      console.error(error);
      revive();
    });
}

function writeCard(queue) {
  const { card, cards, front } = queue;
  const act = (action) => () => cardAction(card.id, action);
  el.title.textContent = card.title || "Agent update";
  el.agent.textContent = card.agentLabel || card.agent || "Agent";
  el.project.textContent = card.project || "project";

  const detail = card.kind === "handed_back" ? "Answer in Codex" : card.detail || "";
  setSummary(detail);
  el.summary.dataset.mono = String(card.kind === "approval");
  if (detail) {
    el.summary.dataset.full = detail;
    el.summary.title = detail;
  } else {
    delete el.summary.dataset.full;
    el.summary.removeAttribute("title");
  }

  el.cardActions.replaceChildren();
  delete el.cardActions.dataset.busy;
  el.cardActions.dataset.card = card.kind;
  if (card.kind === "approval") {
    el.cardActions.append(button("Deny", "card-button secondary", act("deny")));
    if (card.alwaysAllowRule) {
      el.cardActions.append(button("Always allow", "card-button secondary", act("always"), card.alwaysAllowRule));
    }
    el.cardActions.append(button("Approve", "card-button primary", act("approve")));
    el.cardActions.append(button("View", "card-button text", act("view")));
  } else {
    el.cardActions.append(button("Dismiss", "card-button secondary", act("dismiss")));
    el.cardActions.append(button("View", "card-button primary", act("view")));
  }

  el.queueStep.hidden = cards.length < 2;
  el.queueCount.textContent = `${front + 1} of ${cards.length}`;
}

function writeContent(payload) {
  const queue = activeQueue(payload);
  el.island.dataset.kind = effectiveKind(payload);
  el.island.dataset.style = payload.style === "buddy" ? "buddy" : "bloom";
  el.island.dataset.notch = String(Boolean(payload.hasNotch));
  el.island.dataset.queue = String(Boolean(queue));
  el.island.dataset.persistent = String(Boolean(payload.persistent) || Boolean(queue));
  if (queue) {
    writeCard(queue);
    return;
  }
  el.cardActions.replaceChildren();
  el.queueStep.hidden = true;
  delete el.summary.dataset.mono;
  delete el.summary.dataset.full;
  el.summary.removeAttribute("title");
  el.title.textContent = payload.title || "Agent update";
  el.agent.textContent = payload.agent || "Agent";
  el.project.textContent = payload.project || "project";
  setSummary(payload.summary || "The agent has an update.");
}

function restartLifetime(payload) {
  const kind = effectiveKind(payload);
  const lifetime = kind === "turn_finished" ? 6000 : 7000;
  el.island.style.removeProperty("--timeout-play-state");
  el.island.style.setProperty("--timeout", `${lifetime / 1000}s`);
  // The depleting hairline lives on a pseudo-element, so the only way to
  // replay it for a second update is to hand it a fresh node.
  const fresh = el.track.cloneNode(true);
  el.track.replaceWith(fresh);
  el.track = fresh;
  clearDismissTimer();
  if (el.island.dataset.persistent !== "true") {
    scheduleDismiss(lifetime);
    if (hovering) pauseDismiss();
  }
}

/** A second update arriving while the island is still on screen swaps its
 * contents in place. Replaying the arrival would mean re-entering something
 * that never left. */
function swap(payload) {
  if (reducedMotion.matches) {
    writeContent(payload);
    restartLifetime(payload);
    restLean();
    return;
  }
  const buddy = payload.style === "buddy";
  const out = play(
    el.copy,
    [{ opacity: 1, filter: "blur(0px)", transform: "none" }, { opacity: 0, filter: "blur(4px)", transform: "translateY(-4px)" }],
    { duration: SWAP_MS, easing: "ease-out", fill: "forwards" },
  );
  let glyphOut = null;
  if (buddy) {
    // A double take: the mark pops up while the words change.
    play(el.buddy, [{ transform: "none" }, { transform: "translateY(-4px) scale(1.07)", offset: 0.4 }, { transform: "none" }], {
      duration: 300, easing: "ease-out",
    });
  } else {
    // The island gulps the new update in.
    glyphOut = play(el.glyph, [{ opacity: 1, transform: "none" }, { opacity: 0, transform: "scale(0.8)" }], {
      duration: SWAP_MS, easing: "ease-out", fill: "forwards",
    });
    play(el.body, [{ transform: "none" }, { transform: "scale(1.012, 1.045)", offset: 0.35 }, { transform: "none" }], {
      duration: 420, easing: "ease-in-out",
    });
  }
  setTimeout(() => {
    out?.cancel();
    glyphOut?.cancel();
    writeContent(payload);
    play(
      el.copy,
      [{ opacity: 0, filter: "blur(4px)", transform: "translateY(4px)" }, { opacity: 1, filter: "blur(0px)", transform: "none" }],
      { duration: 240, easing: EASE.out },
    );
    if (buddy) accentBuddy(effectiveKind(payload), 200);
    else accentBloom(effectiveKind(payload), false);
    restartLifetime(payload);
  }, SWAP_MS);
}

/** Which card is showing, so an identical re-send does not replay a swap. */
let shownCard = "";
const cardSignature = (payload) => {
  const queue = activeQueue(payload);
  if (!queue) return "";
  const { card, cards, front } = queue;
  return JSON.stringify([card.id, card.kind, card.title, card.detail, card.alwaysAllowRule, cards.length, front]);
};

function render(payload) {
  if (payload.seq > lastSeq) lastSeq = payload.seq;
  // The queue emptied: the native side is done with the island.
  if (payload.kind === "dismiss" && !activeQueue(payload)) {
    if (visible) dismiss();
    return;
  }
  generation += 1;
  cancelAnimationFrame(settleFrame);
  setOffset(0);
  if (leaving) {
    // The island was on its way out when this arrived. Whatever the exit
    // had done is stale: start over with a fresh arrival, not a swap.
    stopAll();
    visible = false;
    shownCard = "";
  }
  leaving = false;

  const signature = cardSignature(payload);
  if (visible) {
    if (signature && signature === shownCard) {
      // Same card re-sent after an action: nothing to animate, but the
      // buttons must come back to life.
      delete el.cardActions.dataset.busy;
      setCardButtons(false);
      return;
    }
    shownCard = signature;
    swap(payload);
    return;
  }
  shownCard = signature;

  stopAll();
  writeContent(payload);
  el.stage.dataset.ready = "true";
  visible = true;
  const kind = effectiveKind(payload);
  if (reducedMotion.matches) {
    play(el.stage, [{ opacity: 0 }, { opacity: 1 }], { duration: 160, easing: "ease-out" });
    restLean();
  } else if (isBuddy()) {
    arriveBuddy(kind);
  } else {
    arriveBloom(kind);
  }
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
  if (event.target.closest("button")) return; // let the actions and the mark win
  dragging = true;
  grabbedAt = event.clientY;
  samples = [{ y: event.clientY, t: event.timeStamp }];
  cancelAnimationFrame(settleFrame);
  el.island.dataset.dragging = "true";
  el.island.setPointerCapture(event.pointerId);
  pauseDismiss();
});

el.island.addEventListener("pointermove", (event) => {
  if (!dragging) {
    if (isBuddy()) leanAt(event.clientX, event.clientY);
    return;
  }
  const travel = event.clientY - grabbedAt;
  // Upward tracks the pointer exactly; downward resists, since the island is
  // already as far down as it goes.
  offset = travel < 0 ? travel : rubberband(travel, islandHeight());
  setOffset(offset);
  if (isBuddy()) leanTo(0, 0, travel < 0 ? -2 : 1.5);
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

  if (projected < -islandHeight() * DISMISS_FRACTION) {
    // Thrown back into the notch: carry the release velocity into the exit so
    // there is no seam between the finger and the animation.
    leaving = true;
    const turn = generation;
    clearDismissTimer();
    const target = -(islandHeight() + 24);
    await spring(offset, target, velocity, setOffset, { response: 0.28 });
    if (turn !== generation) return;
    play(el.stage, [{ opacity: 1 }, { opacity: 0 }], { duration: 80, easing: "linear", fill: "forwards" });
    await wait(90);
    if (turn !== generation) return;
    visible = false;
    await invoke("agent_island_dismiss", { seq: lastSeq }).catch(() => {});
    return;
  }

  await spring(offset, 0, velocity, setOffset);
  offset = 0;
  if (isBuddy()) restLean();
  if (!hovering) resumeDismiss();
}

el.island.addEventListener("pointerup", endDrag);
el.island.addEventListener("pointercancel", endDrag);

/* ------------------------------------------------------------------ actions */

byId("dismiss").addEventListener("click", () => {
  dismiss();
});

byId("open").addEventListener("click", () => {
  leave("agent_island_open_activity");
});

byId("queue-prev").addEventListener("click", () => {
  invoke("agent_queue_step", { delta: -1 }).catch(console.error);
});
byId("queue-next").addEventListener("click", () => {
  invoke("agent_queue_step", { delta: 1 }).catch(console.error);
});

el.buddy.addEventListener("click", boop);

el.island.addEventListener("mouseenter", () => {
  hovering = true;
  pauseDismiss();
});
el.island.addEventListener("mouseleave", () => {
  hovering = false;
  if (dragging) return;
  resumeDismiss();
  if (isBuddy()) restLean();
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) pauseDismiss();
  else if (!hovering) resumeDismiss();
});

/* --------------------------------------------------------------- appearance */

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

/** `island.html?demo=approval`: a static two-card queue for checking the layout
 * without the native side. */
const DEMO_APPROVAL = {
  kind: "needs_input",
  style: "bloom",
  hasNotch: false,
  persistent: true,
  title: "",
  agent: "Claude",
  project: "samlu",
  summary: "",
  queue: {
    front: 0,
    cards: [
      {
        id: "demo-1", kind: "approval", agent: "claude", agentLabel: "Claude", sessionId: "s1",
        project: "samlu", title: "Run a command", detail: "npm run build -- --mode production --sourcemap",
        requestId: "r1", alwaysAllowRule: "Bash(npm run build:*)", createdAt: 0,
      },
      {
        id: "demo-2", kind: "needs_input", agent: "codex", agentLabel: "Codex", sessionId: "s2",
        project: "docs", title: "Waiting for you", detail: "Which branch should the release go out from?",
        requestId: null, alwaysAllowRule: null, createdAt: 0,
      },
    ],
  },
};

async function boot() {
  invoke("get_appearance").then(applyAppearance).catch(() => {});
  listen("appearance://changed", (event) => applyAppearance(event.payload)).catch(() => {});
  await listen("island://show", (event) => render(event.payload));
  await listen("island://note", (event) => showNote(event.payload));
  if (new URLSearchParams(location.search).get("demo") === "approval") {
    render(DEMO_APPROVAL);
    return;
  }
  await invoke("agent_island_ready");
}

boot().catch(console.error);
