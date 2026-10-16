const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const byId = (id) => document.getElementById(id);
const PLATFORM = window.SAMLU_PLATFORM;
const T = PLATFORM.text;
const PANES = ["general", "agents", "voice", "launcher", "projects", "about"];

let currentPane = "general";
let prefs = null;
let updateState = null;
let voiceSettings = null;
let voiceModels = { models: [], catalog: [], recommended: "", selected: "" };
let voiceHistory = [];
let snippets = [];
let projectSettings = { roots: [], excludedPaths: [], projectCount: 0 };
let projectActions = null;
// The project action editor's App pop-up, by bundle id: what each entry is
// called and opens, and the entry shown before Choose App… was picked.
let appChoices = new Map();
let appBeforeChoosing = "";
// Apps picked with Choose App… this session. Samlu has no preset for them, but
// they are certainly installed.
const chosenApps = new Map();
let deepFind = null;
let deepFindDownload = null;
let agentIntegrations = [];
let voiceHotkeyRecorder = null;
let launcherHotkeyRecorder = null;
let toastTimer = null;
let prefsTimer = null;
const savedTimers = {};
const swapTimers = new WeakMap();
const downloadProgress = {};

/* ----------------------------------------------------------------- helpers */

function errorMessage(error) {
  if (typeof error === "string") return error;
  return error?.message || "Something went wrong.";
}

/** Errors, and results that land somewhere you are not looking. Routine saves
 * confirm inline beside the group instead, so a toast always means "read me". */
function toast(message, error = false) {
  const element = byId("toast");
  element.textContent = String(message);
  element.classList.toggle("error", error);
  element.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => element.classList.remove("show"), error ? 4200 : 2600);
}

const CHECK_SVG = '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 6.2l2.3 2.3 4.7-5"/></svg>';

/** A small "Saved" beside a group's title: the check draws itself, then the
 * whole thing fades once it has been seen. */
function showSaved(id, label = "Saved") {
  const element = byId(id);
  if (!element) return;
  clearTimeout(savedTimers[id]);
  element.innerHTML = `${CHECK_SVG}<span></span>`;
  element.lastChild.textContent = label;
  element.classList.remove("show");
  void element.offsetWidth;
  element.classList.add("show");
  savedTimers[id] = setTimeout(() => {
    element.classList.remove("show");
    // Cleared after the fade so the live region never announces nothing.
    savedTimers[id] = setTimeout(() => element.replaceChildren(), 220);
  }, 1800);
}

/** Asks before something that cannot be undone, as a native alert sheet on
 * this window. `window.confirm` is replaced by the dialog plugin with an async
 * function that never waits, so it must not be used here. */
async function confirmAction(title, message, confirm) {
  try {
    return Boolean(await invoke("confirm_action", { title, message, confirm }));
  } catch (error) {
    toast(errorMessage(error), true);
    return false;
  }
}

/** Changes a button's content through a brief blur, so one control changing
 * its label reads as a single morph rather than two labels swapping. */
function swapButton(button, content, { disabled = false, primary = false } = {}) {
  button.disabled = disabled;
  button.classList.toggle("primary", primary);
  if (button.dataset.content === content) return;
  button.dataset.content = content;
  clearTimeout(swapTimers.get(button));
  if (!button.textContent) {
    button.innerHTML = content;
    return;
  }
  button.dataset.swapping = "true";
  swapTimers.set(
    button,
    setTimeout(() => {
      button.innerHTML = content;
      requestAnimationFrame(() => delete button.dataset.swapping);
    }, 90),
  );
}

async function runButton(button, busyLabel, operation) {
  const original = button.innerHTML;
  button.disabled = true;
  button.innerHTML = `<span class="spinner" aria-hidden="true"></span>${busyLabel}`;
  try {
    return await operation();
  } finally {
    button.disabled = false;
    button.innerHTML = original;
  }
}

function formatBytes(bytes) {
  if (!bytes) return "";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit > 1 ? 1 : 0)} ${units[unit]}`;
}

const relativeFormat = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

function timeAgo(iso) {
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return "";
  const seconds = Math.round((then - Date.now()) / 1000);
  const steps = [
    [60, "second"],
    [60, "minute"],
    [24, "hour"],
    [7, "day"],
    [Infinity, "week"],
  ];
  let value = seconds;
  for (const [size, unit] of steps) {
    if (Math.abs(value) < size) {
      return unit === "second" ? "just now" : relativeFormat.format(value, unit);
    }
    value = Math.round(value / size);
  }
  return "";
}

/** Absolute paths under the home folder, written the way Finder abbreviates
 * them. */
function tildePath(path) {
  return String(path || "").replace(/^\/Users\/[^/]+(?=\/|$)/, "~");
}

function emptyRow(text) {
  const row = document.createElement("div");
  row.className = "row empty-row";
  row.textContent = text;
  return row;
}

function setPill(id, text, state) {
  const pill = byId(id);
  pill.className = `pill${state ? ` ${state}` : ""}`;
  pill.textContent = text;
}

/* ------------------------------------------------------------------- panes */

function selectPane(name) {
  if (!PANES.includes(name)) name = "general";
  const changed = name !== currentPane;
  currentPane = name;
  document.querySelectorAll(".nav-item").forEach((item) => {
    if (item.dataset.pane === name) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  });
  document.querySelectorAll(".pane").forEach((pane) => {
    const active = pane.id === `pane-${name}`;
    pane.hidden = !active;
    if (active) {
      byId("pane-title").textContent = pane.dataset.title;
      if (changed) {
        pane.dataset.entering = "true";
        pane.addEventListener("animationend", () => delete pane.dataset.entering, { once: true });
      }
    }
  });
  if (changed) {
    byId("scroller").scrollTop = 0;
    byId("pane-title").parentElement.dataset.scrolled = "false";
  }
  loadPane(name);
}

function loadPane(name) {
  if (name === "general") return Promise.all([loadAppearance(), loadPrefs(), loadUpdateState()]);
  if (name === "agents") return Promise.all([loadPrefs(), loadAgents(), loadActivity()]);
  if (name === "voice") return loadVoiceSettings();
  if (name === "launcher") return loadLauncherSettings();
  if (name === "projects") return Promise.all([loadProjectActions(), loadProjectFolders()]);
  if (name === "about") return loadUpdateState();
  return Promise.resolve();
}

/* -------------------------------------------------------------- appearance */

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

async function loadAppearance() {
  try {
    const appearance = await invoke("get_appearance");
    byId("appearance").value = appearance;
    applyAppearance(appearance);
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

/* ------------------------------------------------------------- preferences */

// Notification, menu-bar and clipboard choices are one saved object in the
// backend, edited from three panes.
async function loadPrefs() {
  try {
    prefs = await invoke("get_notification_preferences");
    renderPrefs();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderPrefs() {
  if (!prefs) return;
  byId("keep-running").checked = prefs.closeBehavior !== "quit";
  byId("notify-needs-input").checked = prefs.notifyNeedsInput;
  byId("notify-task-completed").checked = prefs.notifyTaskCompleted;
  byId("notify-turn-finished").checked = prefs.notifyTurnFinished;
  byId("include-agent-summary").checked = prefs.includeAgentSummary;
  byId("agent-delivery").value = prefs.delivery || "island";
  byId("island-style").value = prefs.islandStyle || "bloom";
  syncIslandStyleRow();
  byId("notification-debounce").value = prefs.debounceSecs;
  byId("clipboard-history-enabled").checked = prefs.clipboardHistoryEnabled;
}

const ISLAND_STYLE_DETAIL = {
  bloom: "Grows out of the notch and settles with a little wobble.",
  buddy: "The Samlu mark comes along: it hops, taps and leans toward you.",
};

/** The style only matters while updates go to the island, so its row hides
 * when they go to Notification Center instead. */
function syncIslandStyleRow() {
  byId("island-style-row").hidden = byId("agent-delivery").value !== "island";
  byId("island-style-detail").textContent =
    ISLAND_STYLE_DETAIL[byId("island-style").value] || ISLAND_STYLE_DETAIL.bloom;
}

function readPrefs() {
  const debounce = Math.round(Number(byId("notification-debounce").value));
  return {
    debounceSecs: Math.max(3, Math.min(300, Number.isFinite(debounce) && debounce > 0 ? debounce : 20)),
    notifyNeedsInput: byId("notify-needs-input").checked,
    notifyTaskCompleted: byId("notify-task-completed").checked,
    notifyTurnFinished: byId("notify-turn-finished").checked,
    includeAgentSummary: byId("include-agent-summary").checked,
    delivery: byId("agent-delivery").value,
    islandStyle: byId("island-style").value,
    clipboardHistoryEnabled: byId("clipboard-history-enabled").checked,
    closeBehavior: byId("keep-running").checked ? "background" : "quit",
  };
}

/** Saves shortly after the last change, so a run of quick toggles is one
 * write, and confirms beside the group that changed. */
function queuePrefsSave(savedId) {
  if (!prefs) return;
  clearTimeout(prefsTimer);
  prefsTimer = setTimeout(async () => {
    const next = readPrefs();
    try {
      await invoke("set_notification_preferences", { preferences: next });
      prefs = next;
      byId("notification-debounce").value = next.debounceSecs;
      if (savedId) showSaved(savedId);
    } catch (error) {
      toast(errorMessage(error), true);
      renderPrefs();
    }
  }, 200);
}

/* ----------------------------------------------------------------- updates */

async function loadUpdateState() {
  try {
    renderUpdate(await invoke("get_update_state"));
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderUpdate(state) {
  if (!state) return;
  updateState = state;
  const { status, available, progress } = state;
  const current = state.currentVersion;
  const next = available?.version;

  byId("sidebar-version").textContent = `Version ${current}`;
  byId("about-version").textContent = `Version ${current}`;
  byId("update-dot").hidden = !["available", "ready"].includes(status);

  const row = byId("update-row");
  row.dataset.status = status;
  const headline = byId("update-headline");
  const detail = byId("update-detail");
  const action = byId("update-action");
  const checked = state.lastCheckedAt ? `Checked ${timeAgo(state.lastCheckedAt)}` : "Not checked yet";

  let showProgress = false;
  switch (status) {
    case "checking":
      headline.textContent = `Samlu ${current}`;
      detail.textContent = "Checking for updates…";
      swapButton(action, '<span class="spinner" aria-hidden="true"></span>Checking…', { disabled: true });
      break;
    case "up_to_date":
      headline.textContent = `Samlu ${current}`;
      detail.textContent = `Samlu is up to date · ${checked}`;
      swapButton(action, "Check Now");
      break;
    case "available":
      headline.textContent = `Samlu ${next} is available`;
      detail.textContent = `You have version ${current}.`;
      swapButton(action, "Install Update", {
        primary: !state.installBlocker,
        disabled: Boolean(state.installBlocker),
      });
      break;
    case "downloading": {
      headline.textContent = `Downloading Samlu ${next}…`;
      const total = progress?.total;
      const received = progress?.received || 0;
      detail.textContent = total
        ? `${formatBytes(received)} of ${formatBytes(total)}`
        : formatBytes(received) || "Starting…";
      setProgress(total ? received / total : null);
      showProgress = true;
      swapButton(action, "Downloading…", { disabled: true });
      break;
    }
    case "installing":
      headline.textContent = `Installing Samlu ${next}…`;
      detail.textContent = "Verifying and replacing the app.";
      showProgress = true;
      swapButton(action, "Installing…", { disabled: true });
      break;
    case "ready":
      headline.textContent = `Samlu ${next || ""} is ready`.replace("  ", " ");
      detail.textContent = "Restart Samlu to finish updating.";
      swapButton(action, "Restart Now", { primary: true });
      break;
    case "error":
      headline.textContent = `Samlu ${current}`;
      detail.textContent = state.error || "The update could not be completed.";
      swapButton(action, "Try Again");
      break;
    default:
      headline.textContent = `Samlu ${current}`;
      detail.textContent = checked;
      swapButton(action, "Check Now");
  }
  byId("update-progress").hidden = !showProgress;
  if (status !== "downloading") setProgress(status === "installing" ? 1 : 0);

  const notes = available?.notes;
  byId("update-notes").hidden = !(notes && ["available", "downloading", "installing", "ready"].includes(status));
  byId("update-notes-title").textContent = next ? `What’s new in ${next}` : "What’s new";
  byId("update-notes-text").textContent = notes || "";

  byId("update-auto-check").checked = state.autoCheck;
  const install = byId("update-auto-install");
  install.checked = state.autoCheck && state.autoInstall;
  install.disabled = !state.autoCheck;

  const note = byId("update-note");
  if (state.installBlocker) {
    note.textContent = state.installBlocker;
    note.hidden = false;
  } else if (!state.signedBuild) {
    note.textContent =
      "This build isn’t signed with an Apple Developer ID, so after each update macOS asks again for Microphone and Accessibility access.";
    note.hidden = false;
  } else {
    note.hidden = true;
  }

  const aboutButton = byId("about-check-updates");
  aboutButton.textContent = status === "ready" ? "Restart to Update" : "Check for Updates…";
  aboutButton.disabled = ["checking", "downloading", "installing"].includes(status);
}

function setProgress(fraction) {
  const fill = byId("update-progress-fill");
  fill.style.transform = fraction == null ? "" : `scaleX(${Math.max(0.02, Math.min(1, fraction))})`;
}

async function updateAction() {
  const status = updateState?.status;
  try {
    if (status === "ready") {
      await invoke("restart_to_update");
    } else if (status === "available" || (status === "error" && updateState?.available)) {
      renderUpdate(await invoke("install_update"));
    } else {
      renderUpdate(await invoke("check_for_updates"));
    }
  } catch (error) {
    toast(errorMessage(error), true);
    loadUpdateState();
  }
}

/** The install switch is shown off while checks are off, but the stored
 * choice is kept, so each switch only ever changes its own setting. */
async function saveUpdatePreferences({ autoCheck, autoInstall }) {
  try {
    renderUpdate(await invoke("set_update_preferences", { autoCheck, autoInstall }));
    showSaved("updates-saved");
  } catch (error) {
    toast(errorMessage(error), true);
    loadUpdateState();
  }
}

/* ------------------------------------------------------------------ agents */

async function loadAgents() {
  try {
    const [status, integrations] = await Promise.all([
      invoke("get_app_status"),
      invoke("get_agent_integrations"),
    ]);
    const listener = byId("listener-status");
    listener.dataset.state = status?.serverRunning ? "good" : "warn";
    byId("listener-text").textContent = status?.serverRunning
      ? `Listening on port ${status.port}`
      : "Listener not running";
    agentIntegrations = integrations || [];
    renderIntegrations();
  } catch (error) {
    agentIntegrations = [];
    byId("agent-integrations").replaceChildren(emptyRow(`Agent status unavailable: ${errorMessage(error)}`));
  }
}

function renderIntegrations() {
  const container = byId("agent-integrations");
  container.replaceChildren();
  if (!agentIntegrations.length) {
    container.append(emptyRow("No supported agents found."));
    return;
  }
  agentIntegrations.forEach((integration) => {
    const row = document.createElement("div");
    row.className = "row integration";

    const logo = document.createElement("span");
    logo.className = "integration-logo";
    logo.setAttribute("aria-hidden", "true");
    logo.textContent = integration.name.slice(0, 1);

    const text = document.createElement("span");
    text.className = "row-text";
    const label = document.createElement("span");
    label.className = "row-label";
    label.textContent = integration.name;
    const detail = document.createElement("span");
    detail.className = "row-detail";
    detail.textContent = integration.error || tildePath(integration.path);
    detail.title = integration.error || integration.path;
    text.append(label, detail);

    const control = document.createElement("span");
    control.className = "row-control";
    const pill = document.createElement("span");
    if (integration.error) {
      pill.className = "pill warn";
      pill.textContent = "Unavailable";
    } else if (integration.installed) {
      pill.className = "pill good";
      pill.textContent = "Connected";
    } else {
      pill.className = "pill";
      pill.textContent = "Not connected";
    }
    const preview = document.createElement("button");
    preview.type = "button";
    preview.className = "text-button";
    preview.dataset.integrationAction = "preview";
    preview.dataset.integrationId = integration.id;
    preview.textContent = "Preview";
    const primary = document.createElement("button");
    primary.type = "button";
    primary.className = "button";
    primary.dataset.integrationAction = integration.installed ? "remove" : "apply";
    primary.dataset.integrationId = integration.id;
    primary.textContent = integration.installed ? "Disconnect" : "Connect";
    primary.disabled = Boolean(integration.error);
    preview.disabled = Boolean(integration.error);
    control.append(pill, preview, primary);

    row.append(logo, text, control);
    container.append(row);
  });
}

async function loadActivity() {
  try {
    renderActivity(await invoke("get_event_history"));
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

const activityTime = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
  month: "short",
  day: "numeric",
});

function renderActivity(items) {
  const list = byId("activity-list");
  list.replaceChildren();
  if (!items?.length) {
    list.append(emptyRow("No agent activity yet."));
    return;
  }
  items.slice(0, 20).forEach((item) => {
    const row = document.createElement("div");
    row.className = "row activity-item";
    const dot = document.createElement("span");
    dot.className = `status-dot ${item.kind_label === "needs your input" ? "warn" : "good"}`;
    dot.setAttribute("aria-hidden", "true");
    const text = document.createElement("span");
    text.className = "row-text";
    const title = document.createElement("span");
    title.className = "row-label";
    title.textContent = `${item.agent_label} ${item.kind_label}`;
    text.append(title);
    const fullDetail = [item.project_label, item.summary].filter(Boolean).join(" · ");
    if (fullDetail) {
      const detail = document.createElement("span");
      detail.className = "row-detail";
      detail.textContent = fullDetail;
      detail.title = fullDetail;
      text.append(detail);
    }
    const time = document.createElement("time");
    time.className = "activity-time";
    time.dateTime = item.timestamp;
    time.textContent = activityTime.format(new Date(item.timestamp));
    row.append(dot, text, time);
    list.append(row);
  });
}

function showAgentPreview(title, code) {
  byId("agent-preview-title").textContent = title;
  byId("agent-preview-code").textContent = code;
  byId("agent-preview").hidden = false;
  byId("agent-preview").scrollIntoView({ behavior: "smooth", block: "nearest" });
}

/* ------------------------------------------------------------------- voice */

async function loadVoiceSettings() {
  try {
    const [settings, microphoneStatus, accessibilityTrusted] = await Promise.all([
      invoke("get_voice_settings"),
      invoke("get_voice_microphone_status"),
      invoke("get_voice_accessibility_status"),
    ]);
    voiceSettings = settings;
    renderSpeechEngine(settings);
    renderTextProcessing(settings.transformation, settings.cleanupDictation);
    byId("voice-vocabulary").value = (settings.vocabulary || []).join("\n");
    voiceHotkeyRecorder.set(settings.hotkey || "Alt+V");
    updateVoiceShortcutGuide(settings.hotkey || "Alt+V");
    byId("voice-dictation-delivery").value = settings.delivery?.dictation || "instant_insert";
    byId("voice-summary-delivery").value = settings.delivery?.summary || "instant_insert";
    byId("voice-prompt-delivery").value = settings.delivery?.prompt || "editable_preview";
    byId("voice-interface-sounds").checked = settings.interfaceSounds !== false;
    byId("voice-keep-history").checked = settings.keepHistory !== false;
    renderMicrophonePermission(microphoneStatus);
    renderAccessibility(accessibilityTrusted);
    await loadVoiceHistory();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

/** Permissions are granted in System Settings, so returning to this window
 * re-reads only them — without disturbing anything being typed here. */
async function refreshPermissions() {
  try {
    const [microphoneStatus, accessibilityTrusted] = await Promise.all([
      invoke("get_voice_microphone_status"),
      invoke("get_voice_accessibility_status"),
    ]);
    renderMicrophonePermission(microphoneStatus);
    renderAccessibility(accessibilityTrusted);
  } catch (_) {
    // The full load reports errors; a background refresh stays quiet.
  }
}

function renderAccessibility(trusted) {
  setPill("voice-accessibility-status", trusted ? "Allowed" : "Not allowed", trusted ? "good" : "warn");
  byId("voice-accessibility-help").textContent = trusted
    ? "Samlu can type the result into the field where you started."
    : T("Not active for this copy of Samlu. If Samlu is already listed in System Settings, remove it and add it again.");
  byId("open-voice-accessibility").textContent = trusted ? "Open Settings…" : "Allow…";
}

function renderMicrophonePermission(status) {
  const help = byId("voice-microphone-help");
  const button = byId("open-voice-microphone");
  if (status === "authorized") {
    setPill("voice-microphone-status", "Allowed", "good");
    help.textContent = "Ready to record in every app and Space.";
    button.textContent = "Open Settings…";
    return;
  }
  if (status === "not_determined") {
    setPill("voice-microphone-status", "Not asked yet", "warn");
    help.textContent = "Allow access once so Samlu can record dictation.";
    button.textContent = "Allow…";
    return;
  }
  if (status === "restricted") {
    setPill("voice-microphone-status", "Restricted", "warn");
    help.textContent = T("This Mac’s privacy policy restricts microphone access.");
    button.textContent = "Open Settings…";
    return;
  }
  setPill("voice-microphone-status", "Not allowed", "warn");
  help.textContent = "Turn on Samlu in Privacy & Security to record dictation.";
  button.textContent = "Open Settings…";
}

async function refreshMicrophonePermission(attempts = 1) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    const status = await invoke("get_voice_microphone_status");
    renderMicrophonePermission(status);
    if (status !== "not_determined") return status;
    if (attempt + 1 < attempts) {
      await new Promise((resolve) => setTimeout(resolve, 400));
    }
  }
  return "not_determined";
}

function setKeyStatus(id, hasKey) {
  setPill(id, hasKey ? "Saved" : "Needed", hasKey ? "good" : "warn");
}

const ENGINE_DETAILS = {
  whisper_cpp: T("Whisper runs on this Mac. No account, and audio never leaves it."),
  apple: "Apple’s on-device speech. Audio never leaves this Mac.",
  groq: "Audio is sent to Groq for transcription.",
  openai: "Audio is sent to OpenAI for transcription.",
  deepgram: "Audio is sent to Deepgram for transcription.",
  elevenlabs: "Audio is sent to ElevenLabs for transcription.",
  custom: "Audio is sent to the OpenAI-compatible server you enter.",
};

function renderSpeechEngine(settings) {
  const stt = settings.stt;
  const choice = stt.choice;
  const appleOption = byId("voice-apple-option");
  appleOption.disabled = !settings.appleAvailable;
  appleOption.textContent = settings.appleAvailable ? "Apple" : "Apple (macOS 26 or later)";
  byId("voice-stt-choice").value = choice;
  byId("voice-language").value = stt.language || "auto";
  byId("voice-engine-detail").textContent = ENGINE_DETAILS[choice] || ENGINE_DETAILS.custom;

  const cloud = !["whisper_cpp", "apple"].includes(choice);
  byId("voice-cloud-model-row").hidden = !cloud;
  byId("voice-cloud-key-row").hidden = !cloud;
  byId("voice-stt-base-url-row").hidden = choice !== "custom";
  byId("voice-apple-row").hidden = choice !== "apple";
  byId("voice-whisper-fields").hidden = choice !== "whisper_cpp";
  byId("voice-stt-base-url").value = stt.baseUrl || "";
  if (cloud) {
    byId("voice-stt-model").value = stt.model || "";
    setKeyStatus("voice-stt-key-status", stt.hasApiKey);
  }

  const englishOnly = choice === "whisper_cpp" && /\.en[.-]/i.test(stt.model || "");
  const note = byId("voice-language-note");
  note.hidden = !englishOnly && choice !== "apple";
  note.textContent = englishOnly
    ? "The selected model only understands English."
    : "Apple has no automatic detection, so Detect automatically uses your Mac’s language.";
  if (choice === "whisper_cpp") loadVoiceModels();
}

function renderTextProcessing(transformation, cleanup) {
  byId("voice-cleanup").checked = cleanup;
  byId("voice-transform-provider").value = transformation.provider;
  byId("voice-transform-model").value = transformation.model || "";
  byId("voice-transform-model-hint").hidden = !["ollama", "lmstudio", "custom"].includes(
    transformation.provider,
  );
  byId("voice-transform-base-url").value = transformation.baseUrl || "";
  byId("voice-transform-base-url-row").hidden = transformation.provider !== "custom";
  byId("voice-transform-key-row").hidden = !transformation.needsApiKey;
  setKeyStatus("voice-transform-key-status", transformation.hasApiKey);
}

function modelRow(title, detail, actions, progress) {
  const row = document.createElement("div");
  row.className = "row model";
  const text = document.createElement("span");
  text.className = "row-text";
  const label = document.createElement("span");
  label.className = "row-label";
  label.textContent = title;
  const small = document.createElement("span");
  small.className = "row-detail";
  small.textContent = detail;
  text.append(label, small);
  if (progress != null) {
    const bar = document.createElement("span");
    bar.className = "progress";
    const fill = document.createElement("span");
    fill.className = "progress-fill";
    fill.style.transform = `scaleX(${Math.max(0.02, progress)})`;
    bar.append(fill);
    text.append(bar);
  }
  const control = document.createElement("span");
  control.className = "row-control";
  actions.forEach(([labelText, handler, primary]) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = primary ? "button primary" : "button";
    button.textContent = labelText;
    button.addEventListener("click", async (event) => {
      try {
        await handler(event.currentTarget);
      } catch (error) {
        toast(errorMessage(error), true);
      }
    });
    control.append(button);
  });
  row.append(text, control);
  return row;
}

function renderVoiceModels() {
  const list = byId("voice-model-list");
  list.replaceChildren();
  if (!voiceModels.models.length) {
    list.append(emptyRow("No Whisper models yet. Download one below, or add a file you already have."));
  }
  const sourceLabel = { downloaded: "Downloaded", discovered: T("Found on this Mac"), added: "Added" };
  voiceModels.models.forEach((model) => {
    const selected = model.path === voiceModels.selected;
    const detail = [
      model.missing ? "Missing" : formatBytes(model.bytes),
      sourceLabel[model.source],
      model.englishOnly ? "English only" : "",
      selected ? "In use" : "",
    ]
      .filter(Boolean)
      .join(" · ");
    const actions = [];
    if (!selected && !model.missing) {
      actions.push([
        "Use",
        async () => {
          await invoke("set_voice_stt", { choice: "whisper_cpp", baseUrl: "", model: model.path });
          showSaved("voice-engine-saved");
          await loadVoiceSettings();
        },
      ]);
    }
    if (model.source === "downloaded") {
      actions.push([
        "Delete…",
        async () => {
          const confirmed = await confirmAction(
            `Delete ${model.name}?`,
            T("The model file is removed from this Mac. You can download it again later."),
            "Delete",
          );
          if (!confirmed) return;
          await invoke("voice_delete_model", { path: model.path });
          await loadVoiceModels();
        },
      ]);
    } else if (model.source === "added") {
      actions.push([
        "Remove",
        async () => {
          await invoke("voice_remove_model", { path: model.path });
          await loadVoiceModels();
        },
      ]);
    }
    list.append(modelRow(model.name, detail, actions));
  });

  const catalog = byId("voice-model-catalog");
  catalog.replaceChildren();
  voiceModels.catalog.forEach((entry) => {
    const progress = downloadProgress[entry.id];
    const recommended = entry.id === voiceModels.recommended ? T(" · Recommended for this Mac") : "";
    let detail = `${formatBytes(entry.bytes)}${recommended}`;
    let actions = [];
    let fraction = null;
    if (entry.downloaded) {
      detail = `Downloaded${recommended}`;
    } else if (entry.downloading) {
      fraction = progress?.total ? progress.received / progress.total : 0;
      detail = `Downloading ${Math.floor(fraction * 100)}% of ${formatBytes(entry.bytes)}`;
      actions = [["Cancel", () => invoke("voice_cancel_model_download", { id: entry.id })]];
    } else {
      actions = [
        [
          "Download",
          async () => {
            const download = invoke("voice_download_model", { id: entry.id });
            await loadVoiceModels();
            try {
              const path = await download;
              // The first usable model is selected so dictation works at once.
              if (!voiceModels.selected) {
                await invoke("set_voice_stt", { choice: "whisper_cpp", baseUrl: "", model: path });
              }
              showSaved("voice-engine-saved", "Model ready");
            } catch (error) {
              if (errorMessage(error) !== "Download cancelled.") throw error;
            } finally {
              delete downloadProgress[entry.id];
              await loadVoiceSettings();
            }
          },
          entry.id === voiceModels.recommended,
        ],
      ];
    }
    catalog.append(modelRow(entry.label, detail, actions, fraction));
  });
}

async function loadVoiceModels() {
  try {
    voiceModels = await invoke("get_voice_models");
    renderVoiceModels();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function saveSpeechChoice() {
  const choice = byId("voice-stt-choice").value;
  const baseUrl = byId("voice-stt-base-url").value.trim();
  if (choice === "custom" && !baseUrl) {
    // A custom engine needs its address before it can be saved.
    byId("voice-stt-base-url-row").hidden = false;
    byId("voice-cloud-model-row").hidden = false;
    byId("voice-cloud-key-row").hidden = false;
    byId("voice-whisper-fields").hidden = true;
    byId("voice-apple-row").hidden = true;
    byId("voice-stt-base-url").focus();
    return;
  }
  let model = choice === voiceSettings?.stt.choice ? byId("voice-stt-model").value : "";
  if (choice === "whisper_cpp") {
    // Keep the current Whisper model, or fall back to the first usable one.
    await loadVoiceModels();
    model = voiceModels.selected || voiceModels.models.find((item) => !item.missing)?.path || "";
  }
  await invoke("set_voice_stt", { choice, baseUrl, model });
  showSaved("voice-engine-saved");
  await loadVoiceSettings();
}

async function loadVoiceHistory() {
  try {
    voiceHistory = await invoke("get_voice_history", { query: byId("voice-history-search").value });
    renderVoiceHistory();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderVoiceHistory() {
  const list = byId("voice-history-list");
  const showRaw = byId("voice-history-raw").checked;
  list.replaceChildren();
  if (!voiceHistory.length) {
    list.append(
      emptyRow(
        byId("voice-keep-history").checked
          ? byId("voice-history-search").value
            ? "No dictations match."
            : "No dictations yet."
          : "History is off.",
      ),
    );
    return;
  }
  voiceHistory.forEach((entry) => {
    const row = document.createElement("div");
    row.className = "row";
    const text = document.createElement("span");
    text.className = "row-text";
    const body = document.createElement("span");
    body.className = "history-text";
    body.textContent = showRaw && entry.raw ? entry.raw : entry.text;
    const meta = document.createElement("span");
    meta.className = "row-detail";
    const when = new Date(entry.timestamp).toLocaleString(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    });
    const outcome = String(entry.outcome || "").replace(/_/g, " ");
    meta.textContent = [when, entry.mode, entry.engine, outcome, entry.targetApp]
      .filter(Boolean)
      .join(" · ");
    text.append(body, meta);
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "text-button";
    copy.textContent = "Copy";
    copy.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(body.textContent);
        showSaved("voice-history-saved", "Copied");
      } catch (error) {
        toast(errorMessage(error), true);
      }
    });
    row.append(text, copy);
    list.append(row);
  });
}

/* --------------------------------------------------------------- shortcuts */

const GLYPHS = {
  cmd: "⌘",
  command: "⌘",
  super: "⌘",
  meta: "⌘",
  shift: "⇧",
  alt: "⌥",
  option: "⌥",
  ctrl: "⌃",
  control: "⌃",
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
Object.assign(GLYPHS, PLATFORM.hotkeySymbols);

function keyGlyph(part) {
  const trimmed = part.trim();
  const glyph = GLYPHS[trimmed.toLowerCase()] || trimmed.toUpperCase();
  // Named keys read as words on a keycap: "Space", not "SPACE".
  return glyph.length > 1 ? glyph[0] + glyph.slice(1).toLowerCase() : glyph;
}

function keycaps(shortcut) {
  return String(shortcut || "")
    .split("+")
    .filter((part) => part.trim())
    .map((part) => {
      const key = document.createElement("kbd");
      key.textContent = keyGlyph(part);
      return key;
    });
}

function addShortcutModifier(shortcut, modifier) {
  const parts = shortcut.split("+").map((part) => part.trim()).filter(Boolean);
  // A chord made only of modifiers has no key to keep last.
  if (parts.length && parts.every(isModifierName)) return [...parts, modifier].join("+");
  const key = parts.pop() || "V";
  return [...parts, modifier, key].join("+");
}

function renderKeys(id, shortcut) {
  const holder = byId(id);
  if (!holder) return;
  holder.replaceChildren(...keycaps(shortcut));
  holder.setAttribute(
    "aria-label",
    shortcut ? shortcut.split("+").map(keyGlyph).join(PLATFORM.keyJoiner) : "Not set",
  );
}

function updateVoiceShortcutGuide(hotkey) {
  renderKeys("voice-normal-key", hotkey);
  renderKeys("voice-summary-key", addShortcutModifier(hotkey, "Shift"));
  renderKeys("voice-prompt-key", addShortcutModifier(hotkey, "Cmd"));
}

// Ctrl, Alt, Shift, Cmd, then the key: the exact spelling and order the global
// shortcut backend parses, e.g. "Alt+V".
const SHORTCUT_MODIFIERS = [
  ["ctrlKey", "Ctrl"],
  ["altKey", "Alt"],
  ["shiftKey", "Shift"],
  ["metaKey", "Cmd"],
];

const isModifierName = (part) => SHORTCUT_MODIFIERS.some(([, name]) => name === part);

const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "OS", "Fn", "FnLock", "CapsLock", "Hyper", "Super"]);

// Physical key (KeyboardEvent.code) to the name the backend's parser accepts.
// Anything missing here can't be registered as a global shortcut on macOS.
const SHORTCUT_KEYS = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Backquote: "Backquote",
  Minus: "Minus",
  Equal: "Equal",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Backslash: "Backslash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  NumpadAdd: "NumAdd",
  NumpadSubtract: "NumSubtract",
  NumpadMultiply: "NumMultiply",
  NumpadDivide: "NumDivide",
  NumpadDecimal: "NumDecimal",
  NumpadEnter: "NumEnter",
  NumpadEqual: "NumEqual",
};

function shortcutKeyName(code) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^Numpad[0-9]$/.test(code)) return `Num${code.slice(6)}`;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return SHORTCUT_KEYS[code] || "";
}

function heldModifiers(event) {
  return SHORTCUT_MODIFIERS.filter(([flag]) => event[flag]).map(([, name]) => name);
}

/** Reads a chord from a keydown. Returns `{ shortcut }` when it can be
 * registered, or `{ hint }` explaining why not. */
function shortcutFromEvent(event) {
  // Some keyboards leave `code` empty for the higher function keys.
  const key = shortcutKeyName(event.code || (/^F\d+$/.test(event.key) ? event.key : ""));
  if (!key) return { hint: "Key not supported" };
  const modifiers = heldModifiers(event);
  // Function keys work alone; everything else would swallow ordinary typing.
  if (!modifiers.length && !/^F\d+$/.test(key)) return { hint: PLATFORM.windows ? "Add Ctrl, Alt, Shift or Win" : "Add ⌃, ⌥, ⇧ or ⌘" };
  return { shortcut: [...modifiers, key].join("+") };
}

// Only one field records at a time; arming another one cancels it.
let activeRecorder = null;

/** Shortcut fields record a chord instead of accepting typed text, so the
 * stored value can never drift from the format the backend registers.
 *
 * Keys are read at the window, not the button: WebKit on macOS doesn't focus a
 * button when it is clicked, so listeners on the button would miss the chord. */
function createShortcutRecorder(id, save, restore, validate = () => "") {
  const button = byId(id);
  let value = "";
  let recording = false;
  let saving = false;
  // The modifiers held at the last change, and whether a regular key landed
  // while they were down: letting go of two or more modifiers on their own
  // records a modifier-only chord such as ⌃⌥.
  let chordModifiers = [];
  let keyLanded = false;

  const paint = (content, armed) => {
    if (typeof content === "string") button.textContent = content;
    else button.replaceChildren(...content);
    button.classList.toggle("recording", armed);
    button.setAttribute("aria-pressed", armed ? "true" : "false");
  };

  const prompt = (hint) => paint(hint || "Type shortcut…", true);

  const detach = () => {
    window.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("keyup", onKeyUp, true);
    window.removeEventListener("blur", cancel);
    document.removeEventListener("pointerdown", onPointerDown, true);
    if (activeRecorder === cancel) activeRecorder = null;
  };

  const rest = () => {
    const wasRecording = recording;
    recording = false;
    saving = false;
    chordModifiers = [];
    keyLanded = false;
    detach();
    paint(value ? keycaps(value) : "Not set", false);
    // Hand the chords back to the system once the recorder is done with them.
    if (wasRecording) invoke("resume_global_shortcuts").catch(() => {});
  };

  function cancel() {
    if (recording && !saving) rest();
  }

  const arm = () => {
    if (activeRecorder) activeRecorder();
    recording = true;
    activeRecorder = cancel;
    prompt();
    button.focus({ preventScroll: true });
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", cancel);
    document.addEventListener("pointerdown", onPointerDown, true);
    // Samlu's own bindings step aside, or pressing the shortcut that is
    // already assigned would fire it instead of being recorded.
    invoke("suspend_global_shortcuts").catch(() => {});
  };

  // Show held modifiers as they go down and up, so the user sees the chord
  // forming before the final key lands.
  const previewModifiers = (event) => {
    const modifiers = heldModifiers(event);
    if (modifiers.length) paint([...keycaps(modifiers.join("+")), document.createTextNode("…")], true);
    else prompt();
  };

  function onPointerDown(event) {
    if (!button.contains(event.target)) cancel();
  }

  function onKeyUp(event) {
    if (!recording || saving) return;
    event.preventDefault();
    event.stopPropagation();
    if (!MODIFIER_KEYS.has(event.key)) return;
    // The first modifier to come up ends a chord that never got a key.
    const released = chordModifiers;
    const clean = !keyLanded;
    chordModifiers = heldModifiers(event);
    if (!chordModifiers.length) keyLanded = false;
    if (clean && released.length >= 2) {
      commit(released.join("+"));
      return;
    }
    if (clean && released.length === 1 && !chordModifiers.length) {
      prompt("Hold two modifiers or add a key");
      return;
    }
    previewModifiers(event);
  }

  async function onKeyDown(event) {
    if (!recording) return;
    // While armed every key belongs to the chord, including Tab and Space.
    event.preventDefault();
    event.stopPropagation();
    if (saving || event.repeat || event.isComposing) return;
    if (event.key === "Escape" && !heldModifiers(event).length) {
      rest();
      return;
    }
    if (MODIFIER_KEYS.has(event.key)) {
      chordModifiers = heldModifiers(event);
      previewModifiers(event);
      return;
    }
    keyLanded = true;
    const { shortcut, hint } = shortcutFromEvent(event);
    await commit(shortcut, hint);
  }

  async function commit(shortcut, hint) {
    const problem = hint || validate(shortcut);
    if (problem) {
      prompt(problem);
      return;
    }
    const previous = value;
    value = shortcut;
    saving = true;
    // Show the chord at once, but stay armed until the save lands so the
    // rebind happens before the shortcuts are handed back.
    paint(keycaps(shortcut), false);
    try {
      await save(shortcut);
      rest();
    } catch (error) {
      value = previous;
      rest();
      toast(errorMessage(error), true);
      await restore();
    }
  }

  button.addEventListener("click", () => {
    if (!recording) arm();
    else cancel();
  });
  button.addEventListener("blur", cancel);

  return {
    set(next) {
      value = next || "";
      if (!recording) rest();
    },
  };
}

/* ---------------------------------------------------------------- launcher */

async function loadLauncherSettings() {
  try {
    const [hotkey, preferences, storedSnippets] = await Promise.all([
      invoke("get_launcher_hotkey"),
      invoke("get_notification_preferences"),
      invoke("get_snippets"),
    ]);
    launcherHotkeyRecorder.set(hotkey);
    prefs = preferences;
    renderPrefs();
    snippets = storedSnippets;
    renderSnippets();
  } catch (error) {
    toast(errorMessage(error), true);
  }
  loadDeepFind();
}

/* -------------------------------------------------------- descriptive find */

async function loadDeepFind() {
  try {
    deepFind = await invoke("get_deep_find_settings");
    renderDeepFind();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function deepFindPill(status) {
  if (!deepFind.enabled) return ["Off", ""];
  if (status.state === "error") return ["Needs attention", "warn"];
  if (status.state === "indexing") {
    const progress = status.total ? ` ${status.done.toLocaleString()} of ${status.total.toLocaleString()}` : "";
    return [`${status.phase.replace(/…$/, "")}${progress}`, ""];
  }
  const files = status.files;
  return [`${files.toLocaleString()} ${files === 1 ? "file" : "files"}`, "good"];
}

function renderDeepFind() {
  if (!deepFind) return;
  const status = deepFind.status;
  byId("deep-find-enabled").checked = deepFind.enabled;
  byId("deep-find-details").hidden = !deepFind.enabled;
  byId("deep-find-code").checked = deepFind.includeCode;
  const [pill, state] = deepFindPill(status);
  setPill("deep-find-pill", pill, state);

  const model = byId("deep-find-model");
  model.replaceChildren();
  const label = document.createElement("div");
  label.className = "list-label";
  label.textContent = "Meaning model";
  model.append(label);
  DEEP_FIND_MODELS.forEach((option) => {
    if (option.id === "large" && !status.largeSupported) return;
    model.append(deepFindModelRow(option, status));
  });

  renderPathList("deep-find-root-list", deepFind.roots, "No folders yet. Add one to start.", (path) =>
    saveDeepFindFolders({ roots: deepFind.roots.filter((value) => value !== path) }),
  );
  renderPathList("deep-find-exclusion-list", deepFind.excluded, "None.", (path) =>
    saveDeepFindFolders({ excluded: deepFind.excluded.filter((value) => value !== path) }),
  );
  const usage = [
    `${status.passages.toLocaleString()} passages`,
    status.vectors ? `${status.vectors.toLocaleString()} understood by meaning` : "",
    status.error || "",
  ].filter(Boolean);
  byId("deep-find-usage").textContent = `${usage.join(" · ")}. Stored on this computer only.`;
}

const DEEP_FIND_MODELS = [
  {
    id: "large",
    title: "Large: EmbeddingGemma 2",
    about: "Google DeepMind’s model, as in DigUp. Best at descriptions, 100+ languages. Runs on the GPU.",
  },
  {
    id: "small",
    title: "Small: Model2Vec retrieval",
    about: "Fast and light on any computer. Answers in microseconds on the CPU.",
  },
  { id: "none", title: "None", about: "Exact words only. Nothing to download." },
];

function deepFindModelRow(option, status) {
  const chosen = deepFind.model === option.id;
  const downloaded = option.id === "none" || status[`${option.id}Downloaded`];
  const bytes = status[`${option.id}Bytes`];
  const parts = [option.about];
  const actions = [];
  let fraction = null;
  if (status.downloading === option.id) {
    fraction = deepFindDownload?.model === option.id && deepFindDownload.total
      ? deepFindDownload.received / deepFindDownload.total
      : 0;
    parts.push(`Downloading ${Math.floor(fraction * 100)}% of ${formatBytes(bytes)}`);
    actions.push(["Cancel", () => invoke("deep_find_cancel_download")]);
  } else {
    if (option.id !== "none") parts.push(downloaded ? `${formatBytes(bytes)} downloaded` : `${formatBytes(bytes)} download`);
    if (chosen) {
      parts.push(
        option.id === "none" || !downloaded
          ? "In use"
          : status.modelLoaded === option.id
            ? "In use · Ready"
            : "In use · Loads when you search",
      );
      if (!downloaded) actions.push(["Download", () => downloadDeepFindModel(option.id), true]);
    } else {
      actions.push([
        downloaded ? "Use" : "Download & Use",
        () => chooseDeepFindModel(option.id, downloaded),
        false,
      ]);
    }
  }
  return modelRow(option.title, parts.join(" · "), actions, fraction);
}

async function chooseDeepFindModel(model, downloaded) {
  deepFind = await invoke("set_deep_find_model", { model });
  renderDeepFind();
  if (!downloaded) await downloadDeepFindModel(model);
}

async function downloadDeepFindModel(model = deepFind.model) {
  if (model === "none") return;
  const download = invoke("deep_find_download_model", { model });
  // The status flips to downloading as soon as the command starts.
  setTimeout(loadDeepFind, 150);
  try {
    deepFind = await download;
    showSaved("launcher-saved", "Model ready");
  } catch (error) {
    if (errorMessage(error) !== "Download cancelled.") throw error;
  } finally {
    deepFindDownload = null;
    await loadDeepFind();
  }
}

async function saveDeepFindFolders(change) {
  try {
    const next = {
      roots: deepFind.roots,
      excluded: deepFind.excluded,
      includeCode: deepFind.includeCode,
      ...change,
    };
    deepFind = await invoke("set_deep_find_folders", next);
    renderDeepFind();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function addDeepFindFolder(kind, button) {
  try {
    const path = await runButton(button, "Choosing…", () =>
      invoke("deep_find_choose_folder", {
        title: kind === "root" ? "Choose a folder to search inside" : "Choose a folder to leave out",
      }),
    );
    if (!path) return;
    const key = kind === "root" ? "roots" : "excluded";
    if (deepFind[key].includes(path)) {
      toast(kind === "root" ? "That folder is already searched." : "That folder is already excluded.");
      return;
    }
    await saveDeepFindFolders({ [key]: [...deepFind[key], path] });
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderPathList(id, paths, emptyLabel, onRemove) {
  const list = byId(id);
  list.replaceChildren();
  if (!paths.length) {
    const empty = emptyRow(emptyLabel);
    empty.classList.add("path");
    list.append(empty);
    return;
  }
  paths.forEach((path) => {
    const row = document.createElement("div");
    row.className = "row path";
    const text = document.createElement("span");
    text.className = "row-text";
    const label = document.createElement("span");
    label.className = "row-label";
    // Read right to left so a long path keeps its last folders in view.
    label.textContent = `\u200e${tildePath(path)}\u200e`;
    label.title = path;
    text.append(label);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "text-button danger";
    remove.textContent = "Remove";
    remove.setAttribute("aria-label", `Remove ${path}`);
    remove.addEventListener("click", () => onRemove(path));
    row.append(text, remove);
    list.append(row);
  });
}

function renderSnippets(activeId = byId("snippet-id").value) {
  const list = byId("snippet-list");
  list.replaceChildren();
  if (!snippets.length) {
    list.append(emptyRow("No snippets yet."));
    return;
  }
  snippets.forEach((snippet) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "snippet-item";
    if (snippet.id === activeId) button.setAttribute("aria-current", "true");
    const title = document.createElement("strong");
    title.textContent = snippet.title;
    const keyword = document.createElement("span");
    keyword.textContent = snippet.keyword ? `;${snippet.keyword}` : "No keyword";
    button.append(title, keyword);
    button.addEventListener("click", () => editSnippet(snippet));
    list.append(button);
  });
}

function editSnippet(snippet = { id: "", title: "", keyword: "", content: "" }) {
  byId("snippet-form").hidden = false;
  byId("snippet-empty").hidden = true;
  byId("snippet-id").value = snippet.id;
  byId("snippet-title").value = snippet.title;
  byId("snippet-keyword").value = snippet.keyword;
  byId("snippet-content").value = snippet.content;
  byId("delete-snippet").hidden = !snippet.id;
  renderSnippets(snippet.id);
  byId("snippet-title").focus();
}

/* ---------------------------------------------------------------- projects */

// The keys a project result answers to, in the order the backend lists them.
const PROJECT_CHORDS = ["enter", "cmd+enter", "alt+enter", "ctrl+enter", "shift+enter"];

const CHORD_NAMES = {
  enter: "Return",
  "cmd+enter": "Command-Return",
  "alt+enter": "Option-Return",
  "ctrl+enter": "Control-Return",
  "shift+enter": "Shift-Return",
};

// Terminals that can run commands, named even after they are uninstalled and
// so missing from the catalog.
const TERMINAL_NAMES = { warp: "Warp", ghostty: "Ghostty", iterm: "iTerm", terminal: "Terminal" };

// The App pop-up's last entry. A colon never appears in a bundle id.
const CHOOSE_APP = ":choose";

const PLAY_GLYPH = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3.6v8.8l7-4.4z"/></svg>';

const ACTION_GLYPHS = {
  open: '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="2.5" y="3" width="11" height="10" rx="2"/><path d="M2.5 6.2h11"/></svg>',
  reveal:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4.6c0-.7.5-1.2 1.2-1.2h2.6l1.4 1.5h5.6c.7 0 1.2.5 1.2 1.2v5.5c0 .7-.5 1.2-1.2 1.2H3.2c-.7 0-1.2-.5-1.2-1.2z"/></svg>',
  copy_path:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="5.2" y="5.2" width="8.3" height="8.3" rx="1.6"/><path d="M10.8 3.4c-.2-.5-.7-.9-1.3-.9H3.9c-.8 0-1.4.6-1.4 1.4v5.6c0 .6.4 1.1.9 1.3"/></svg>',
  url: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6.9 9.1a2.5 2.5 0 0 0 3.6 0l2.1-2.1a2.5 2.5 0 0 0-3.6-3.6l-.8.8M9.1 6.9a2.5 2.5 0 0 0-3.6 0L3.4 9a2.5 2.5 0 0 0 3.6 3.6l.8-.8"/></svg>',
  terminal:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="2" y="2.8" width="12" height="10.4" rx="2"/><path d="m5 6.4 2 1.6-2 1.6M8.6 10.1h2.6"/></svg>',
  task: PLAY_GLYPH,
  script: PLAY_GLYPH,
};

// Every field any kind of action has, so the editor can switch kinds back and
// forth without losing what was typed.
const BLANK_ACTION = {
  id: "",
  title: "",
  kind: "open",
  bundleId: "",
  appPath: "",
  url: "",
  command: "",
  terminal: "",
  cwd: "",
  steps: [],
  continueOnError: false,
  scripts: [],
  mode: "terminal",
};

async function loadProjectActions() {
  try {
    projectActions = await invoke("get_project_actions");
    renderProjectActions();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

/** Draws what the backend found and saved. An open editor keeps what is being
 * typed; only its pop-ups are refreshed. */
function renderProjectActions() {
  renderFoundNote();
  applyProjectConfig(projectActions.config);
  if (byId("project-action-form").hidden) return;
  const id = byId("project-action-id").value;
  if (id && !projectActions.config.actions.some((action) => action.id === id)) {
    closeActionEditor();
    return;
  }
  const bundleId = selectedAppId();
  fillAppSelect(bundleId, appChoices.get(bundleId)?.appPath || "");
}

/** Shows a configuration the backend returned. The editor is left alone, so
 * an unsaved draft survives a key change or a reorder. */
function applyProjectConfig(config) {
  projectActions.config = config;
  renderProjectKeys();
  renderActionList();
  renderAddActionMenu();
  renderProjectRunning();
  const terminal = byId("project-action-terminal");
  fillTerminalSelect(terminal, terminal.value, config.terminal || projectActions.catalog.defaultTerminal);
}

function renderProjectKeys() {
  const { actions, bindings } = projectActions.config;
  document.querySelectorAll("select[data-chord]").forEach((select) => {
    select.replaceChildren(
      new Option("Show Actions", ""),
      ...actions.map((action) => new Option(action.title, action.id)),
    );
    select.value = bindings[select.dataset.chord] || "";
  });
}

async function bindProjectKey(select) {
  if (!projectActions) return;
  try {
    applyProjectConfig(
      await invoke("set_project_action_binding", {
        chord: select.dataset.chord,
        actionId: select.value || null,
      }),
    );
    showSaved("projects-saved");
  } catch (error) {
    toast(errorMessage(error), true);
    renderProjectKeys();
  }
}

/** "cmd+enter" as one keycap, "⌘↩". */
function chordGlyphs(chord) {
  return chord
    .split("+")
    .map((part) => PLATFORM.actionSymbols[part] || keyGlyph(part))
    .join(PLATFORM.keyJoiner);
}

function boundChords(id) {
  const { bindings } = projectActions.config;
  return PROJECT_CHORDS.filter((chord) => bindings[chord] === id);
}

/** An app's name: Samlu's own for apps it knows, else the bundle's file
 * name. */
function appName(bundleId, appPath) {
  const known = projectActions.catalog.apps.find((app) => app.bundleId === bundleId) || chosenApps.get(bundleId);
  if (known) return known.name;
  return (
    String(appPath || "")
      .split(/[\\/]/)
      .pop()
      .replace(/\.(app|exe)$/i, "") || bundleId
  );
}

function actionSubtitle(action) {
  switch (action.kind) {
    case "open":
      return appName(action.bundleId, action.appPath);
    case "reveal":
      return T("Finder");
    case "copy_path":
      return "Path to the clipboard";
    case "url":
      return action.url;
    case "terminal":
      return action.command;
    case "task":
      return (action.steps || []).map((step) => step.title || step.command).join(" → ");
    case "script":
      return (action.scripts || []).join(" · ");
    default:
      return "";
  }
}

function renderActionList() {
  const list = byId("project-action-list");
  const { actions } = projectActions.config;
  const activeId = byId("project-action-form").hidden ? null : byId("project-action-id").value;
  list.replaceChildren();
  if (!actions.length) list.append(emptyRow("No actions yet. Add one above."));
  actions.forEach((action) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "action-item";
    if (action.id === activeId) button.setAttribute("aria-current", "true");
    const glyph = document.createElement("span");
    glyph.className = "action-glyph";
    glyph.innerHTML = ACTION_GLYPHS[action.kind] || PLAY_GLYPH;
    const text = document.createElement("span");
    text.className = "action-text";
    const subtitle = actionSubtitle(action);
    const title = document.createElement("strong");
    title.textContent = action.title;
    const detail = document.createElement("span");
    detail.textContent = subtitle;
    text.append(title, detail);
    button.append(glyph, text);
    const chords = boundChords(action.id);
    // Keys are named in words; the keycaps alone would be read as symbols.
    button.setAttribute(
      "aria-label",
      [action.title, subtitle, ...chords.map((chord) => CHORD_NAMES[chord])].filter(Boolean).join(", "),
    );
    if (chords.length) {
      const keys = document.createElement("span");
      keys.className = "keys";
      keys.setAttribute("aria-hidden", "true");
      chords.forEach((chord) => {
        const key = document.createElement("kbd");
        key.textContent = chordGlyphs(chord);
        keys.append(key);
      });
      button.append(keys);
    }
    button.addEventListener("click", () => editAction(action));
    list.append(button);
  });
  const index = actions.findIndex((action) => action.id === activeId);
  byId("move-project-action-up").disabled = index <= 0;
  byId("move-project-action-down").disabled = index < 0 || index === actions.length - 1;
}

/** Presets for what is installed, less the ones already added. */
function renderAddActionMenu() {
  const added = new Set(projectActions.config.actions.map((action) => action.id));
  const groups = { open: [], terminal: [], project: [] };
  projectActions.catalog.presets
    .filter((preset) => !added.has(preset.id))
    .forEach((preset) => (groups[preset.kind] || groups.project).push(new Option(preset.title, `preset:${preset.id}`)));
  byId("add-action-apps").replaceChildren(...groups.open);
  byId("add-action-tools").replaceChildren(...groups.terminal);
  byId("add-action-project").replaceChildren(...groups.project);
}

function renderProjectRunning() {
  const { config, catalog } = projectActions;
  fillTerminalSelect(byId("project-terminal"), config.terminal, catalog.defaultTerminal);
  byId("project-script-mode").value = config.scriptMode;
  byId("project-notify-finished").checked = config.notifyTaskFinished;
  byId("project-notify-url").checked = config.notifyTaskUrl;
}

/** "A, B and C". */
function listNames(names) {
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** Says what detection found, so a CLI missing from Add Action is explained
 * before anyone goes looking for it. */
function renderFoundNote() {
  const { clis, terminals } = projectActions.catalog;
  const found = [clis, terminals].map((items) => listNames(items.map((item) => item.name))).filter(Boolean);
  byId("project-found-note").textContent = clis.length
    ? `Found ${found.join(" · ")}.`
    : "No agent CLIs found on your PATH.";
}

function terminalName(id) {
  return projectActions.catalog.terminals.find((terminal) => terminal.id === id)?.name || TERMINAL_NAMES[id] || id;
}

/** A terminal pop-up: the default, named, then each terminal Samlu can run
 * commands in. A choice that is no longer one of those stays listed, so the
 * pop-up never claims something the action doesn't say. */
function fillTerminalSelect(select, value, defaultId) {
  const options = [new Option(`Default (${terminalName(defaultId)})`, "")];
  projectActions.catalog.terminals
    .filter((terminal) => terminal.runsCommands)
    .forEach((terminal) => options.push(new Option(terminal.name, terminal.id)));
  if (value && !options.some((option) => option.value === value)) {
    const installed = projectActions.catalog.terminals.some((terminal) => terminal.id === value);
    options.push(
      new Option(`${terminalName(value)} (${installed ? "can’t run commands" : "not installed"})`, value),
    );
  }
  select.replaceChildren(...options);
  select.value = value;
}

/** Opens an action in the editor: a saved one, or a new draft with no id. */
function editAction(action, { focus = true } = {}) {
  const draft = { ...BLANK_ACTION, ...action };
  byId("project-action-form").hidden = false;
  byId("project-action-empty").hidden = true;
  byId("project-action-id").value = draft.id;
  byId("project-action-title").value = draft.title;
  byId("project-action-kind").value = draft.kind;
  fillAppSelect(draft.bundleId, draft.appPath);
  byId("project-action-url").value = draft.url;
  byId("project-action-command").value = draft.command;
  fillTerminalSelect(
    byId("project-action-terminal"),
    draft.terminal,
    projectActions.config.terminal || projectActions.catalog.defaultTerminal,
  );
  byId("project-action-cwd").value = draft.cwd;
  renderSteps(draft.steps.length ? draft.steps : [{ title: "", command: "", cwd: "" }]);
  byId("project-action-continue").checked = draft.continueOnError;
  byId("project-action-scripts").value = draft.scripts.join(", ");
  byId("project-action-mode").value = draft.mode;
  byId("delete-project-action").hidden = !draft.id;
  syncActionKind();
  renderActionList();
  if (focus) byId("project-action-title").focus();
}

function closeActionEditor() {
  byId("project-action-form").hidden = true;
  byId("project-action-empty").hidden = false;
  byId("project-action-id").value = "";
  renderActionList();
}

/** Shows the fields for the chosen kind. The others keep their values, so
 * changing your mind loses nothing. */
function syncActionKind() {
  const kind = byId("project-action-kind").value;
  byId("project-action-form")
    .querySelectorAll("[data-kind]")
    .forEach((element) => {
      element.hidden = element.dataset.kind !== kind;
    });
}

/** The App pop-up: installed apps Samlu knows, apps chosen this session, the
 * action's own app even when Samlu can't find it, then Choose App…. */
function fillAppSelect(bundleId, appPath) {
  appChoices = new Map();
  projectActions.catalog.apps.forEach((app) =>
    appChoices.set(app.bundleId, { name: app.name, appPath: app.appPath }),
  );
  chosenApps.forEach((app, id) => {
    if (!appChoices.has(id)) appChoices.set(id, app);
  });
  const options = [...appChoices].map(([id, app]) => new Option(app.name, id));
  if (bundleId && !appChoices.has(bundleId)) {
    const name = appName(bundleId, appPath);
    appChoices.set(bundleId, { name, appPath });
    // A preset whose app has gone. An app chosen by hand can't be checked
    // from here, so it keeps its plain name.
    const missing = byId("project-action-id").value.startsWith("open-");
    options.push(new Option(missing ? `${name} (not installed)` : name, bundleId));
  }
  options.push(new Option("Choose App…", CHOOSE_APP));
  const select = byId("project-action-app");
  select.replaceChildren(...options);
  select.value = bundleId;
  appBeforeChoosing = bundleId;
}

function selectedAppId() {
  const value = byId("project-action-app").value;
  return value === CHOOSE_APP ? "" : value;
}

async function changeActionApp(select) {
  const previous = appChoices.get(appBeforeChoosing)?.name;
  if (select.value !== CHOOSE_APP) {
    appBeforeChoosing = select.value;
    nameAfterApp(previous, appChoices.get(select.value)?.name);
    return;
  }
  // The picker can be cancelled, so the previous app stays until it returns.
  select.value = appBeforeChoosing;
  try {
    const chosen = await invoke("choose_application");
    if (!chosen) return;
    chosenApps.set(chosen.bundleId, { name: chosen.name, appPath: chosen.appPath });
    fillAppSelect(chosen.bundleId, chosen.appPath);
    nameAfterApp(previous, appChoices.get(chosen.bundleId)?.name);
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

/** An empty title, or one still reading "Open in <the previous app>",
 * follows the app. A title you wrote yourself is left alone. */
function nameAfterApp(previous, name) {
  if (!name) return;
  const title = byId("project-action-title");
  const current = title.value.trim();
  if (!current || (previous && current === `Open in ${previous}`)) title.value = `Open in ${name}`;
}

function renderSteps(steps, focusIndex = -1) {
  const list = byId("project-action-steps");
  list.replaceChildren(...steps.map((step, index) => stepRow(step, index, steps.length)));
  list.children[focusIndex]?.querySelector("input")?.focus();
}

function stepRow(step, index, count) {
  const row = document.createElement("div");
  row.className = "step";
  const number = document.createElement("span");
  number.className = "step-number";
  number.setAttribute("aria-hidden", "true");
  number.textContent = String(index + 1);
  const input = (field, label, placeholder) => {
    const element = document.createElement("input");
    element.type = "text";
    element.className = field === "command" ? "field mono" : "field";
    element.dataset.field = field;
    element.value = step[field] || "";
    element.placeholder = placeholder;
    element.spellcheck = false;
    element.autocomplete = "off";
    element.setAttribute("aria-label", `Step ${index + 1} ${label}`);
    return element;
  };
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "text-button danger";
  remove.textContent = "Remove";
  remove.disabled = count < 2;
  remove.setAttribute("aria-label", `Remove step ${index + 1}`);
  remove.addEventListener("click", () => {
    const steps = readSteps();
    steps.splice(index, 1);
    renderSteps(steps, Math.min(index, steps.length - 1));
  });
  row.append(
    number,
    input("title", "title", "Title"),
    input("cwd", "folder", "Project folder"),
    input("command", "command", "Command"),
    remove,
  );
  return row;
}

function readSteps() {
  return [...byId("project-action-steps").children].map((row) => ({
    title: row.querySelector('[data-field="title"]').value,
    command: row.querySelector('[data-field="command"]').value,
    cwd: row.querySelector('[data-field="cwd"]').value,
  }));
}

/** The action as the backend stores it: the fields of its kind only. The
 * backend trims and checks them. */
function readActionForm() {
  const kind = byId("project-action-kind").value;
  const action = { id: byId("project-action-id").value, title: byId("project-action-title").value, kind };
  if (kind === "open") {
    const bundleId = selectedAppId();
    Object.assign(action, { bundleId, appPath: appChoices.get(bundleId)?.appPath || "" });
  } else if (kind === "url") {
    action.url = byId("project-action-url").value;
  } else if (kind === "terminal") {
    Object.assign(action, {
      command: byId("project-action-command").value,
      terminal: byId("project-action-terminal").value,
      cwd: byId("project-action-cwd").value,
    });
  } else if (kind === "task") {
    Object.assign(action, { steps: readSteps(), continueOnError: byId("project-action-continue").checked });
  } else if (kind === "script") {
    Object.assign(action, {
      scripts: byId("project-action-scripts")
        .value.split(",")
        .map((name) => name.trim())
        .filter(Boolean),
      mode: byId("project-action-mode").value,
    });
  }
  return action;
}

async function saveAction() {
  if (!projectActions) return;
  const action = readActionForm();
  const known = new Set(projectActions.config.actions.map((item) => item.id));
  try {
    const config = await invoke("save_project_action", { action });
    applyProjectConfig(config);
    // A new action comes back under the id the backend gave it. The fields
    // stay as typed, so focus stays where it was.
    const saved = config.actions.find((item) => (action.id ? item.id === action.id : !known.has(item.id)));
    if (saved) {
      byId("project-action-id").value = saved.id;
      byId("delete-project-action").hidden = false;
      renderActionList();
    }
    showSaved("project-actions-saved");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function addAction(select) {
  const value = select.value;
  // The pop-up is a list of things to add, so it always reads "Add Action…".
  select.value = "";
  if (!projectActions || !value) return;
  if (value.startsWith("new:")) {
    editAction({ id: "", title: "", kind: value.slice(4), mode: projectActions.config.scriptMode }, { focus: false });
    // The pop-up hands focus back to itself as it closes; take it after.
    setTimeout(() => byId("project-action-title").focus());
    return;
  }
  const preset = projectActions.catalog.presets.find((item) => `preset:${item.id}` === value);
  if (!preset) return;
  try {
    const config = await invoke("save_project_action", { action: preset });
    applyProjectConfig(config);
    const added = config.actions.find((item) => item.id === preset.id);
    if (added) {
      editAction(added, { focus: false });
      byId("project-action-list").querySelector('[aria-current="true"]')?.scrollIntoView({ block: "nearest" });
    }
    showSaved("project-actions-saved", "Added");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function deleteAction() {
  const id = byId("project-action-id").value;
  const action = projectActions?.config.actions.find((item) => item.id === id);
  if (!action) return;
  const confirmed = await confirmAction(
    `Delete “${action.title}”?`,
    "A key bound to it shows the action panel instead.",
    "Delete",
  );
  if (!confirmed) return;
  try {
    applyProjectConfig(await invoke("delete_project_action", { id }));
    closeActionEditor();
    showSaved("project-actions-saved", "Deleted");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function moveAction(offset) {
  if (!projectActions) return;
  const ids = projectActions.config.actions.map((action) => action.id);
  const from = ids.indexOf(byId("project-action-id").value);
  const to = from + offset;
  if (from < 0 || to < 0 || to >= ids.length) return;
  ids.splice(to, 0, ...ids.splice(from, 1));
  try {
    applyProjectConfig(await invoke("reorder_project_actions", { ids }));
    byId("project-action-list").querySelector('[aria-current="true"]')?.scrollIntoView({ block: "nearest" });
    showSaved("project-actions-saved");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function restoreActions() {
  if (!projectActions) return;
  const confirmed = await confirmAction(
    "Restore default actions?",
    "Your actions and keys go back to what Samlu found installed, and the Running settings to their defaults.",
    "Restore",
  );
  if (!confirmed) return;
  try {
    applyProjectConfig(await invoke("reset_project_actions"));
    closeActionEditor();
    showSaved("project-actions-saved", "Restored");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function saveProjectPreferences() {
  if (!projectActions) return;
  try {
    applyProjectConfig(
      await invoke("set_project_action_preferences", {
        terminal: byId("project-terminal").value,
        scriptMode: byId("project-script-mode").value,
        notifyTaskFinished: byId("project-notify-finished").checked,
        notifyTaskUrl: byId("project-notify-url").checked,
      }),
    );
    showSaved("project-running-saved");
  } catch (error) {
    toast(errorMessage(error), true);
    renderProjectRunning();
  }
}

async function loadProjectFolders() {
  try {
    projectSettings = await invoke("get_project_settings");
    renderProjectSettings();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderProjectSettings() {
  const count = projectSettings.projectCount;
  byId("project-count").textContent = `${count} ${count === 1 ? "project" : "projects"}`;
  renderPathList("project-root-list", projectSettings.roots, "No folders yet.", (path) => removeProjectPath("root", path));
  renderPathList("project-exclusion-list", projectSettings.excludedPaths, "None.", (path) =>
    removeProjectPath("exclusion", path),
  );
}

async function saveProjectSettings(next) {
  const projectCount = await invoke("set_project_settings", {
    roots: next.roots,
    excludedPaths: next.excludedPaths,
  });
  projectSettings = { ...next, projectCount };
  renderProjectSettings();
}

async function removeProjectPath(kind, path) {
  try {
    await saveProjectSettings({
      roots: kind === "root" ? projectSettings.roots.filter((value) => value !== path) : [...projectSettings.roots],
      excludedPaths:
        kind === "exclusion"
          ? projectSettings.excludedPaths.filter((value) => value !== path)
          : [...projectSettings.excludedPaths],
    });
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function addProjectPath(kind, button) {
  try {
    const path = await runButton(button, "Choosing…", () =>
      invoke("choose_project_folder", {
        prompt: kind === "root" ? "Choose a folder of projects to index" : "Choose a folder to exclude",
      }),
    );
    if (!path) return;
    const key = kind === "root" ? "roots" : "excludedPaths";
    if (projectSettings[key].includes(path)) {
      toast(kind === "root" ? "That folder is already indexed." : "That folder is already excluded.");
      return;
    }
    const next = { roots: [...projectSettings.roots], excludedPaths: [...projectSettings.excludedPaths] };
    next[key].push(path);
    await saveProjectSettings(next);
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

/* ------------------------------------------------------------------ events */

function bindEvents() {
  const navItems = [...document.querySelectorAll(".nav-item")];
  navItems.forEach((item, index) => {
    item.addEventListener("click", () => selectPane(item.dataset.pane));
    // Arrow keys move through the sidebar like a native source list.
    item.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      event.preventDefault();
      const next = navItems[(index + (event.key === "ArrowDown" ? 1 : navItems.length - 1)) % navItems.length];
      next.focus();
      selectPane(next.dataset.pane);
    });
  });

  const scroller = byId("scroller");
  scroller.addEventListener(
    "scroll",
    () => {
      byId("pane-title").parentElement.dataset.scrolled = String(scroller.scrollTop > 2);
    },
    { passive: true },
  );

  // General
  byId("appearance").addEventListener("change", async (event) => {
    const appearance = event.currentTarget.value;
    applyAppearance(appearance);
    try {
      await invoke("set_appearance", { appearance });
    } catch (error) {
      toast(errorMessage(error), true);
      await loadAppearance();
    }
  });
  byId("keep-running").addEventListener("change", () => queuePrefsSave());
  byId("update-action").addEventListener("click", updateAction);
  byId("about-check-updates").addEventListener("click", () => {
    if (updateState?.status === "ready") {
      invoke("restart_to_update").catch((error) => toast(errorMessage(error), true));
      return;
    }
    selectPane("general");
    updateAction();
  });
  byId("update-auto-check").addEventListener("change", (event) =>
    saveUpdatePreferences({
      autoCheck: event.currentTarget.checked,
      autoInstall: Boolean(updateState?.autoInstall),
    }),
  );
  byId("update-auto-install").addEventListener("change", (event) =>
    saveUpdatePreferences({ autoCheck: true, autoInstall: event.currentTarget.checked }),
  );
  byId("open-setup-guide").addEventListener("click", () =>
    invoke("onboarding_show").catch((error) => toast(errorMessage(error), true)),
  );
  document.querySelectorAll("[data-link]").forEach((link) => {
    link.addEventListener("click", () =>
      invoke("open_samlu_link", { link: link.dataset.link }).catch((error) =>
        toast(errorMessage(error), true),
      ),
    );
  });

  // Agents
  [
    "notify-needs-input",
    "notify-task-completed",
    "notify-turn-finished",
    "include-agent-summary",
    "agent-delivery",
    "island-style",
    "notification-debounce",
  ].forEach((id) => byId(id).addEventListener("change", () => queuePrefsSave("notifications-saved")));
  ["agent-delivery", "island-style"].forEach((id) => byId(id).addEventListener("change", syncIslandStyleRow));
  byId("test-agent-notification").addEventListener("click", async (event) => {
    try {
      await runButton(event.currentTarget, "Sending…", () => invoke("test_agent_notification"));
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("clear-activity").addEventListener("click", async () => {
    const confirmed = await confirmAction(
      "Clear agent activity?",
      T("Samlu’s record of recent agent events is deleted from this Mac."),
      "Clear",
    );
    if (!confirmed) return;
    try {
      await invoke("clear_event_history");
      renderActivity([]);
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("close-agent-preview").addEventListener("click", () => {
    byId("agent-preview").hidden = true;
  });
  byId("agent-integrations").addEventListener("click", async (event) => {
    const button = event.target.closest("button[data-integration-action]");
    if (!button) return;
    const id = button.dataset.integrationId;
    const action = button.dataset.integrationAction;
    const name = agentIntegrations.find((item) => item.id === id)?.name || "This agent";
    try {
      if (action === "preview") {
        showAgentPreview(`${name} configuration`, await invoke("preview_agent_integration", { id }));
        return;
      }
      if (action === "remove") {
        const confirmed = await confirmAction(
          `Disconnect ${name}?`,
          "Samlu removes only its own hooks. The rest of the configuration stays as it is.",
          "Disconnect",
        );
        if (!confirmed) return;
        await runButton(button, "Disconnecting…", () => invoke("remove_agent_integration", { id }));
        toast(`${name} disconnected.`);
      } else {
        await runButton(button, "Connecting…", () => invoke("apply_agent_integration", { id }));
        toast(`${name} connected.`);
      }
      await loadAgents();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

  // Voice
  listen("voice://model-download", (event) => {
    downloadProgress[event.payload.id] = event.payload;
    if (!byId("voice-whisper-fields").hidden) renderVoiceModels();
  });
  byId("voice-stt-choice").addEventListener("change", () =>
    saveSpeechChoice().catch((error) => toast(errorMessage(error), true)),
  );
  ["voice-stt-base-url", "voice-stt-model"].forEach((id) => {
    byId(id).addEventListener("change", () =>
      saveSpeechChoice().catch((error) => toast(errorMessage(error), true)),
    );
  });
  byId("voice-language").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_language", { language: event.currentTarget.value });
      showSaved("voice-engine-saved");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("save-voice-stt-key").addEventListener("click", async (event) => {
    const input = byId("voice-stt-api-key");
    if (!input.value.trim()) {
      toast("Paste an API key first.", true);
      input.focus();
      return;
    }
    if (byId("voice-stt-choice").value !== voiceSettings?.stt.choice) {
      await saveSpeechChoice().catch((error) => toast(errorMessage(error), true));
      if (byId("voice-stt-choice").value !== voiceSettings?.stt.choice) {
        toast("Enter the provider URL before saving a key.", true);
        return;
      }
    }
    try {
      await runButton(event.currentTarget, "Saving…", () =>
        invoke("set_voice_api_key", { role: "transcription", key: input.value.trim() }),
      );
      input.value = "";
      showSaved("voice-engine-saved", T("Saved to Keychain"));
      await loadVoiceSettings();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("voice-add-model").addEventListener("click", async () => {
    try {
      const path = await invoke("voice_add_model");
      if (path) await loadVoiceModels();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("voice-rescan-models").addEventListener("click", loadVoiceModels);
  byId("voice-apple-install").addEventListener("click", async (event) => {
    try {
      const locale = await runButton(event.currentTarget, "Installing…", () =>
        invoke("voice_apple_install", { language: byId("voice-language").value }),
      );
      byId("voice-apple-help").textContent = `Ready for ${locale}.`;
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("voice-cleanup").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_cleanup", { enabled: event.currentTarget.checked });
      showSaved("voice-text-saved");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("open-voice-microphone").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    try {
      const status = await invoke("get_voice_microphone_status");
      if (status === "not_determined") {
        await runButton(button, "Asking…", async () => {
          await invoke("request_voice_microphone");
          await refreshMicrophonePermission(20);
        });
        const updated = await refreshMicrophonePermission();
        if (updated === "denied") {
          toast(T("Microphone access is off. Opening System Settings."), true);
          await invoke("open_voice_microphone_settings");
        } else if (updated === "not_determined") {
          toast("Waiting for the macOS microphone prompt.");
        }
        return;
      }
      await invoke("open_voice_microphone_settings");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("open-voice-accessibility").addEventListener("click", async () => {
    try {
      const trusted = await invoke("request_voice_accessibility");
      if (!trusted) await new Promise((resolve) => setTimeout(resolve, 700));
      await invoke("open_voice_accessibility_settings");
      await refreshPermissions();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

  const saveTransformation = async () => {
    const provider = byId("voice-transform-provider").value;
    const baseUrl = byId("voice-transform-base-url").value.trim();
    byId("voice-transform-base-url-row").hidden = provider !== "custom";
    if (provider === "custom" && !baseUrl) {
      byId("voice-transform-base-url").focus();
      return;
    }
    await invoke("set_voice_transformation", { provider, baseUrl });
    showSaved("voice-text-saved");
    await loadVoiceSettings();
  };
  ["voice-transform-provider", "voice-transform-base-url"].forEach((id) => {
    byId(id).addEventListener("change", () =>
      saveTransformation().catch((error) => toast(errorMessage(error), true)),
    );
  });
  byId("voice-transform-model").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_transform_model", { model: event.currentTarget.value });
      showSaved("voice-text-saved");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("save-voice-transform-key").addEventListener("click", async (event) => {
    const input = byId("voice-transform-api-key");
    if (!input.value.trim()) {
      toast("Paste an API key first.", true);
      input.focus();
      return;
    }
    try {
      await runButton(event.currentTarget, "Saving…", () =>
        invoke("set_voice_api_key", { role: "transformation", key: input.value.trim() }),
      );
      input.value = "";
      showSaved("voice-text-saved", T("Saved to Keychain"));
      await loadVoiceSettings();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("voice-vocabulary").addEventListener("change", async (event) => {
    try {
      const terms = await invoke("set_voice_vocabulary", { text: event.currentTarget.value });
      event.currentTarget.value = terms.join("\n");
      showSaved("voice-vocabulary-saved", `${terms.length} ${terms.length === 1 ? "term" : "terms"} saved`);
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  voiceHotkeyRecorder = createShortcutRecorder(
    "voice-hotkey",
    async (hotkey) => {
      await invoke("set_voice_hotkey", { hotkey });
      updateVoiceShortcutGuide(hotkey);
      showSaved("voice-modes-saved");
    },
    loadVoiceSettings,
    // ⇧ and ⌘ are added on top of the base chord for Summary and Prompt.
    (hotkey) =>
      /(^|\+)(Shift|Cmd)(\+|$)/.test(hotkey) ? (PLATFORM.windows ? "Use Ctrl or Alt only" : "Use ⌃ or ⌥ only") : "",
  );
  [
    ["dictation", "voice-dictation-delivery"],
    ["summary", "voice-summary-delivery"],
    ["prompt", "voice-prompt-delivery"],
  ].forEach(([mode, id]) => {
    byId(id).addEventListener("change", async (event) => {
      try {
        await invoke("set_voice_delivery", { mode, delivery: event.currentTarget.value });
        showSaved("voice-modes-saved");
      } catch (error) {
        toast(errorMessage(error), true);
        await loadVoiceSettings();
      }
    });
  });
  byId("voice-interface-sounds").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_interface_sounds", { enabled: event.currentTarget.checked });
      showSaved("voice-modes-saved");
    } catch (error) {
      toast(errorMessage(error), true);
      await loadVoiceSettings();
    }
  });
  byId("voice-history-search").addEventListener("input", loadVoiceHistory);
  byId("voice-history-raw").addEventListener("change", renderVoiceHistory);
  byId("voice-keep-history").addEventListener("change", async (event) => {
    const enabled = event.currentTarget.checked;
    const toggle = event.currentTarget;
    if (!enabled && voiceHistory.length) {
      const confirmed = await confirmAction(
        "Turn off transcript history?",
        T("Saved transcripts are deleted from this Mac."),
        "Turn Off",
      );
      if (!confirmed) {
        toggle.checked = true;
        return;
      }
    }
    try {
      await invoke("set_voice_keep_history", { enabled });
      showSaved("voice-history-saved");
      await loadVoiceHistory();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("clear-voice-history").addEventListener("click", async () => {
    const confirmed = await confirmAction(
      "Delete all transcripts?",
      T("Your saved dictation history is removed from this Mac."),
      "Delete",
    );
    if (!confirmed) return;
    try {
      await invoke("clear_voice_history");
      showSaved("voice-history-saved", "Cleared");
      await loadVoiceHistory();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

  // Launcher
  launcherHotkeyRecorder = createShortcutRecorder(
    "launcher-hotkey",
    async (hotkey) => {
      await invoke("set_launcher_hotkey", { hotkey });
      showSaved("launcher-saved");
    },
    async () => launcherHotkeyRecorder.set(await invoke("get_launcher_hotkey")),
  );
  byId("open-launcher").addEventListener("click", () =>
    invoke("launcher_toggle").catch((error) => toast(errorMessage(error), true)),
  );
  byId("clipboard-history-enabled").addEventListener("change", () => queuePrefsSave());
  byId("clear-clipboard").addEventListener("click", async () => {
    const confirmed = await confirmAction(
      "Clear clipboard history?",
      T("Every saved clip is deleted from this Mac."),
      "Clear",
    );
    if (!confirmed) return;
    try {
      await invoke("clear_clipboard_history");
      toast("Clipboard history cleared.");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("deep-find-enabled").addEventListener("change", async (event) => {
    try {
      deepFind = await invoke("set_deep_find_enabled", { enabled: event.currentTarget.checked });
      renderDeepFind();
      // Turning it on offers the meaning model straight away.
      const chosen = deepFind.model;
      if (
        deepFind.enabled &&
        chosen !== "none" &&
        !deepFind.status[`${chosen}Downloaded`] &&
        !deepFind.status.downloading
      ) {
        downloadDeepFindModel(chosen).catch((error) => toast(errorMessage(error), true));
      }
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("deep-find-code").addEventListener("change", (event) =>
    saveDeepFindFolders({ includeCode: event.currentTarget.checked }),
  );
  byId("deep-find-add-root").addEventListener("click", (event) => addDeepFindFolder("root", event.currentTarget));
  byId("deep-find-add-exclusion").addEventListener("click", (event) =>
    addDeepFindFolder("exclusion", event.currentTarget),
  );
  byId("deep-find-rebuild").addEventListener("click", async () => {
    const confirmed = await confirmAction(
      "Rebuild the index?",
      "Every file in the searched folders is read again. Searching keeps working meanwhile, with fewer results until it finishes.",
      "Rebuild",
    );
    if (!confirmed) return;
    invoke("deep_find_rebuild").catch((error) => toast(errorMessage(error), true));
  });
  byId("deep-find-remove").addEventListener("click", async () => {
    const confirmed = await confirmAction(
      "Remove Descriptive Find’s data?",
      "This turns Descriptive Find off and deletes its index and meaning model from this computer. Your files are not touched.",
      "Remove",
    );
    if (!confirmed) return;
    try {
      deepFind = await invoke("deep_find_remove_data");
      renderDeepFind();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  listen("deep-find://status", (event) => {
    if (!deepFind) return;
    deepFind.status = event.payload;
    if (currentPane === "launcher") renderDeepFind();
  });
  listen("deep-find://download", (event) => {
    deepFindDownload = event.payload;
    if (deepFind && currentPane === "launcher") {
      deepFind.status.downloading = event.payload.model;
      renderDeepFind();
    }
  });
  byId("new-snippet").addEventListener("click", () => editSnippet());
  byId("snippet-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      const saved = await invoke("save_snippet", {
        snippet: {
          id: byId("snippet-id").value,
          title: byId("snippet-title").value,
          keyword: byId("snippet-keyword").value,
          content: byId("snippet-content").value,
        },
      });
      const index = snippets.findIndex((item) => item.id === saved.id);
      if (index >= 0) snippets[index] = saved;
      else snippets.push(saved);
      editSnippet(saved);
      toast("Snippet saved.");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("delete-snippet").addEventListener("click", async () => {
    const id = byId("snippet-id").value;
    if (!id) return;
    const confirmed = await confirmAction("Delete this snippet?", "It is removed from the launcher.", "Delete");
    if (!confirmed) return;
    try {
      await invoke("delete_snippet", { id });
      snippets = snippets.filter((item) => item.id !== id);
      byId("snippet-form").hidden = true;
      byId("snippet-empty").hidden = false;
      byId("snippet-id").value = "";
      renderSnippets("");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

  // Projects
  document.querySelectorAll("select[data-chord]").forEach((select) => {
    select.addEventListener("change", () => bindProjectKey(select));
  });
  byId("add-project-action").addEventListener("change", (event) => addAction(event.currentTarget));
  byId("restore-project-actions").addEventListener("click", restoreActions);
  byId("move-project-action-up").addEventListener("click", () => moveAction(-1));
  byId("move-project-action-down").addEventListener("click", () => moveAction(1));
  byId("project-action-kind").addEventListener("change", syncActionKind);
  byId("project-action-app").addEventListener("change", (event) => changeActionApp(event.currentTarget));
  byId("add-project-step").addEventListener("click", () => {
    const steps = readSteps();
    steps.push({ title: "", command: "", cwd: "" });
    renderSteps(steps, steps.length - 1);
  });
  byId("project-action-form").addEventListener("submit", (event) => {
    event.preventDefault();
    saveAction();
  });
  byId("delete-project-action").addEventListener("click", deleteAction);
  ["project-terminal", "project-script-mode", "project-notify-finished", "project-notify-url"].forEach((id) =>
    byId(id).addEventListener("change", saveProjectPreferences),
  );
  byId("add-project-root").addEventListener("click", (event) => addProjectPath("root", event.currentTarget));
  byId("add-project-exclusion").addEventListener("click", (event) =>
    addProjectPath("exclusion", event.currentTarget),
  );
  byId("reindex-projects").addEventListener("click", async (event) => {
    try {
      const projectCount = await runButton(event.currentTarget, "Indexing…", () => invoke("reindex_projects"));
      projectSettings.projectCount = projectCount;
      renderProjectSettings();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
}

/* -------------------------------------------------------------------- boot */

async function boot() {
  bindEvents();
  selectPane("general");

  window.addEventListener("focus", () => {
    if (currentPane === "voice") refreshPermissions();
    if (currentPane === "general" && updateState?.status !== "checking") loadUpdateState();
  });
  await listen("updater://state", (event) => renderUpdate(event.payload));
  await listen("agent://event", () => {
    if (currentPane === "agents") loadActivity();
  });
  await listen("appearance://changed", (event) => {
    applyAppearance(event.payload);
    byId("appearance").value = event.payload;
  });
  await listen("settings://tab", (event) => {
    const tab = typeof event.payload === "string" ? event.payload : event.payload?.tab;
    // "overview" is the pane's name in older builds of the backend.
    selectPane(tab === "overview" ? "general" : tab);
  });
}

boot().catch((error) => toast(errorMessage(error), true));
