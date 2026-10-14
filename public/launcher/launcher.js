/* Samlu's keyboard-driven developer launcher.
 *
 * Lifecycle: Rust emits `launcher://present` while the window is still
 * hidden, and the page resets to a fresh, closed panel. The entrance plays
 * when the window gains focus. `launcher://dismiss` plays the exit, after
 * which Rust hides the window; `launcher://reopen` arrives if the hotkey is
 * pressed again mid-exit, and the panel simply turns back around. */

const tauri = window.__TAURI__ || {};
const invoke = tauri.core?.invoke || (async () => null);
const listen = tauri.event?.listen || (async () => () => {});

window.addEventListener("error", (event) => console.error("[launcher]", event.error || event.message));

const el = {
  stage: document.getElementById("stage"),
  panel: document.getElementById("panel"),
  query: document.getElementById("query"),
  results: document.getElementById("results"),
  hotkey: document.getElementById("hotkey"),
  action: document.getElementById("action"),
  legendDescribe: document.getElementById("legend-describe"),
};

// Geometry, mirrored from launcher.css: search field with its divider, the
// footer, and at most eight rows of results with the list's inset. The window
// keeps a margin above the panel and room below it for the shadow.
const SEARCH_H = 57;
const FOOTER_H = 36;
const MAX_BODY_H = 6 + 8 * 44 + 6;
const MAX_RESULTS = 18;
const STAGE_TOP = 16;
const STAGE_BOTTOM = 76;
// Long enough for the height spring (--launcher-resize) to settle.
const RESIZE_SETTLE_MS = 300;
// Descriptive Find, after DigUp's panel: words match while you type, and
// once typing pauses this long the meaning results take over.
const TYPING_PAUSE_MS = 120;
// In an ordinary search, at most this many strong in-file matches join the
// end of the list; `?` searches inside files only.
const MAX_BLENDED_CONTENT = 4;
const DESCRIBE_PREFIX = "?";

const KINDS = {
  app: { label: "Application", verb: "Open" },
  project: { label: "Project", verb: "Open Folder" },
  file: { label: "File", verb: "Open" },
  action: { label: "Command", verb: "Run" },
  snippet: { label: "Snippet", verb: "Copy" },
  clipboard: { label: "Clipboard", verb: "Copy" },
  calculation: { label: "Calculator", verb: "Copy Result" },
  content: { label: "In File", verb: "Open" },
};
const REVEALABLE = new Set(["app", "file", "project", "content"]);

const SVG = (body) => `<svg viewBox="0 0 16 16" aria-hidden="true">${body}</svg>`;
const ICONS = {
  app: SVG('<rect x="2.5" y="2.5" width="11" height="11" rx="3"/><path d="M5.5 8h5M8 5.5v5"/>'),
  project: SVG('<path d="M2 4.6c0-.9.7-1.6 1.6-1.6h2.6l1.4 1.5h4.8c.9 0 1.6.7 1.6 1.6v5.3c0 .9-.7 1.6-1.6 1.6H3.6c-.9 0-1.6-.7-1.6-1.6Z"/>'),
  file: SVG('<path d="M4 2h5l3 3v8.4c0 .3-.3.6-.6.6H4.6a.6.6 0 0 1-.6-.6V2.6c0-.3.3-.6.6-.6Z"/><path d="M9 2v3h3"/>'),
  content: SVG('<path d="M4 2h5l3 3v8.4c0 .3-.3.6-.6.6H4.6a.6.6 0 0 1-.6-.6V2.6c0-.3.3-.6.6-.6Z"/><path d="M6 8h4M6 10.5h3"/>'),
  open_launcher_settings: SVG('<circle cx="7" cy="7" r="3.5"/><path d="m9.6 9.6 3.4 3.4"/>'),
  snippet: SVG('<path d="M6 3.5 3.5 8 6 12.5M10 3.5 12.5 8 10 12.5"/>'),
  clipboard: SVG('<rect x="3.5" y="3" width="9" height="11" rx="1.5"/><path d="M6 3V2h4v1"/>'),
  open_settings: SVG('<circle cx="8" cy="8" r="2"/><path d="M8 1.8v1.6M8 12.6v1.6M3.6 3.6l1.1 1.1M11.3 11.3l1.1 1.1M1.8 8h1.6M12.6 8h1.6M3.6 12.4l1.1-1.1M11.3 4.7l1.1-1.1"/>'),
  color_picker: SVG('<path d="M10.2 2.6a1.7 1.7 0 0 1 2.4 2.4l-1.4 1.4-2.4-2.4Z"/><path d="m8.8 4 3.2 3.2M9.6 4.8 3.4 11l-.9 2.5 2.5-.9 6.2-6.2"/>'),
  manage_snippets: SVG('<path d="M6 3.5 3.5 8 6 12.5M10 3.5 12.5 8 10 12.5"/>'),
  open_terminal: SVG('<rect x="2" y="3" width="12" height="10" rx="1.6"/><path d="m4.8 6.5 1.8 1.5-1.8 1.5M8.4 10h2.8"/>'),
  check_for_updates: SVG('<path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5v2.8h-2.8"/>'),
  action: SVG('<path d="M5.5 3.5h5M5.5 8h5M5.5 12.5h5"/>'),
};
const GLYPHS = { calculation: "=" };

const HOTKEY_SYMBOLS = {
  cmd: "⌘",
  command: "⌘",
  super: "⌘",
  meta: "⌘",
  ctrl: "⌃",
  control: "⌃",
  alt: "⌥",
  option: "⌥",
  shift: "⇧",
};

let results = [];
let selectedIndex = 0;
let generation = 0;
let slowTimer = null;
let findTimer = null;
// Whether Descriptive Find is on, from its last answer; the `?` legend shows
// only then.
let describeEnabled = false;
// Set while `?` mode is waiting on its first answer, so the empty state
// doesn't flash "no results" before anything could arrive.
let describePending = false;
let describeWarming = false;
let openTimer = null;
let panelHeight = 0;
let windowHeight = 0;
let fitSequence = 0;
let shrinkTimer = null;
let sizeInstantly = true;
let commandHeld = false;
const iconCache = new Map();

/* -------------------------------------------------------------- appearance */

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

invoke("get_appearance").then(applyAppearance).catch(() => {});
listen("appearance://changed", (event) => applyAppearance(event.payload)).catch(() => {});

function renderHotkey(hotkey) {
  el.hotkey.replaceChildren(
    ...String(hotkey || "")
      .split("+")
      .map((part) => part.trim())
      .filter(Boolean)
      .map((part) => {
        const key = document.createElement("kbd");
        key.textContent = HOTKEY_SYMBOLS[part.toLowerCase()] || part.toUpperCase();
        return key;
      }),
  );
}

function refreshHotkey() {
  invoke("get_launcher_hotkey").then(renderHotkey).catch(() => {});
}

/* ------------------------------------------------------------------ search */

function scheduleSearch(query) {
  const current = ++generation;
  clearTimeout(slowTimer);
  clearTimeout(findTimer);
  const trimmed = query.trim();
  if (trimmed.startsWith(DESCRIBE_PREFIX)) {
    scheduleDescribe(trimmed.slice(DESCRIBE_PREFIX.length).trim(), current);
    return;
  }
  describePending = false;
  runFastSearch(query, current);
  if (trimmed.length >= 2 && !/^[=;@>]/.test(trimmed)) {
    slowTimer = setTimeout(() => runSlowSearch(query, current), 160);
  }
  if (trimmed.length >= 3 && !/^[=;@>]/.test(trimmed)) {
    findTimer = setTimeout(() => runBlendedFind(trimmed, current), 160 + TYPING_PAUSE_MS);
  }
}

/* ------------------------------------------------------ descriptive find */

async function findInFiles(query, meaning, limit) {
  try {
    const reply = await invoke("launcher_find_in_files", { query, meaning, limit });
    if (reply) {
      describeEnabled = reply.enabled;
      el.legendDescribe.hidden = !describeEnabled;
    }
    return reply;
  } catch (error) {
    console.error("[launcher] launcher_find_in_files failed", error);
    return null;
  }
}

/** `?` mode: every keystroke gets the word matches at once (the last word as
 * a prefix), and the meaning results replace them once typing pauses. */
function scheduleDescribe(query, current) {
  if (!query) {
    describePending = false;
    results = [];
    selectedIndex = 0;
    render();
    return;
  }
  describePending = true;
  findInFiles(query, false, 24).then((reply) => {
    if (current !== generation || !reply) return;
    showDescribe(reply, current, false);
  });
  findTimer = setTimeout(async () => {
    const reply = await findInFiles(query, true, 24);
    if (current !== generation || !reply) return;
    describePending = false;
    showDescribe(reply, current, true);
  }, TYPING_PAUSE_MS);
}

function showDescribe(reply, current, final) {
  if (!reply.enabled) {
    describePending = false;
    results = [
      {
        id: "action:open_launcher_settings",
        kind: "action",
        title: "Turn On Descriptive Find",
        subtitle: "Search inside your files by describing them, in Launcher settings",
        target: "open_launcher_settings",
      },
    ];
  } else {
    // Keyword results never replace meaning results that already arrived.
    if (!final && !describePending) return;
    describeWarming = final && reply.warming;
    results = reply.results.slice(0, MAX_RESULTS);
  }
  selectedIndex = 0;
  render();
}

/** An ordinary search: a few strong in-file matches join the end. */
async function runBlendedFind(query, current) {
  const reply = await findInFiles(query, true, 12);
  if (current !== generation || !reply?.results?.length) return;
  const strong = reply.results.slice(0, Math.min(reply.strong, MAX_BLENDED_CONTENT));
  if (!strong.length) return;
  const selectedId = results[selectedIndex]?.id;
  // A file already listed by name stays where it is.
  const listed = new Set(results.map((result) => result.target));
  results = results.concat(strong.filter((result) => !listed.has(result.target))).slice(0, MAX_RESULTS);
  const kept = results.findIndex((result) => result.id === selectedId);
  selectedIndex = kept >= 0 ? kept : 0;
  render({ keepScroll: true });
}

async function runFastSearch(query, current) {
  let fast = [];
  try {
    fast = (await invoke("launcher_search", { query })) || [];
  } catch (error) {
    console.error("[launcher] launcher_search failed", error);
  }
  if (current !== generation) return; // a newer keystroke has taken over
  results = fast.slice(0, MAX_RESULTS);
  selectedIndex = 0;
  render();
}

async function runSlowSearch(query, current) {
  let slow = [];
  try {
    slow = (await invoke("launcher_search_files", { query })) || [];
  } catch (error) {
    console.error("[launcher] launcher_search_files failed", error);
  }
  if (current !== generation || !slow.length) return;
  // Files arrive a moment after everything else. They join the end of the
  // list, and whatever is selected stays selected.
  const selectedId = results[selectedIndex]?.id;
  const seen = new Set(results.map((result) => result.id));
  results = results.concat(slow.filter((result) => !seen.has(result.id))).slice(0, MAX_RESULTS);
  const kept = results.findIndex((result) => result.id === selectedId);
  selectedIndex = kept >= 0 ? kept : 0;
  render({ keepScroll: true });
}

/* ------------------------------------------------------------------ render */

function iconFor(result) {
  if (result.kind === "app") {
    const cached = iconCache.get(result.target);
    if (cached) {
      const image = document.createElement("img");
      image.className = "result-icon";
      image.src = cached;
      image.alt = "";
      return image;
    }
  }
  const tile = document.createElement("span");
  tile.className = "result-tile";
  tile.setAttribute("aria-hidden", "true");
  if (result.kind === "app") tile.dataset.iconFor = result.target;
  if (GLYPHS[result.kind]) {
    tile.classList.add("glyph");
    tile.textContent = GLYPHS[result.kind];
  } else {
    tile.innerHTML =
      result.kind === "action"
        ? ICONS[result.target] || ICONS.action
        : ICONS[result.kind] || ICONS.action;
  }
  return tile;
}

/** Swaps placeholder tiles for the app's real icon once it is extracted. The
 * first time an icon is fetched it fades up; after that it is cached and
 * appears with its row. The list may have been redrawn while the icon was on
 * its way, so whichever placeholders are on screen when it lands are swapped. */
function loadAppIcon(result) {
  if (iconCache.has(result.target)) return;
  iconCache.set(result.target, null);
  invoke("launcher_app_icon", { path: result.target })
    .then((dataUrl) => {
      if (!dataUrl) {
        iconCache.delete(result.target);
        return;
      }
      iconCache.set(result.target, dataUrl);
      el.results.querySelectorAll(".result-tile[data-icon-for]").forEach((placeholder) => {
        if (placeholder.dataset.iconFor !== result.target) return;
        const image = document.createElement("img");
        image.className = "result-icon";
        image.alt = "";
        image.dataset.fresh = "true";
        image.addEventListener(
          "load",
          () => requestAnimationFrame(() => delete image.dataset.fresh),
          { once: true },
        );
        image.src = dataUrl;
        placeholder.replaceWith(image);
      });
    })
    .catch(() => iconCache.delete(result.target));
}

/** A passage's matched words arrive wrapped in « »; they're shown as <mark>
 * without ever parsing the file's text as HTML. */
function appendMarked(target, value) {
  value.split(/(«[^»]*»)/).forEach((part) => {
    if (!part) return;
    if (part.startsWith("«") && part.endsWith("»")) {
      const mark = document.createElement("mark");
      mark.textContent = part.slice(1, -1);
      target.append(mark);
    } else {
      // A marker cut off at the end of the excerpt is dropped.
      target.append(document.createTextNode(part.replace(/[«»]/g, "")));
    }
  });
}

function renderRow(result, index) {
  const row = document.createElement("li");
  row.id = `result-${index}`;
  row.className = "result";
  row.dataset.index = String(index);
  row.dataset.kind = result.kind;
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", "false");

  const icon = iconFor(result);
  const text = document.createElement("span");
  text.className = "result-text";
  const title = document.createElement("span");
  title.className = "result-title";
  title.textContent = result.title;
  text.append(title);
  if (result.subtitle) {
    const subtitle = document.createElement("span");
    subtitle.className = "result-subtitle";
    if (result.kind === "content") {
      appendMarked(subtitle, result.subtitle);
      subtitle.title = result.subtitle.replace(/[«»]/g, "");
    } else {
      subtitle.textContent = result.subtitle;
    }
    text.append(subtitle);
  }
  const kind = document.createElement("span");
  kind.className = "result-kind";
  kind.textContent = KINDS[result.kind]?.label || "";

  row.append(icon, text, kind);
  if (result.kind === "app" && !iconCache.get(result.target)) loadAppIcon(result);

  // Hover moves the selection only when the pointer actually moves, so a
  // list re-rendering under a resting pointer never steals it.
  row.addEventListener("pointermove", () => setSelected(index));
  row.addEventListener("click", (event) => activate(index, event.metaKey));
  return row;
}

function renderEmpty() {
  const empty = document.createElement("li");
  empty.className = "empty";
  empty.setAttribute("role", "presentation");
  const headline = document.createElement("p");
  const hint = document.createElement("span");
  const query = el.query.value.trim();
  if (query.startsWith("=")) {
    headline.textContent = "Type an expression";
    hint.textContent = "For example = 12 * (8 + 2). Return copies the result.";
  } else if (query.startsWith(";")) {
    headline.textContent = "No snippets yet";
    hint.textContent = "Create one with the Manage Snippets command.";
  } else if (query.startsWith(DESCRIBE_PREFIX)) {
    const description = query.slice(DESCRIBE_PREFIX.length).trim();
    if (!description) {
      headline.textContent = "Describe what you’re looking for";
      hint.textContent = "For example ? the clause about pets in the lease. Searches inside your files.";
    } else if (describePending) {
      headline.textContent = "Searching inside your files…";
      hint.textContent = "Matching words now; meaning follows when you pause.";
    } else {
      headline.textContent = `Nothing in your files matches “${description}”`;
      hint.textContent = describeWarming
        ? "Matched words only while meaning search warms up. Pause again in a moment."
        : "Try other words, or add folders in Launcher settings.";
    }
  } else if (query.startsWith("@")) {
    headline.textContent = "Nothing in clipboard history";
    hint.textContent = "Text you copy appears here while clipboard history is on.";
  } else if (query) {
    headline.textContent = `No results for “${query}”`;
    hint.textContent = describeEnabled
      ? "Search apps, projects, and files by name, start with ? to describe a file’s contents, or = ; @ > for tools."
      : "Search apps, projects, and files by name, or start with = ; @ or > for tools.";
  } else {
    headline.textContent = "Start typing to search";
    hint.textContent = "Apps, projects, and files, plus = ; @ and > for tools.";
  }
  empty.append(headline, hint);
  return empty;
}

/** Redraws the list. New results start at the top; results that only grew
 * (files arriving late) keep the scroll position and the selection in view. */
function render({ keepScroll = false } = {}) {
  const body = el.results.parentElement;
  const scrollTop = body.scrollTop;
  const fragment = document.createDocumentFragment();
  let section = null;
  results.forEach((result, index) => {
    if (result.section && result.section !== section) {
      section = result.section;
      const heading = document.createElement("li");
      heading.className = "section";
      heading.setAttribute("role", "presentation");
      heading.textContent = section;
      fragment.append(heading);
    }
    fragment.append(renderRow(result, index));
  });
  if (!results.length) fragment.append(renderEmpty());
  el.results.replaceChildren(fragment);
  body.scrollTop = keepScroll ? scrollTop : 0;
  updateSelection(keepScroll);
  fitPanel();
}

/** Sizes the panel to its content. The height is a CSS spring, so a list
 * growing or shrinking as you type glides instead of jumping. The window
 * follows out of sight: it grows before the panel does and shrinks after the
 * panel has settled, so the transparent window around the panel — which still
 * takes clicks — is never more than the shadow's margin. */
async function fitPanel() {
  const body = Math.min(MAX_BODY_H, el.results.offsetHeight);
  const height = SEARCH_H + body + FOOTER_H;
  const sequence = ++fitSequence;
  const needed = STAGE_TOP + height + STAGE_BOTTOM;
  const resting = !el.query.value;
  clearTimeout(shrinkTimer);
  if (needed > windowHeight) {
    windowHeight = needed;
    await invoke("launcher_resize", { height: needed, resting }).catch(() => {});
    // A newer render took over while the window was growing.
    if (sequence !== fitSequence) return;
  } else if (needed < windowHeight) {
    shrinkTimer = setTimeout(
      () => {
        windowHeight = needed;
        invoke("launcher_resize", { height: needed, resting }).catch(() => {});
      },
      sizeInstantly ? 0 : RESIZE_SETTLE_MS,
    );
  }
  if (height === panelHeight) return;
  panelHeight = height;
  if (sizeInstantly) {
    el.panel.classList.add("instant");
    el.panel.style.height = `${height}px`;
    void el.panel.offsetHeight;
    el.panel.classList.remove("instant");
  } else {
    el.panel.style.height = `${height}px`;
  }
}

function rows() {
  return el.results.querySelectorAll(".result");
}

function updateSelection(scroll) {
  let selected = null;
  rows().forEach((row) => {
    const on = Number(row.dataset.index) === selectedIndex;
    row.setAttribute("aria-selected", String(on));
    if (on) selected = row;
  });
  if (selected) {
    el.query.setAttribute("aria-activedescendant", selected.id);
    if (scroll) selected.scrollIntoView({ block: "nearest" });
  } else {
    el.query.removeAttribute("aria-activedescendant");
  }
  updateAction();
}

function setSelected(index, scroll = false) {
  if (index === selectedIndex) return;
  selectedIndex = index;
  updateSelection(scroll);
}

/** The footer names what Return will do to the selection, and holding
 * Command shows the alternative. */
function updateAction() {
  const result = results[selectedIndex];
  el.action.replaceChildren();
  if (!result) return;
  const reveal = commandHeld && REVEALABLE.has(result.kind);
  const label = document.createElement("span");
  label.textContent = reveal ? "Show in Finder" : KINDS[result.kind]?.verb || "Open";
  const keys = reveal ? ["⌘", "↩"] : ["↩"];
  el.action.append(
    label,
    ...keys.map((symbol) => {
      const key = document.createElement("kbd");
      key.textContent = symbol;
      return key;
    }),
  );
}

/* --------------------------------------------------------------- activate */

async function activate(index, reveal = false) {
  const result = results[index];
  if (!result) return;
  if (reveal && !REVEALABLE.has(result.kind)) reveal = false;
  hide();
  try {
    if (reveal) {
      await invoke("launcher_reveal", { kind: result.kind, target: result.target });
    } else {
      await invoke("launcher_activate", {
        id: result.id,
        kind: result.kind,
        target: result.target,
        title: result.title,
        subtitle: result.subtitle,
      });
    }
  } catch (error) {
    console.error("[launcher] activation failed", error);
  }
}

function hide() {
  invoke("launcher_hide").catch(() => {});
}

/* --------------------------------------------------------------- keyboard */

el.query.addEventListener("input", () => scheduleSearch(el.query.value));

el.query.addEventListener("keydown", (event) => {
  const count = results.length;
  const down = event.key === "ArrowDown" || (event.ctrlKey && event.key === "n");
  const up = event.key === "ArrowUp" || (event.ctrlKey && event.key === "p");
  if (down || up) {
    event.preventDefault();
    if (count) setSelected((selectedIndex + (down ? 1 : count - 1)) % count, true);
  } else if (event.key === "Enter") {
    event.preventDefault();
    if (!event.repeat) activate(selectedIndex, event.metaKey);
  } else if (event.key === "Escape") {
    event.preventDefault();
    // Like Spotlight: the first Escape clears what you typed.
    if (el.query.value) {
      el.query.value = "";
      scheduleSearch("");
    } else {
      hide();
    }
  } else if (event.key === "Tab") {
    event.preventDefault();
  }
});

function trackCommand(event) {
  const held = event.metaKey;
  if (held === commandHeld) return;
  commandHeld = held;
  updateAction();
}

window.addEventListener("keydown", trackCommand);
window.addEventListener("keyup", trackCommand);

/* -------------------------------------------------------------- lifecycle */

function setState(state, instant = false) {
  if (instant) el.panel.classList.add("instant");
  el.panel.dataset.state = state;
  if (instant) {
    void el.panel.offsetHeight;
    el.panel.classList.remove("instant");
  }
}

/** Window still hidden: start over with a closed panel and fresh results, so
 * nothing from last time can flash on screen. */
function present() {
  clearTimeout(openTimer);
  setState("closed", true);
  commandHeld = false;
  sizeInstantly = true;
  // Rust presents the window at the last resting size; whatever this render
  // needs is applied from scratch.
  windowHeight = 0;
  el.query.value = "";
  refreshHotkey();
  scheduleSearch("");
  // Learns whether Descriptive Find is on, for the `?` legend.
  findInFiles("", false, 1);
  if (document.hasFocus()) {
    // The window took focus before this event arrived.
    requestAnimationFrame(open);
  } else {
    // Focus normally opens the panel; this covers a window that never gets it.
    openTimer = setTimeout(open, 200);
  }
}

function open() {
  clearTimeout(openTimer);
  if (el.panel.dataset.state === "open") return;
  // Settle the closed style first so the change to open is a transition.
  void getComputedStyle(el.panel).opacity;
  setState("open");
  // Size changes from here on are part of using the launcher and animate.
  requestAnimationFrame(() => {
    sizeInstantly = false;
  });
  el.query.focus();
}

listen("launcher://present", present).catch(() => {});
listen("launcher://reopen", () => {
  setState("open");
  el.query.focus();
}).catch(() => {});
listen("launcher://dismiss", () => {
  clearTimeout(openTimer);
  setState("leaving");
}).catch(() => {});

window.addEventListener("focus", () => {
  if (el.panel.dataset.state !== "leaving") open();
});
window.addEventListener("blur", () => {
  if (el.panel.dataset.state === "open") hide();
});

// The margin around the panel is part of the window but not of the launcher:
// a click there is a click outside.
el.stage.addEventListener("pointerdown", (event) => {
  if (event.target === el.stage) hide();
});

refreshHotkey();
present();
