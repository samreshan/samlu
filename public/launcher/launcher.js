/* Samlu's keyboard-driven developer launcher.
 *
 * Lifecycle: Rust emits `launcher://present` while the window is still
 * hidden, and the page resets to a fresh, closed panel. The entrance plays
 * when the window gains focus. `launcher://dismiss` plays the exit, after
 * which Rust hides the window; `launcher://reopen` arrives if the hotkey is
 * pressed again mid-exit, and the panel simply turns back around.
 *
 * Views: the results list (which `!` fills with tasks), one project's action
 * panel (Tab or ⌘K on a project) and one task's output. They swap in place
 * without animation, and Escape steps back to the view you came from exactly
 * as you left it. */

const tauri = window.__TAURI__ || {};
const invoke = tauri.core?.invoke || (async () => null);
const listen = tauri.event?.listen || (async () => () => {});

window.addEventListener("error", (event) => console.error("[launcher]", event.error || event.message));

const el = {
  stage: document.getElementById("stage"),
  panel: document.getElementById("panel"),
  scope: document.getElementById("scope"),
  query: document.getElementById("query"),
  notice: document.getElementById("notice"),
  body: document.getElementById("body"),
  results: document.getElementById("results"),
  task: document.getElementById("task"),
  taskStatus: document.getElementById("task-status"),
  taskMeta: document.getElementById("task-meta"),
  taskLinks: document.getElementById("task-links"),
  taskLog: document.getElementById("task-log"),
  hotkey: document.getElementById("hotkey"),
  legend: document.getElementById("legend"),
  action: document.getElementById("action"),
  legendDescribe: document.getElementById("legend-describe"),
};
const SEARCH_PLACEHOLDER = el.query.placeholder;

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
// `!` lists tasks instead of searching.
const TASKS_PREFIX = "!";
// The task view keeps this much output on screen; ⌘O opens the whole log.
const MAX_LOG_LINES = 1000;
// An arrow key scrolls the task view's log by about two lines.
const LOG_STEP = 36;
// Finished tasks the action panel lists after its running ones.
const MAX_PANEL_RECENT = 5;
// How long "Copied" stays in the footer.
const FLASH_MS = 1200;
const MODIFIER_KEYS = new Set(["Meta", "Control", "Alt", "Shift", "CapsLock", "Fn"]);

const KINDS = {
  app: { label: "Application", verb: "Open" },
  project: { label: "Project", verb: "Open Folder" },
  file: { label: "File", verb: "Open" },
  action: { label: "Command", verb: "Run" },
  snippet: { label: "Snippet", verb: "Copy" },
  clipboard: { label: "Clipboard", verb: "Copy" },
  calculation: { label: "Calculator", verb: "Copy Result" },
  content: { label: "In File", verb: "Open" },
  task: { label: "Task", verb: "Show Task" },
};
const REVEALABLE = new Set(["app", "file", "project", "content"]);

// Project actions by kind: the panel's label for an action no key runs, the
// footer's verb for it, and the footer's word while it runs.
const ACTION_KINDS = {
  open: { label: "App", verb: "Open", busy: "Opening…" },
  reveal: { label: "Finder", verb: "Show", busy: "Opening…" },
  copy_path: { label: "Clipboard", verb: "Copy", busy: "Copying…" },
  url: { label: "Link", verb: "Open", busy: "Opening…" },
  terminal: { label: "Terminal", verb: "Run", busy: "Starting…" },
  task: { label: "Task", verb: "Run", busy: "Starting…" },
  script: { label: "Script", verb: "Run", busy: "Starting…" },
};
// A task's status is always a word; the dot beside it is never the only sign.
const STATUS = { running: "Running", succeeded: "Succeeded", failed: "Failed", stopped: "Stopped" };

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
  task: SVG('<path d="M1.8 8.2h2.6L6.2 4l3.4 8 1.8-3.8h2.8"/>'),
  script: SVG('<path d="M5 3.6v8.8c0 .5.5.7.9.5l6.8-4.4c.4-.2.4-.8 0-1L5.9 3.1c-.4-.2-.9 0-.9.5Z"/>'),
};
ICONS.open_projects_settings = ICONS.open_settings;
// Project actions without an app icon to show.
const ACTION_ICONS = {
  open: ICONS.app,
  reveal: ICONS.project,
  copy_path: ICONS.clipboard,
  url: SVG('<path d="M6.7 8.7a3.3 3.3 0 0 0 5 .4l2-2a3.3 3.3 0 0 0-4.7-4.7l-1.1 1.1"/><path d="M9.3 7.3a3.3 3.3 0 0 0-5-.4l-2 2a3.3 3.3 0 0 0 4.7 4.7l1.1-1.1"/>'),
  terminal: ICONS.open_terminal,
  task: ICONS.script,
  script: ICONS.script,
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
  up: "↑",
  arrowup: "↑",
  down: "↓",
  arrowdown: "↓",
  left: "←",
  arrowleft: "←",
  right: "→",
  arrowright: "→",
  enter: "↩",
  tab: "⇥",
  backspace: "⌫",
  delete: "⌦",
  pageup: "⇞",
  pagedown: "⇟",
  home: "↖",
  end: "↘",
  space: "Space",
  backquote: "`",
  minus: "-",
  equal: "=",
  bracketleft: "[",
  bracketright: "]",
  backslash: "\\",
  semicolon: ";",
  quote: "'",
  comma: ",",
  period: ".",
  slash: "/",
};

let results = [];
let selectedIndex = 0;
let generation = 0;
// The search whose answer is on screen. Live task rows are only refreshed on
// top of it, never on top of rows from an earlier query.
let landed = 0;
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
// The modifiers held right now; the footer names what Return does with them.
let held = { ctrl: false, alt: false, shift: false, cmd: false };
const iconCache = new Map();
// What each key does to a project result, by chord ("alt+enter"), and the
// request fetching them, which Return waits on.
let bindings = new Map();
let bindingsReady = Promise.resolve();
// The view on screen: "results", "panel" or "task". The views Escape steps
// back to wait on the stack, each as it was left.
let view = "results";
const stack = [];
// The action panel's project as launcher_project_actions described it, and
// the result it was opened from, whose subtitle goes into launcher history.
let panel = null;
let panelOrigin = null;
let panelRequest = 0;
// The task the task view shows, whether its log follows the end, and the
// clock that ticks its elapsed time.
let shownTask = null;
let followLog = true;
let taskTimer = null;
let stopping = false;
// While an action runs, the footer says so ("Opening…") and Return waits.
let pending = "";
// A brief confirmation in the footer ("Copied").
let flash = "";
let flashTimer = null;
// Counts presentations, so an answer meant for an earlier one is dropped.
let presentation = 0;

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
  if (trimmed.startsWith(TASKS_PREFIX)) {
    showTaskList(trimmed.slice(TASKS_PREFIX.length).trim(), current);
    return;
  }
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
  // An empty search leads with the tasks that are still running.
  const running = query.trim() ? null : runningTaskRows();
  let fast = [];
  try {
    fast = (await invoke("launcher_search", { query })) || [];
  } catch (error) {
    console.error("[launcher] launcher_search failed", error);
  }
  const tasks = running ? await running : [];
  if (current !== generation) return; // a newer keystroke has taken over
  results = tasks.concat(fast).slice(0, MAX_RESULTS);
  // Return still opens what you used last; the running tasks are a step up.
  selectedIndex = Math.max(0, results.findIndex((result) => result.kind !== "task"));
  landed = current;
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

/** The app whose icon a row shows: the app itself, or the app a project
 * action opens. */
function iconPathFor(result) {
  if (result.kind === "app") return result.target;
  return result.kind === "project_action" ? result.appPath : "";
}

function iconFor(result) {
  const appPath = iconPathFor(result);
  if (appPath) {
    const cached = iconCache.get(appPath);
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
  if (appPath) tile.dataset.iconFor = appPath;
  if (GLYPHS[result.kind]) {
    tile.classList.add("glyph");
    tile.textContent = GLYPHS[result.kind];
  } else if (result.kind === "action") {
    tile.innerHTML = ICONS[result.target] || ICONS.action;
  } else if (result.kind === "project_action") {
    tile.innerHTML = ACTION_ICONS[result.actionKind] || ICONS.action;
  } else {
    tile.innerHTML = ICONS[result.kind] || ICONS.action;
  }
  return tile;
}

/** Swaps placeholder tiles for the app's real icon once it is extracted. The
 * first time an icon is fetched it fades up; after that it is cached and
 * appears with its row. The list may have been redrawn while the icon was on
 * its way, so whichever placeholders are on screen when it lands are swapped. */
function loadAppIcon(path) {
  if (iconCache.has(path)) return;
  iconCache.set(path, null);
  invoke("launcher_app_icon", { path })
    .then((dataUrl) => {
      if (!dataUrl) {
        iconCache.delete(path);
        return;
      }
      iconCache.set(path, dataUrl);
      el.results.querySelectorAll(".result-tile[data-icon-for]").forEach((placeholder) => {
        if (placeholder.dataset.iconFor !== path) return;
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
    .catch(() => iconCache.delete(path));
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

function keycap(symbol) {
  const key = document.createElement("kbd");
  key.textContent = symbol;
  return key;
}

/** A task's status: a word, with a dot whose colour and shape repeat it. */
function statusMark(status) {
  const mark = document.createElement("span");
  mark.className = "status";
  mark.dataset.status = status;
  const dot = document.createElement("span");
  dot.className = "status-dot";
  dot.setAttribute("aria-hidden", "true");
  const word = document.createElement("span");
  word.textContent = STATUS[status] || status;
  mark.append(dot, word);
  return mark;
}

/** The right side of a row: the keys that run a project action, a task's
 * status, or what kind of thing the row is. */
function renderAccessory(result) {
  const side = document.createElement("span");
  side.className = "result-kind";
  if (result.chords?.length) {
    side.classList.add("result-keys");
    result.chords.forEach((chord) => {
      const group = document.createElement("span");
      group.append(...chordKeys(chord).map(keycap));
      side.append(group);
    });
  } else if (result.kind === "task") {
    side.append(statusMark(result.status));
  } else {
    side.textContent = result.label ?? (KINDS[result.kind]?.label || "");
  }
  return side;
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
  const kind = renderAccessory(result);

  row.append(icon, text, kind);
  const appPath = iconPathFor(result);
  if (appPath && !iconCache.get(appPath)) loadAppIcon(appPath);

  // Hover moves the selection only when the pointer actually moves, so a
  // list re-rendering under a resting pointer never steals it.
  row.addEventListener("pointermove", () => setSelected(index));
  row.addEventListener("click", (event) => activate(index, event));
  return row;
}

function renderEmpty() {
  const empty = document.createElement("li");
  empty.className = "empty";
  empty.setAttribute("role", "presentation");
  const headline = document.createElement("p");
  const hint = document.createElement("span");
  const query = el.query.value.trim();
  if (view === "panel") {
    headline.textContent = query ? `Nothing matches “${query}”` : "No actions yet";
    hint.textContent = "Search this project’s actions, scripts and tasks, or add actions in Settings › Projects.";
  } else if (query.startsWith("=")) {
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
  } else if (query.startsWith(TASKS_PREFIX)) {
    const filter = query.slice(TASKS_PREFIX.length).trim();
    if (filter) {
      headline.textContent = `No tasks match “${filter}”`;
      hint.textContent = "Tasks are found by their title or their project’s name.";
    } else {
      headline.textContent = "No tasks yet";
      hint.textContent = "Run an action that starts a task, or a script in the background.";
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
 * takes clicks — is never more than the shadow's margin. The task view always
 * takes the full height, and a notice takes its room from the list, so the
 * panel never outgrows the window. */
async function fitPanel() {
  const notice = el.notice.offsetHeight;
  const content = view === "task" ? MAX_BODY_H : el.results.offsetHeight;
  const body = Math.min(MAX_BODY_H - notice, content);
  const height = SEARCH_H + notice + body + FOOTER_H;
  const sequence = ++fitSequence;
  const needed = STAGE_TOP + height + STAGE_BOTTOM;
  const resting = view === "results" && !el.query.value;
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

/** The footer names what Return will do to the selection, and holding a
 * modifier shows what that key does instead. Whatever else the view offers
 * follows, quieter. */
function updateAction() {
  const hints = view === "task" ? taskHints() : listHints();
  el.action.replaceChildren(
    ...hints.map(([label, keys], index) => {
      const item = document.createElement("span");
      item.className = index ? "hint quiet" : "hint";
      const text = document.createElement("span");
      text.textContent = label;
      item.append(text, ...keys.map(keycap));
      return item;
    }),
  );
}

function listHints() {
  const result = results[selectedIndex];
  const hints = result ? [returnHint(result)] : [];
  if (view === "results" && result?.kind === "project") hints.push(["Actions", ["⇥"]]);
  if (view === "panel") hints.push(["Back", ["⎋"]]);
  return hints;
}

/** What Return does to `result` with the modifiers held now. */
function returnHint(result) {
  if (pending) return [pending, []];
  if (result.kind === "project") {
    const chord = heldChord();
    return [bindings.get(chord)?.title || "Show Actions", chordKeys(chord)];
  }
  if (result.kind === "project_action") {
    return [ACTION_KINDS[result.actionKind]?.verb || "Run", ["↩"]];
  }
  if (result.kind === "script") {
    if (!held.cmd) return ["Run", ["↩"]];
    return [panel?.scriptMode === "task" ? "Run in Terminal" : "Run in Background", ["⌘", "↩"]];
  }
  const reveal = held.cmd && REVEALABLE.has(result.kind);
  return [reveal ? "Show in Finder" : KINDS[result.kind]?.verb || "Open", reveal ? ["⌘", "↩"] : ["↩"]];
}

function taskHints() {
  const task = shownTask;
  if (!task) return [];
  const hints = [];
  if (flash) hints.push([flash, []]);
  else if (task.urls?.length) hints.push(held.cmd ? ["Copy Link", ["⌘", "↩"]] : ["Open Link", ["↩"]]);
  if (stopping) hints.push(["Stopping…", []]);
  else if (task.status === "running") hints.push(["Stop", ["⌘", "."]]);
  else hints.push(["Run Again", ["⌘", "R"]]);
  hints.push(["Open Log", ["⌘", "O"]], ["Back", ["⎋"]]);
  return hints;
}

function showFlash(text) {
  flash = text;
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => {
    flash = "";
    updateAction();
  }, FLASH_MS);
  updateAction();
}

/** A short message above the list when something didn't work. It stays until
 * your next keystroke. */
function showNotice(error) {
  const message = typeof error === "string" ? error : error?.message;
  setNotice(message || "Something went wrong. Try again.");
}

function setNotice(text) {
  if (el.notice.textContent === text) return;
  el.notice.textContent = text;
  fitPanel();
}

/* ------------------------------------------------------------------- views */

/** Switches the search field, the body and the footer to `name`; the caller
 * fills in the rows. Answers still on their way belong to the view being
 * left, so they are dropped. */
function setView(name, { query = "", scope = "" } = {}) {
  ++generation;
  clearTimeout(slowTimer);
  clearTimeout(findTimer);
  view = name;
  el.query.value = query;
  el.query.readOnly = name === "task";
  el.query.placeholder = { panel: "Search actions", task: "" }[name] ?? SEARCH_PLACEHOLDER;
  el.query.setAttribute("aria-expanded", String(name !== "task"));
  el.query.removeAttribute("aria-activedescendant");
  el.results.setAttribute("aria-label", name === "panel" ? "Actions" : "Results");
  el.scope.textContent = scope;
  el.scope.hidden = !scope;
  el.legend.hidden = name !== "results";
  el.results.hidden = name === "task";
  el.task.hidden = name !== "task";
  if (name !== "task") {
    shownTask = null;
    stopping = false;
    clearInterval(taskTimer);
    taskTimer = null;
    el.taskLog.replaceChildren();
  }
}

function saveView() {
  return {
    view,
    query: el.query.value,
    scope: el.scope.textContent,
    results,
    selectedIndex,
    selectedId: results[selectedIndex]?.id,
    scrollTop: el.body.scrollTop,
  };
}

/** Steps back to the view you came from, exactly as you left it. */
function back() {
  const saved = stack.pop();
  if (!saved) {
    hide();
    return;
  }
  setView(saved.view, { query: saved.query, scope: saved.scope });
  if (saved.view === "panel") {
    // The project's tasks may have moved on in the meantime.
    results = panelRows();
  } else {
    panel = null;
    panelOrigin = null;
    results = saved.results;
  }
  const kept = results.findIndex((result) => result.id === saved.selectedId);
  selectedIndex = kept >= 0 ? kept : Math.min(saved.selectedIndex, Math.max(0, results.length - 1));
  landed = generation;
  render();
  el.body.scrollTop = saved.scrollTop;
  refreshTaskRows();
}

/** Swaps in fresh rows without disturbing you: the selection stays on the
 * same row and the list keeps its scroll position. */
function replaceRows(next) {
  const selectedId = results[selectedIndex]?.id;
  results = next;
  const kept = results.findIndex((result) => result.id === selectedId);
  selectedIndex = kept >= 0 ? kept : 0;
  render({ keepScroll: true });
}

/* --------------------------------------------------------------- activate */

/** Return or a click on a row. The modifiers held with it matter for a
 * project (each key runs its own action) and a script (⌘ runs it the other
 * way). */
function activate(index, event) {
  const result = results[index];
  if (!result || pending) return;
  if (result.kind === "project") runProject(result, chordFrom(event));
  else if (result.kind === "task") showTask(result.target);
  else if (result.kind === "project_action" || result.kind === "script") runPanelRow(result, event);
  else activateResult(result, Boolean(event?.metaKey));
}

async function activateResult(result, reveal) {
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

/* ------------------------------------------------------------ project keys */

function loadBindings() {
  bindingsReady = invoke("launcher_project_bindings")
    .then((list) => {
      bindings = new Map((list || []).map((binding) => [binding.chord, binding]));
      updateAction();
    })
    .catch((error) => console.error("[launcher] launcher_project_bindings failed", error));
}

/** The chord a key or click makes: modifiers in ⌃⌥⇧⌘ order, then "enter". */
function chordFrom(event) {
  return [
    event?.ctrlKey && "ctrl",
    event?.altKey && "alt",
    event?.shiftKey && "shift",
    event?.metaKey && "cmd",
    "enter",
  ]
    .filter(Boolean)
    .join("+");
}

/** The chord Return would make with the modifiers held now. */
function heldChord() {
  return chordFrom({ ctrlKey: held.ctrl, altKey: held.alt, shiftKey: held.shift, metaKey: held.cmd });
}

/** "alt+enter" → ["⌥", "↩"]. */
function chordKeys(chord) {
  return chord.split("+").map((part) => HOTKEY_SYMBOLS[part] || part);
}

/** Return on a project, with or without modifiers: the action bound to that
 * key, or the action panel when the key is free. */
async function runProject(result, chord) {
  await bindingsReady;
  const binding = bindings.get(chord);
  if (!binding) {
    openPanel(result);
    return;
  }
  run(
    "launcher_run_project_chord",
    { path: result.target, chord, title: result.title, subtitle: result.subtitle },
    ACTION_KINDS[binding.kind]?.busy,
  );
}

/** Runs a project action. One that starts a task keeps the launcher open on
 * that task; anything else closes the launcher once it has worked, and stays
 * open with the reason when it has not. */
async function run(command, args, busy = "Opening…") {
  if (pending) return;
  const session = presentation;
  pending = busy;
  updateAction();
  try {
    const outcome = await invoke(command, args);
    if (session !== presentation) return;
    if (outcome?.taskId) await showTask(outcome.taskId);
    else hide();
  } catch (error) {
    if (session === presentation) showNotice(error);
  } finally {
    if (session === presentation) {
      pending = "";
      updateAction();
    }
  }
}

/* ------------------------------------------------------------ action panel */

/** Tab or ⌘K on a project: every action, the project's own scripts and its
 * tasks, in place of the results. */
async function openPanel(result) {
  if (result?.kind !== "project") return;
  const request = ++panelRequest;
  const session = presentation;
  // Typing before the panel arrives means you have moved on.
  const current = generation;
  let data = null;
  try {
    data = await invoke("launcher_project_actions", { path: result.target, title: result.title });
  } catch (error) {
    if (request === panelRequest && session === presentation) showNotice(error);
    return;
  }
  const stale = request !== panelRequest || session !== presentation || current !== generation;
  if (!data || stale || view !== "results") return;
  stack.push(saveView());
  panel = data;
  panelOrigin = result;
  setView("panel", { scope: data.project?.name || result.title });
  results = panelRows();
  selectedIndex = 0;
  render();
}

/** The panel's rows, filtered by what is typed: actions in their configured
 * order, then the project's scripts, then its tasks. */
function panelRows() {
  if (!panel) return [];
  const actions = (panel.actions || []).map((action) => ({
    id: `action:${action.id}`,
    kind: "project_action",
    section: "Actions",
    title: action.title,
    subtitle: action.subtitle,
    target: action.id,
    actionKind: action.kind,
    appPath: action.appPath || "",
    chords: action.chords || [],
    label: ACTION_KINDS[action.kind]?.label || "",
  }));
  actions.push({
    id: "settings:projects",
    kind: "action",
    section: "Actions",
    title: "Edit Actions…",
    subtitle: "Settings › Projects",
    target: "open_projects_settings",
  });
  const scripts = (panel.scripts || []).map((script) => ({
    id: `script:${script.id}`,
    kind: "script",
    section: "Scripts",
    title: script.name,
    subtitle: script.command,
    target: script.id,
    label: script.cwd ? `${script.cwd} · ${script.source}` : script.source,
  }));
  const tasks = taskRows(panel.tasks || [], MAX_PANEL_RECENT, false);
  const needle = el.query.value.trim().toLowerCase();
  return actions
    .concat(scripts, tasks)
    .filter((row) => !needle || `${row.title}\n${row.subtitle || ""}`.toLowerCase().includes(needle));
}

/** Return on an action or a script in the panel. A script runs the way
 * Settings says, and ⌘↩ runs it the other way (terminal or background). */
function runPanelRow(result, event) {
  if (!panel) return;
  const project = {
    path: panel.project.path,
    title: panel.project.name,
    subtitle: panelOrigin?.subtitle,
  };
  if (result.kind === "script") {
    const other = panel.scriptMode === "task" ? "terminal" : "task";
    run(
      "launcher_run_script",
      { ...project, scriptId: result.target, mode: event?.metaKey ? other : panel.scriptMode },
      "Starting…",
    );
  } else {
    run(
      "launcher_run_project_action",
      { ...project, actionId: result.target },
      ACTION_KINDS[result.actionKind]?.busy,
    );
  }
}

/* ------------------------------------------------------------------- tasks */

async function listTasks() {
  try {
    return (await invoke("tasks_list")) || [];
  } catch (error) {
    console.error("[launcher] tasks_list failed", error);
    return [];
  }
}

function filterTasks(tasks, filter) {
  const needle = filter.toLowerCase();
  if (!needle) return tasks;
  return tasks.filter((task) => `${task.title}\n${task.project?.name || ""}`.toLowerCase().includes(needle));
}

/** Running tasks first, then the most recent: the order tasks_list uses. */
function sortTasks(tasks) {
  return tasks.sort(
    (a, b) => Number(b.status === "running") - Number(a.status === "running") || b.startedAt - a.startedAt,
  );
}

/** Rows for tasks: the running ones under "Running", then up to `recent`
 * finished ones under "Recent". */
function taskRows(tasks, recent = Infinity, withProject = true) {
  const running = tasks.filter((task) => task.status === "running");
  const finished = tasks.filter((task) => task.status !== "running").slice(0, recent);
  return running
    .map((task) => taskRow(task, "Running", withProject))
    .concat(finished.map((task) => taskRow(task, "Recent", withProject)));
}

function taskRow(task, section, withProject) {
  return {
    id: `task:${task.id}`,
    kind: "task",
    section,
    title: task.title,
    subtitle: taskSubtitle(task, withProject),
    target: task.id,
    status: task.status,
    task,
    withProject,
  };
}

/** "samlu-companion · Step 2 of 3 — Compiling samlu…" while it runs, then
 * "… · Finished in 1m 12s", "… · Failed at Test (exit 3)" or "… · Stopped".
 * The action panel leaves out the project, which it is already about. */
function taskSubtitle(task, withProject = true) {
  const parts = withProject && task.project?.name ? [task.project.name] : [];
  if (task.status === "running") {
    if (task.stepCount > 1) parts.push(`Step ${(task.step || 0) + 1} of ${task.stepCount}`);
    const head = parts.join(" · ");
    if (!task.lastLine) return head || "No output yet";
    return head ? `${head} — ${task.lastLine}` : task.lastLine;
  }
  if (task.status === "succeeded") {
    parts.push(`Finished in ${duration((task.finishedAt || task.startedAt) - task.startedAt)}`);
  } else if (task.status === "failed") {
    const step = task.stepTitle ? ` at ${task.stepTitle}` : "";
    const exit = task.exitCode == null ? "" : ` (exit ${task.exitCode})`;
    parts.push(`Failed${step}${exit}`);
  } else {
    parts.push("Stopped");
  }
  return parts.join(" · ");
}

/** 72_000 → "1m 12s", the way task notifications write it. */
function duration(millis) {
  const seconds = Math.max(0, Math.floor((millis || 0) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

async function runningTaskRows() {
  return taskRows(await listTasks(), 0);
}

/** `!` lists tasks, running ones first, found by title or project. It never
 * reaches launcher_search. */
async function showTaskList(filter, current) {
  const tasks = await listTasks();
  if (current !== generation) return;
  results = taskRows(filterTasks(tasks, filter));
  selectedIndex = 0;
  landed = current;
  render();
}

/** Keeps the task rows of the results view current: the `!` list, or the
 * running tasks an empty search leads with. */
function refreshTaskRows() {
  const query = el.query.value.trim();
  if (view !== "results" || landed !== generation) return;
  if (query && !query.startsWith(TASKS_PREFIX)) return;
  const current = generation;
  listTasks().then((tasks) => {
    if (current !== generation) return;
    if (query) {
      replaceRows(taskRows(filterTasks(tasks, query.slice(TASKS_PREFIX.length).trim())));
    } else {
      const others = results.filter((result) => result.kind !== "task");
      replaceRows(taskRows(tasks, 0).concat(others).slice(0, MAX_RESULTS));
    }
  });
}

/** Updates a running task's row in place as its output arrives, without
 * redrawing the list under the pointer. */
function patchTaskRows(id, line) {
  results.forEach((result, index) => {
    if (result.kind !== "task" || result.target !== id || result.status !== "running") return;
    result.task = { ...result.task, lastLine: line };
    result.subtitle = taskSubtitle(result.task, result.withProject);
    const subtitle = el.results.querySelector(`#result-${index} .result-subtitle`);
    if (subtitle) subtitle.textContent = result.subtitle;
  });
}

/** One task: its status and elapsed time, the links it printed, and its
 * output as it arrives. */
async function showTask(id) {
  const session = presentation;
  let detail = null;
  try {
    detail = await invoke("task_detail", { id });
  } catch (error) {
    if (session === presentation) showNotice(error);
    return;
  }
  if (session !== presentation) return;
  if (!detail) {
    showNotice("That task is gone.");
    return;
  }
  // Running it again replaces the task on screen rather than stacking it.
  if (view !== "task") stack.push(saveView());
  const { lines = [], ...summary } = detail;
  setView("task", { query: summary.title, scope: summary.project?.name || "" });
  shownTask = summary;
  stopping = false;
  followLog = true;
  el.taskLog.replaceChildren();
  appendLog(lines);
  renderTaskHead();
  updateAction();
  fitPanel();
}

function renderTaskHead() {
  const task = shownTask;
  if (!task) return;
  el.taskStatus.replaceChildren(statusMark(task.status));
  renderTaskClock();
  el.taskLinks.replaceChildren(...(task.urls || []).map((url) => linkChip(url)));
  el.taskLinks.hidden = !task.urls?.length;
  el.taskLog.dataset.empty = task.status === "running" ? "No output yet" : "No output";
  clearInterval(taskTimer);
  taskTimer = task.status === "running" ? setInterval(renderTaskClock, 1000) : null;
}

/** Elapsed time and the step, ticking each second while the task runs. */
function renderTaskClock() {
  const task = shownTask;
  if (!task) return;
  const end = task.status === "running" ? Date.now() : task.finishedAt || task.startedAt;
  const parts = [duration(end - task.startedAt)];
  if (task.stepCount > 1) parts.push(`Step ${(task.step || 0) + 1} of ${task.stepCount}`);
  if (task.stepTitle) parts.push(task.stepTitle);
  if (task.status === "failed" && task.exitCode != null) parts.push(`Exit code ${task.exitCode}`);
  el.taskMeta.textContent = parts.join(" · ");
}

function linkChip(url) {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "task-link";
  chip.tabIndex = -1;
  chip.title = url;
  chip.textContent = url.replace(/^https?:\/\//, "").replace(/\/$/, "");
  chip.addEventListener("click", (event) => openLink(url, event.metaKey));
  return chip;
}

/** Adds output to the log. It follows the end unless you have scrolled up to
 * read, and keeps at most MAX_LOG_LINES on screen. */
function appendLog(lines) {
  if (!lines?.length) return;
  const log = el.taskLog;
  const fragment = document.createDocumentFragment();
  lines.forEach((line) => {
    const row = document.createElement("div");
    // Each step starts with a "▸ title" line from the runner.
    row.className = line.startsWith("▸ ") ? "log-line step" : "log-line";
    row.textContent = line;
    fragment.append(row);
  });
  log.append(fragment);
  const extra = log.childElementCount - MAX_LOG_LINES;
  if (extra > 0) {
    const before = log.scrollHeight;
    for (let index = 0; index < extra; index += 1) log.firstElementChild.remove();
    // Lines leaving the top must not move what you are reading.
    if (!followLog) log.scrollTop -= before - log.scrollHeight;
  }
  if (followLog) log.scrollTop = log.scrollHeight;
}

/** Opens a link the task printed, or with ⌘ copies it. */
async function openLink(url, copy = false) {
  const task = shownTask;
  const session = presentation;
  if (!task || !url) return;
  try {
    await invoke("task_url", { id: task.id, url, copy });
    if (session !== presentation) return;
    if (!copy) hide();
    else if (shownTask?.id === task.id) showFlash("Copied");
  } catch (error) {
    if (session === presentation) showNotice(error);
  }
}

async function stopTask() {
  const task = shownTask;
  if (!task || task.status !== "running" || stopping) return;
  stopping = true;
  updateAction();
  try {
    await invoke("task_stop", { id: task.id });
  } catch (error) {
    showNotice(error);
  } finally {
    if (shownTask?.id === task.id) {
      stopping = false;
      updateAction();
    }
  }
}

async function rerunTask() {
  const task = shownTask;
  if (!task) return;
  try {
    const next = await invoke("task_rerun", { id: task.id });
    if (next?.id && shownTask?.id === task.id) await showTask(next.id);
  } catch (error) {
    showNotice(error);
  }
}

function openLog() {
  const task = shownTask;
  const session = presentation;
  if (!task) return;
  invoke("task_open_log", { id: task.id })
    .then(() => {
      if (session === presentation) hide();
    })
    .catch((error) => {
      if (session === presentation) showNotice(error);
    });
}

/** tasks://changed: a task started, moved to its next step, printed a link
 * or finished. */
function taskChanged(task) {
  if (!task?.id) return;
  if (view === "task" && shownTask?.id === task.id) {
    shownTask = { ...shownTask, ...task };
    renderTaskHead();
    updateAction();
  }
  if (panel && task.project?.path === panel.project?.path) {
    panel.tasks = sortTasks([task, ...(panel.tasks || []).filter((known) => known.id !== task.id)]);
    if (view === "panel") replaceRows(panelRows());
  }
  refreshTaskRows();
}

/** tasks://log: new output, a batch about every 100 ms while a task talks. */
function taskLogged(payload) {
  const { id, lines } = payload || {};
  if (!id || !lines?.length) return;
  if (view === "task" && shownTask?.id === id) appendLog(lines);
  // Rows show the latest output line; a step's "▸ title" header is not one.
  let line = "";
  for (let index = lines.length - 1; index >= 0 && !line; index -= 1) {
    if (!lines[index].startsWith("▸ ")) line = lines[index];
  }
  if (!line) return;
  const known = (panel?.tasks || []).find((task) => task.id === id);
  if (known) known.lastLine = line;
  if (view !== "task") patchTaskRows(id, line);
}

/* --------------------------------------------------------------- keyboard */

el.query.addEventListener("input", () => {
  if (view === "panel") {
    results = panelRows();
    selectedIndex = 0;
    render();
  } else {
    scheduleSearch(el.query.value);
  }
});

el.query.addEventListener("keydown", (event) => {
  // A notice is answered by whatever you do next.
  if (!MODIFIER_KEYS.has(event.key)) setNotice("");
  if (view === "task") {
    taskKeydown(event);
    return;
  }
  const count = results.length;
  const down = event.key === "ArrowDown" || (event.ctrlKey && event.key === "n");
  const up = event.key === "ArrowUp" || (event.ctrlKey && event.key === "p");
  const commandK = event.metaKey && event.key.toLowerCase() === "k";
  if (down || up) {
    event.preventDefault();
    if (count) setSelected((selectedIndex + (down ? 1 : count - 1)) % count, true);
  } else if (event.key === "Enter") {
    event.preventDefault();
    if (!event.repeat) activate(selectedIndex, event);
  } else if (event.key === "Escape") {
    event.preventDefault();
    if (view === "panel") {
      back();
    } else if (el.query.value) {
      // Like Spotlight: the first Escape clears what you typed.
      el.query.value = "";
      scheduleSearch("");
    } else {
      hide();
    }
  } else if (event.key === "Tab" || commandK) {
    event.preventDefault();
    if (view === "panel") {
      if (event.shiftKey || commandK) back();
    } else if (!event.shiftKey) {
      openPanel(results[selectedIndex]);
    }
  } else if (event.key === "Backspace" && view === "panel" && !el.query.value) {
    event.preventDefault();
    back();
  }
});

/** The task view's keys. The search field holds its title, read-only. */
function taskKeydown(event) {
  const key = event.key.toLowerCase();
  const log = el.taskLog;
  if (event.key === "Escape" || (event.key === "Tab" && event.shiftKey)) {
    back();
  } else if (event.key === "Enter") {
    if (!event.repeat) openLink(shownTask?.urls?.[0], event.metaKey);
  } else if (event.metaKey && key === ".") {
    stopTask();
  } else if (event.metaKey && key === "r") {
    if (!event.repeat) rerunTask();
  } else if (event.metaKey && key === "o") {
    if (!event.repeat) openLog();
  } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
    log.scrollTop += event.key === "ArrowUp" ? -LOG_STEP : LOG_STEP;
  } else if (event.key === "PageUp" || event.key === "PageDown") {
    const page = Math.max(LOG_STEP, log.clientHeight - LOG_STEP);
    log.scrollTop += event.key === "PageUp" ? -page : page;
  } else if (event.key === "Home" || event.key === "End") {
    log.scrollTop = event.key === "Home" ? 0 : log.scrollHeight;
  } else if (event.key !== "Tab") {
    return;
  }
  event.preventDefault();
}

el.taskLog.addEventListener("scroll", () => {
  const log = el.taskLog;
  followLog = log.scrollHeight - log.scrollTop - log.clientHeight < 8;
});

function trackModifiers(event) {
  const next = { ctrl: event.ctrlKey, alt: event.altKey, shift: event.shiftKey, cmd: event.metaKey };
  if (Object.keys(next).every((key) => next[key] === held[key])) return;
  held = next;
  updateAction();
}

window.addEventListener("keydown", trackModifiers);
window.addEventListener("keyup", trackModifiers);

// Clicking a link, the log or a heading leaves the keyboard in the search
// field, so Escape and the arrow keys keep working.
el.panel.addEventListener("pointerup", () => {
  if (document.activeElement !== el.query) el.query.focus({ preventScroll: true });
});

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
  presentation += 1;
  held = { ctrl: false, alt: false, shift: false, cmd: false };
  pending = "";
  flash = "";
  clearTimeout(flashTimer);
  sizeInstantly = true;
  // Rust presents the window at the last resting size; whatever this render
  // needs is applied from scratch.
  windowHeight = 0;
  stack.length = 0;
  panel = null;
  panelOrigin = null;
  el.notice.textContent = "";
  setView("results");
  refreshHotkey();
  loadBindings();
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
listen("tasks://changed", (event) => taskChanged(event.payload)).catch(() => {});
listen("tasks://log", (event) => taskLogged(event.payload)).catch(() => {});

window.addEventListener("focus", () => {
  if (el.panel.dataset.state !== "leaving") open();
});
window.addEventListener("blur", () => {
  // Modifiers released in another app never reach this window.
  held = { ctrl: false, alt: false, shift: false, cmd: false };
  if (el.panel.dataset.state === "open") hide();
});

// The margin around the panel is part of the window but not of the launcher:
// a click there is a click outside.
el.stage.addEventListener("pointerdown", (event) => {
  if (event.target === el.stage) hide();
});

refreshHotkey();
present();
