/* Samlu's keyboard-driven developer launcher. */

const tauri = window.__TAURI__ || {};
const invoke = tauri.core?.invoke || (async () => null);

window.addEventListener("error", (e) => console.error("[launcher]", e.error || e.message));

const el = {
  shell: document.getElementById("shell"),
  query: document.getElementById("query"),
  results: document.getElementById("results"),
  hint: document.getElementById("query-hint"),
};

// Row height and list padding are fixed in launcher.css; deriving the window
// height from them is exact and avoids measuring a flex-stretched box.
const ROW_H = 40;
const LIST_PADDING = 12;
const CHROME_H = 56 + 34; // query bar + prefix legend
const MAX_H = 420;

// Mono glyph per result type, mirroring the prefix legend in the footer.
const KIND_GLYPH = {
  project: "\u2318",
  file: "</>",
  app: "\u25a2",
  snippet: ";",
  command: ">",
  action: ">",
  clipboard: "@",
  calculation: "=",
};

const HOTKEY_SYMBOLS = {
  cmd: "\u2318",
  command: "\u2318",
  super: "\u2318",
  meta: "\u2318",
  ctrl: "\u2303",
  control: "\u2303",
  alt: "\u2325",
  option: "\u2325",
  shift: "\u21e7",
};

function formatHotkey(hotkey) {
  return String(hotkey || "")
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => HOTKEY_SYMBOLS[part.toLowerCase()] || part.toUpperCase())
    .join("");
}

invoke("get_launcher_hotkey")
  .then((hotkey) => {
    if (hotkey) el.hint.textContent = formatHotkey(hotkey);
  })
  .catch(() => {});

let results = [];
let selectedIndex = 0;
const iconCache = new Map();
const MAX_RESULTS = 12;

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

invoke("get_appearance").then(applyAppearance).catch(() => {});
tauri.event?.listen("appearance://changed", (event) => applyAppearance(event.payload)).catch(() => {});

// Fast (apps/actions/projects, in-memory) and slow (files, via mdfind)
// results are fetched separately: fast renders immediately on every
// keystroke, slow arrives a moment later and gets appended, so a slow file
// search never makes typing itself feel laggy. `searchGeneration` guards
// against a slow response for an old query landing after a newer one.
let searchGeneration = 0;
let fastTimer = null;
let slowTimer = null;

function loadAppIcon(path) {
  if (iconCache.has(path)) return Promise.resolve(iconCache.get(path));
  return invoke("launcher_app_icon", { path })
    .then((dataUrl) => {
      iconCache.set(path, dataUrl);
      return dataUrl;
    })
    .catch(() => null);
}

/** Fits the window to what is actually on screen. A half-empty pane of glass
 * reads as unfinished, and a fixed-height panel can only ever be right for one
 * result count. */
let lastHeight = 0;

function syncHeight() {
  const empty = el.results.querySelector(".results-empty");
  const content = empty
    ? empty.getBoundingClientRect().height + LIST_PADDING
    : results.length * ROW_H + LIST_PADDING;
  const height = Math.min(MAX_H, Math.round(CHROME_H + content));
  if (height === lastHeight) return;
  lastHeight = height;
  invoke("launcher_resize", { height, resting: !el.query.value }).catch(() => {});
}

/** Nothing matched, so say what was searched and point at the tools that are
 * one keystroke away, rather than dead-ending on "No results". */
function renderEmptyState() {
  const empty = document.createElement("li");
  empty.className = "results-empty";

  const headline = document.createElement("p");
  const query = el.query.value.trim();
  headline.textContent = query ? `No matches for “${query}”` : "Start typing to search";

  const suggestion = document.createElement("span");
  suggestion.className = "suggestion";
  suggestion.append(
    "Search projects, apps, and files by name — or use ",
    prefixHint("="),
    " to calculate, ",
    prefixHint(";"),
    " for snippets, ",
    prefixHint("@"),
    " for clipboard, ",
    prefixHint(">"),
    " for commands.",
  );

  empty.append(headline, suggestion);
  el.results.appendChild(empty);
}

function prefixHint(symbol) {
  const code = document.createElement("code");
  code.textContent = symbol;
  return code;
}

function renderResults() {
  el.results.textContent = "";
  el.query.removeAttribute("aria-activedescendant");

  if (!results.length) {
    renderEmptyState();
    syncHeight();
    return;
  }

  results.forEach((result, index) => {
    const row = document.createElement("li");
    row.id = `result-${index}`;
    row.className = "result";
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", String(index === selectedIndex));

    const kind = document.createElement("span");
    kind.className = "result-kind";
    kind.textContent = KIND_GLYPH[result.kind] || "\u25aa";

    const main = document.createElement("span");
    main.className = "result-main";
    const title = document.createElement("span");
    title.className = "result-title";
    title.textContent = result.title;
    const subtitle = document.createElement("span");
    subtitle.className = "result-subtitle";
    subtitle.textContent = result.subtitle;
    main.append(title, subtitle);

    row.append(kind, main);
    row.addEventListener("pointerenter", () => setSelected(index));
    row.addEventListener("click", () => activate(index));

    // Apps get their real icon once it's loaded (cached on the Rust side
    // after the first extraction) - starts as the text badge above so the
    // row isn't empty while that's in flight, then swaps in place.
    if (result.kind === "app") {
      loadAppIcon(result.target).then((dataUrl) => {
        if (!dataUrl || !kind.isConnected) return;
        const icon = document.createElement("img");
        icon.className = "result-icon";
        icon.src = dataUrl;
        icon.alt = "";
        kind.replaceWith(icon);
      });
    }

    el.results.appendChild(row);
  });
  updateSelection(false);
  syncHeight();
}

function updateSelection(ensureVisible) {
  const rows = el.results.querySelectorAll(".result");
  rows.forEach((row, index) => {
    row.setAttribute("aria-selected", String(index === selectedIndex));
  });
  const selected = rows[selectedIndex];
  if (!selected) {
    el.query.removeAttribute("aria-activedescendant");
    return;
  }
  el.query.setAttribute("aria-activedescendant", selected.id);
  if (ensureVisible) selected.scrollIntoView({ block: "nearest" });
}

function setSelected(index, ensureVisible = false) {
  if (index === selectedIndex) return;
  selectedIndex = index;
  updateSelection(ensureVisible);
}

async function runFastSearch(query, generation) {
  let fast;
  try {
    fast = (await invoke("launcher_search", { query })) || [];
  } catch (err) {
    console.error("[launcher] launcher_search failed", err);
    fast = [];
  }
  if (generation !== searchGeneration) return; // superseded by a newer keystroke
  results = fast;
  selectedIndex = 0;
  renderResults();
}

async function runSlowSearch(query, generation) {
  if (!query) return; // fast path already covers the empty-query state
  let slow;
  try {
    slow = (await invoke("launcher_search_files", { query })) || [];
  } catch (err) {
    console.error("[launcher] launcher_search_files failed", err);
    slow = [];
  }
  if (generation !== searchGeneration || !slow.length) return;
  const seen = new Set(results.map((result) => result.id));
  results = results
    .concat(slow.filter((result) => !seen.has(result.id)))
    .slice(0, MAX_RESULTS);
  renderResults();
}

function scheduleSearch(query) {
  const generation = ++searchGeneration;
  clearTimeout(fastTimer);
  clearTimeout(slowTimer);
  fastTimer = setTimeout(() => runFastSearch(query, generation), 16);
  slowTimer = setTimeout(() => runSlowSearch(query, generation), 200);
}

async function activate(index) {
  const target = results[index];
  if (!target) return;
  hide();
  try {
    await invoke("launcher_activate", {
      id: target.id,
      kind: target.kind,
      target: target.target,
    });
  } catch (err) {
    console.error("[launcher] launcher_activate failed", err);
  }
}

function hide() {
  invoke("launcher_hide").catch(() => {});
}

function resetAndSearch() {
  el.query.value = "";
  scheduleSearch("");
}

el.query.addEventListener("input", () => {
  scheduleSearch(el.query.value);
});

el.query.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    if (results.length) setSelected((selectedIndex + 1) % results.length, true);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (results.length) setSelected((selectedIndex - 1 + results.length) % results.length, true);
  } else if (e.key === "Enter") {
    e.preventDefault();
    activate(selectedIndex);
  } else if (e.key === "Escape") {
    e.preventDefault();
    hide();
  }
});

/** Rust asks for the exit, waits out its duration, then hides the window, so
 * every dismissal — hotkey, Escape, blur, activating a result — leaves the
 * same way the panel arrived. */
function present() {
  delete el.shell.dataset.leaving;
  // Replay the entrance: the finished fill state would otherwise leave the
  // panel simply appearing on the next open.
  el.shell.style.animation = "none";
  void el.shell.offsetWidth;
  el.shell.style.animation = "";
  lastHeight = 0;
  resetAndSearch();
  el.query.focus();
}

tauri.event?.listen("launcher://dismiss", () => {
  el.shell.dataset.leaving = "true";
}).catch(() => {});

tauri.event?.listen("launcher://present", present).catch(() => {});

// Reset to a fresh search whenever the persistent launcher window is shown.
window.addEventListener("focus", present);

window.addEventListener("blur", hide);

resetAndSearch();
