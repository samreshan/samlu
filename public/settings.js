const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const byId = (id) => document.getElementById(id);
let notificationPreferences = null;
let snippets = [];
let projectSettings = { roots: [], excludedPaths: [], projectCount: 0 };
let agentIntegrations = [];
let toastTimer = null;
let saveTimer = null;
const saveStateTimers = {};
let voiceHotkeyRecorder = null;
let launcherHotkeyRecorder = null;

function applyAppearance(appearance) {
  const value = ["light", "dark"].includes(appearance) ? appearance : "";
  if (value) document.documentElement.dataset.theme = value;
  else delete document.documentElement.dataset.theme;
}

async function loadAppearance() {
  const appearance = await invoke("get_appearance");
  byId("appearance").value = appearance;
  applyAppearance(appearance);
}

function toast(message, error = false) {
  const element = byId("toast");
  element.textContent = String(message);
  element.classList.toggle("error", error);
  element.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => element.classList.remove("show"), 2800);
}

// Routine saves confirm inline, next to the control that changed. The toast is
// kept for errors and for results that land somewhere the user is not looking,
// so a toast always means "read me".
function setSaveState(id, label) {
  const state = byId(id);
  clearTimeout(saveStateTimers[id]);
  state.textContent = label;
  state.classList.toggle("show", Boolean(label));
}

function showSaveState(id, label = "Saved") {
  setSaveState(id, label);
  saveStateTimers[id] = setTimeout(() => {
    byId(id).classList.remove("show");
    // Text is cleared only after the fade so the live region does not announce
    // an empty update mid-transition.
    saveStateTimers[id] = setTimeout(() => setSaveState(id, ""), 240);
  }, 2000);
}

function errorMessage(error) {
  if (typeof error === "string") return error;
  return error?.message || "Something went wrong.";
}

function setDot(id, state) {
  byId(id).className = `status-dot ${state || ""}`.trim();
}

function selectTab(name) {
  document.querySelectorAll(".nav-item").forEach((button) => {
    const active = button.dataset.tab === name;
    button.classList.toggle("active", active);
    if (active) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  document.querySelectorAll(".tab-view").forEach((view) => {
    const active = view.id === `tab-${name}`;
    view.classList.toggle("active", active);
    view.hidden = !active;
  });
  if (name === "overview") loadOverview();
  if (name === "agents") loadAgentStatus();
  if (name === "voice") loadVoiceSettings();
  if (name === "launcher") loadLauncherSettings();
}

async function loadAgentStatus() {
  try {
    agentIntegrations = await invoke("get_agent_integrations");
    renderAgentIntegrations();
    ["claude-code", "codex"].forEach((id) => {
      const integration = agentIntegrations.find((item) => item.id === id);
      const prefix = id === "claude-code" ? "claude" : "codex";
      const connected = Boolean(integration?.installed);
      setDot(`${prefix}-dot`, connected ? "good" : "warn");
      byId(`${prefix}-detail`).textContent = integration?.error
        ? "Status unavailable"
        : connected
          ? "Connected"
          : "Setup needed";
    });
  } catch (error) {
    agentIntegrations = [];
    byId("agent-integrations").replaceChildren();
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = `Agent status unavailable: ${errorMessage(error)}`;
    byId("agent-integrations").append(empty);
    ["claude", "codex"].forEach((prefix) => {
      setDot(`${prefix}-dot`, "warn");
      byId(`${prefix}-detail`).textContent = "Status unavailable";
    });
  }
}

function renderAgentIntegrations() {
  const container = byId("agent-integrations");
  container.replaceChildren();
  agentIntegrations.forEach((integration) => {
    const section = document.createElement("section");
    section.className = "integration";
    const head = document.createElement("div");
    head.className = "integration-head";
    const logo = document.createElement("div");
    logo.className = "integration-logo";
    logo.setAttribute("aria-hidden", "true");
    logo.textContent = integration.name.slice(0, 1);
    const copy = document.createElement("div");
    const title = document.createElement("h2");
    title.textContent = integration.name;
    const path = document.createElement("p");
    path.textContent = integration.path;
    path.title = integration.path;
    copy.append(title, path);
    const pill = document.createElement("span");
    pill.className = `status-pill ${integration.installed ? "good" : "warn"}`;
    pill.textContent = integration.installed ? "Connected" : "Setup needed";
    head.append(logo, copy, pill);

    const description = document.createElement("p");
    description.textContent = integration.error || integration.description;
    const actions = document.createElement("div");
    actions.className = "button-row";
    const install = document.createElement("button");
    install.type = "button";
    install.className = "primary-button";
    install.dataset.integrationAction = "apply";
    install.dataset.integrationId = integration.id;
    install.textContent = integration.installed ? "Repair connection" : "Install integration";
    const preview = document.createElement("button");
    preview.type = "button";
    preview.className = "secondary-button";
    preview.dataset.integrationAction = "preview";
    preview.dataset.integrationId = integration.id;
    preview.textContent = "Preview changes";
    actions.append(install, preview);
    if (integration.installed) {
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "secondary-button danger-text";
      remove.dataset.integrationAction = "remove";
      remove.dataset.integrationId = integration.id;
      remove.textContent = "Disconnect";
      actions.append(remove);
    }
    section.append(head, description, actions);
    container.append(section);
  });
}

async function loadNotificationPreferences() {
  notificationPreferences = await invoke("get_notification_preferences");
  byId("notify-needs-input").checked = notificationPreferences.notifyNeedsInput;
  byId("notify-task-completed").checked = notificationPreferences.notifyTaskCompleted;
  byId("notify-turn-finished").checked = notificationPreferences.notifyTurnFinished;
  byId("include-agent-summary").checked = notificationPreferences.includeAgentSummary;
  byId("agent-delivery").value = notificationPreferences.delivery || "island";
  byId("close-behavior").value = notificationPreferences.closeBehavior || "background";
  byId("notification-debounce").value = notificationPreferences.debounceSecs;
  byId("clipboard-history-enabled").checked = notificationPreferences.clipboardHistoryEnabled;
}

async function saveNotificationPreferences(stateId) {
  if (!notificationPreferences) return;
  notificationPreferences = {
    debounceSecs: Math.max(3, Math.min(300, Number(byId("notification-debounce").value) || 20)),
    notifyNeedsInput: byId("notify-needs-input").checked,
    notifyTaskCompleted: byId("notify-task-completed").checked,
    notifyTurnFinished: byId("notify-turn-finished").checked,
    includeAgentSummary: byId("include-agent-summary").checked,
    delivery: byId("agent-delivery").value,
    clipboardHistoryEnabled: byId("clipboard-history-enabled").checked,
    closeBehavior: byId("close-behavior").value,
  };
  setSaveState(stateId, "Saving");
  try {
    await invoke("set_notification_preferences", { preferences: notificationPreferences });
    showSaveState(stateId);
  } catch (error) {
    setSaveState(stateId, "");
    toast(errorMessage(error), true);
  }
}

// Clipboard history is one of these preferences but lives on the Launcher tab,
// so the caller says which heading should show the confirmation.
function queuePreferenceSave(stateId = "notification-save-state") {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => saveNotificationPreferences(stateId), 220);
}

function renderActivity(items) {
  const list = byId("activity-list");
  list.replaceChildren();
  if (!items?.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No agent events received yet.";
    list.append(empty);
    return;
  }
  items.slice(0, 18).forEach((item) => {
    const row = document.createElement("div");
    row.className = "activity-item";
    const dot = document.createElement("span");
    dot.className = `status-dot ${item.kind_label === "needs your input" ? "warn" : "good"}`;
    const copy = document.createElement("div");
    copy.className = "activity-copy";
    const title = document.createElement("strong");
    title.textContent = `${item.agent_label} ${item.kind_label}`;
    const detail = document.createElement("span");
    const fullDetail = [item.project_label, item.summary].filter(Boolean).join(" · ");
    detail.textContent =
      fullDetail.length > 220 ? `${fullDetail.slice(0, 217).trimEnd()}…` : fullDetail;
    if (fullDetail.length > 220) detail.title = fullDetail;
    const time = document.createElement("time");
    time.className = "activity-time";
    time.dateTime = item.timestamp;
    time.textContent = new Intl.DateTimeFormat(undefined, {
      hour: "numeric",
      minute: "2-digit",
      month: "short",
      day: "numeric",
    }).format(new Date(item.timestamp));
    copy.append(title, detail);
    row.append(dot, copy, time);
    list.append(row);
  });
}

async function loadOverview() {
  const results = await Promise.allSettled([
    invoke("get_app_status"),
    invoke("get_event_history"),
    loadNotificationPreferences(),
    loadAgentStatus(),
    loadAppearance(),
  ]);
  const appStatus = results[0];
  if (appStatus.status === "fulfilled" && appStatus.value.serverRunning) {
    setDot("listener-dot", "good");
    setDot("sidebar-status-dot", "good");
    byId("listener-detail").textContent = `Listening on ${appStatus.value.port}`;
    byId("sidebar-status").textContent = "Listener active";
  } else {
    setDot("listener-dot", "warn");
    setDot("sidebar-status-dot", "warn");
    byId("listener-detail").textContent = "Not running";
    byId("sidebar-status").textContent = "Listener unavailable";
  }
  if (results[1].status === "fulfilled") renderActivity(results[1].value);
}

function showAgentPreview(title, code) {
  byId("agent-preview-title").textContent = title;
  byId("agent-preview-code").textContent = code;
  byId("agent-preview").hidden = false;
  byId("agent-preview").scrollIntoView({ behavior: "smooth", block: "nearest" });
}

async function runButton(button, busyLabel, operation) {
  const original = button.textContent;
  button.disabled = true;
  button.textContent = busyLabel;
  try {
    return await operation();
  } finally {
    button.disabled = false;
    button.textContent = original;
  }
}

let voiceHistory = [];

function renderVoiceHistory() {
  const list = byId("voice-history-list");
  const showRaw = byId("voice-history-raw").checked;
  list.replaceChildren();
  if (!voiceHistory.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = byId("voice-keep-history").checked
      ? "No dictations yet."
      : "History is off.";
    list.append(empty);
    return;
  }
  voiceHistory.forEach((entry) => {
    const row = document.createElement("div");
    row.className = "history-row";
    const body = document.createElement("div");
    const text = document.createElement("p");
    text.textContent = showRaw && entry.raw ? entry.raw : entry.text;
    const meta = document.createElement("small");
    const when = new Date(entry.timestamp).toLocaleString();
    const outcome = entry.outcome.replace(/_/g, " ");
    meta.textContent = `${when} · ${entry.mode} · ${entry.engine} · ${outcome}${entry.targetApp ? ` · ${entry.targetApp}` : ""}`;
    body.append(text, meta);
    const copy = document.createElement("button");
    copy.className = "secondary-button";
    copy.type = "button";
    copy.textContent = "Copy";
    copy.addEventListener("click", async () => {
      await navigator.clipboard.writeText(text.textContent);
      showSaveState("voice-history-save-state", "Copied");
    });
    row.append(body, copy);
    list.append(row);
  });
}

async function loadVoiceHistory() {
  try {
    voiceHistory = await invoke("get_voice_history", {
      query: byId("voice-history-search").value,
    });
    renderVoiceHistory();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

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
    byId("voice-dictation-delivery").value = settings.delivery?.dictation || "instant_insert";
    byId("voice-summary-delivery").value = settings.delivery?.summary || "instant_insert";
    byId("voice-prompt-delivery").value = settings.delivery?.prompt || "editable_preview";
    byId("voice-interface-sounds").checked = settings.interfaceSounds !== false;
    byId("voice-pet-capsule").checked = settings.petCapsule !== false;
    byId("voice-keep-history").checked = settings.keepHistory !== false;
    await loadVoiceHistory();
    renderMicrophonePermission(microphoneStatus);
    const accessibility = byId("voice-accessibility-status");
    accessibility.className = `status-pill ${accessibilityTrusted ? "good" : "warn"}`;
    accessibility.textContent = accessibilityTrusted ? "Ready" : "Permission needed";
    byId("voice-accessibility-help").textContent = accessibilityTrusted
      ? "Ready to return text to the field where recording began."
      : "Not active for this build. If Samlu is already enabled in System Settings, remove that old entry and add the running Samlu again.";
    byId("open-voice-accessibility").textContent = accessibilityTrusted
      ? "Open Settings"
      : "Enable access";
    updateVoiceShortcutGuide(settings.hotkey || "Alt+V");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderMicrophonePermission(status) {
  const microphone = byId("voice-microphone-status");
  const help = byId("voice-microphone-help");
  const button = byId("open-voice-microphone");
  const authorized = status === "authorized";
  microphone.className = `status-pill ${authorized ? "good" : "warn"}`;

  if (authorized) {
    microphone.textContent = "Ready";
    help.textContent = "Ready to record dictation in every app and Space.";
    button.textContent = "Open Settings";
    return;
  }
  if (status === "not_determined") {
    microphone.textContent = "Not requested";
    help.textContent = "Grant access once so Samlu can record dictation.";
    button.textContent = "Grant permission";
    return;
  }
  if (status === "restricted") {
    microphone.textContent = "Restricted";
    help.textContent = "Microphone access is restricted by this Mac’s privacy policy.";
    button.textContent = "Open Settings";
    return;
  }

  microphone.textContent = "Permission needed";
  help.textContent = "Microphone access is off. Enable Samlu in macOS Privacy & Security.";
  button.textContent = "Open Settings";
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

let voiceSettings = null;
let voiceModels = { models: [], catalog: [], recommended: "", selected: "" };
const downloadProgress = {};

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

function setKeyStatus(id, hasKey) {
  const pill = byId(id);
  pill.className = `status-pill ${hasKey ? "good" : "warn"}`;
  pill.textContent = hasKey ? "Key saved" : "Key required";
}

function renderSpeechEngine(settings) {
  const stt = settings.stt;
  const choice = stt.choice;
  byId("voice-apple-option").disabled = !settings.appleAvailable;
  byId("voice-apple-option").textContent = settings.appleAvailable
    ? "Apple (on-device)"
    : "Apple (on-device) — needs macOS 26";
  byId("voice-stt-choice").value = choice;
  byId("voice-language").value = stt.language || "auto";
  const cloud = !["whisper_cpp", "apple"].includes(choice);
  byId("voice-cloud-fields").hidden = !cloud;
  byId("voice-whisper-fields").hidden = choice !== "whisper_cpp";
  byId("voice-apple-fields").hidden = choice !== "apple";
  byId("voice-stt-base-url-row").hidden = choice !== "custom";
  byId("voice-stt-base-url").value = stt.baseUrl || "";
  if (cloud) {
    byId("voice-stt-model").value = stt.model || "";
    setKeyStatus("voice-stt-key-status", stt.hasApiKey);
  }
  const englishOnly = choice === "whisper_cpp" && /\.en[.-]/i.test(stt.model || "");
  const note = byId("voice-language-note");
  note.hidden = !englishOnly && choice !== "apple";
  note.textContent = englishOnly
    ? "This model only understands English."
    : "Apple has no automatic detection; Detect automatically uses your Mac's language.";
  if (choice === "whisper_cpp") loadVoiceModels();
}

function renderTextProcessing(transformation, cleanup) {
  byId("voice-cleanup").checked = cleanup;
  byId("voice-transform-provider").value = transformation.provider;
  byId("voice-transform-model").value = transformation.model || "";
  byId("voice-transform-base-url").value = transformation.baseUrl || "";
  byId("voice-transform-base-url-row").hidden = transformation.provider !== "custom";
  byId("voice-transform-key-row").hidden = !transformation.needsApiKey;
  setKeyStatus("voice-transform-key-status", transformation.hasApiKey);
}

function modelRow(title, detail, actions) {
  const row = document.createElement("div");
  row.className = "model-row";
  const text = document.createElement("div");
  const strong = document.createElement("strong");
  strong.textContent = title;
  const small = document.createElement("small");
  small.textContent = detail;
  text.append(strong, small);
  const buttons = document.createElement("div");
  buttons.className = "model-row-actions";
  actions.forEach(([label, handler, primary]) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = primary ? "primary-button" : "secondary-button";
    button.textContent = label;
    button.addEventListener("click", async (event) => {
      try {
        await handler(event.currentTarget);
      } catch (error) {
        toast(errorMessage(error), true);
      }
    });
    buttons.append(button);
  });
  row.append(text, buttons);
  return row;
}

function renderVoiceModels() {
  const list = byId("voice-model-list");
  list.replaceChildren();
  if (!voiceModels.models.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No whisper models yet. Download one below or add a file you already have.";
    list.append(empty);
  }
  const sourceLabel = { downloaded: "Downloaded", discovered: "Found on this Mac", added: "Added" };
  voiceModels.models.forEach((model) => {
    const selected = model.path === voiceModels.selected;
    const detail = [
      model.missing ? "Missing" : formatBytes(model.bytes),
      sourceLabel[model.source],
      model.englishOnly ? "English only" : "",
      selected ? "In use" : "",
    ].filter(Boolean).join(" · ");
    const actions = [];
    if (!selected && !model.missing) {
      actions.push(["Use", async () => {
        await invoke("set_voice_stt", { choice: "whisper_cpp", baseUrl: "", model: model.path });
        showSaveState("voice-provider-save-state");
        await loadVoiceSettings();
      }, true]);
    }
    if (model.source === "downloaded") {
      actions.push(["Delete", async () => {
        if (!confirm(`Delete ${model.name} from this Mac?`)) return;
        await invoke("voice_delete_model", { path: model.path });
        await loadVoiceModels();
      }]);
    } else if (model.source === "added") {
      actions.push(["Remove", async () => {
        await invoke("voice_remove_model", { path: model.path });
        await loadVoiceModels();
      }]);
    }
    list.append(modelRow(model.name, detail, actions));
  });

  const catalog = byId("voice-model-catalog");
  catalog.replaceChildren();
  voiceModels.catalog.forEach((entry) => {
    const progress = downloadProgress[entry.id];
    const recommended = entry.id === voiceModels.recommended ? " · Recommended for this Mac" : "";
    let detail = `${formatBytes(entry.bytes)}${recommended}`;
    let actions = [];
    if (entry.downloaded) {
      detail = `Downloaded${recommended}`;
    } else if (entry.downloading) {
      const percent = progress ? Math.floor((progress.received / progress.total) * 100) : 0;
      detail = `Downloading ${percent}% of ${formatBytes(entry.bytes)}`;
      actions = [["Cancel", () => invoke("voice_cancel_model_download", { id: entry.id })]];
    } else {
      actions = [["Download", async () => {
        const download = invoke("voice_download_model", { id: entry.id });
        await loadVoiceModels();
        try {
          const path = await download;
          // First usable model: select it so dictation works right away.
          if (!voiceModels.selected) {
            await invoke("set_voice_stt", { choice: "whisper_cpp", baseUrl: "", model: path });
          }
          showSaveState("voice-provider-save-state", "Model ready");
        } catch (error) {
          if (errorMessage(error) !== "Download cancelled.") throw error;
        } finally {
          delete downloadProgress[entry.id];
          await loadVoiceSettings();
        }
      }, entry.id === voiceModels.recommended]];
    }
    catalog.append(modelRow(entry.label, detail, actions));
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
    byId("voice-stt-base-url-row").hidden = false;
    byId("voice-stt-base-url").focus();
    return;
  }
  let model = choice === voiceSettings?.stt.choice ? byId("voice-stt-model").value : "";
  if (choice === "whisper_cpp") {
    // Keep the current whisper model, or fall back to the first usable one.
    await loadVoiceModels();
    model =
      voiceModels.selected || voiceModels.models.find((item) => !item.missing)?.path || "";
  }
  await invoke("set_voice_stt", { choice, baseUrl, model });
  showSaveState("voice-provider-save-state");
  await loadVoiceSettings();
}

function shortcutGlyphs(shortcut) {
  const glyph = {
    cmd: "⌘",
    command: "⌘",
    shift: "⇧",
    alt: "⌥",
    option: "⌥",
    ctrl: "⌃",
    control: "⌃",
  };
  return shortcut
    .split("+")
    .map((part) => glyph[part.trim().toLowerCase()] || part.trim().toUpperCase())
    .join("");
}

function addShortcutModifier(shortcut, modifier) {
  const parts = shortcut.split("+").map((part) => part.trim()).filter(Boolean);
  const key = parts.pop() || "V";
  return [...parts, modifier, key].join("+");
}

function updateVoiceShortcutGuide(hotkey) {
  byId("voice-normal-key").textContent = shortcutGlyphs(hotkey);
  byId("voice-summary-key").textContent = shortcutGlyphs(addShortcutModifier(hotkey, "Shift"));
  byId("voice-prompt-key").textContent = shortcutGlyphs(addShortcutModifier(hotkey, "Cmd"));
}

// Ctrl, Alt, Shift, Cmd, then the key: the exact spelling and order the global
// shortcut backend parses, e.g. "Alt+V".
const SHORTCUT_MODIFIERS = [
  ["ctrlKey", "Ctrl"],
  ["altKey", "Alt"],
  ["shiftKey", "Shift"],
  ["metaKey", "Cmd"],
];

function shortcutKeyName(code) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  return code.toUpperCase();
}

function shortcutFromEvent(event) {
  const modifiers = SHORTCUT_MODIFIERS.filter(([flag]) => event[flag]).map(([, name]) => name);
  if (!modifiers.length || !event.code) return "";
  return [...modifiers, shortcutKeyName(event.code)].join("+");
}

// Shortcut fields record a chord instead of accepting typed text, so the stored
// value can never drift from the format the backend registers.
function createShortcutRecorder(id, save, restore) {
  const button = byId(id);
  let value = "";
  let recording = false;

  const paint = (text, armed) => {
    button.textContent = text;
    button.classList.toggle("recording", armed);
    button.setAttribute("aria-pressed", armed ? "true" : "false");
  };

  const rest = () => {
    const wasRecording = recording;
    recording = false;
    paint(value ? shortcutGlyphs(value) : "Not set", false);
    // Hand the chords back to the system once the recorder is done with them.
    if (wasRecording) invoke("resume_global_shortcuts").catch(() => {});
  };

  button.addEventListener("click", () => {
    if (recording) return;
    recording = true;
    paint("Press shortcut", true);
    // Samlu's own bindings have to step aside, or pressing the shortcut that
    // is already assigned would fire it instead of being recorded.
    invoke("suspend_global_shortcuts").catch(() => {});
  });
  button.addEventListener("blur", () => {
    if (recording) rest();
  });
  button.addEventListener("keydown", async (event) => {
    if (!recording) return;
    // While armed every key belongs to the chord, including Tab and Space.
    event.preventDefault();
    if (event.key === "Escape") {
      rest();
      return;
    }
    if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
    const shortcut = shortcutFromEvent(event);
    if (!shortcut) {
      paint("Hold a modifier", true);
      return;
    }
    const previous = value;
    value = shortcut;
    // Show the chord straight away, but stay armed until the save lands so the
    // rebind happens before the shortcuts are handed back.
    paint(shortcutGlyphs(shortcut), false);
    try {
      await save(shortcut);
      rest();
    } catch (error) {
      value = previous;
      rest();
      toast(errorMessage(error), true);
      await restore();
    }
  });

  return {
    set(next) {
      value = next || "";
      if (!recording) rest();
    },
  };
}

async function loadLauncherSettings() {
  try {
    const [hotkey, preferences, storedSnippets, storedProjectSettings] = await Promise.all([
      invoke("get_launcher_hotkey"),
      invoke("get_notification_preferences"),
      invoke("get_snippets"),
      invoke("get_project_settings"),
    ]);
    launcherHotkeyRecorder.set(hotkey);
    notificationPreferences = preferences;
    byId("clipboard-history-enabled").checked = preferences.clipboardHistoryEnabled;
    snippets = storedSnippets;
    projectSettings = storedProjectSettings;
    renderSnippets();
    renderProjectSettings();
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderProjectSettings() {
  byId("project-count").textContent = `${projectSettings.projectCount} indexed`;
  renderPathList("project-root-list", projectSettings.roots, "No project folders configured.", "root");
  renderPathList(
    "project-exclusion-list",
    projectSettings.excludedPaths,
    "No excluded folders.",
    "exclusion",
  );
}

function renderPathList(id, paths, emptyLabel, kind) {
  const list = byId(id);
  list.replaceChildren();
  if (!paths.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = emptyLabel;
    list.append(empty);
    return;
  }
  paths.forEach((path) => {
    const row = document.createElement("div");
    row.className = "path-row";
    const value = document.createElement("span");
    value.title = path;
    value.textContent = path;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "path-remove";
    remove.textContent = "Remove";
    remove.setAttribute("aria-label", `Remove ${path}`);
    remove.addEventListener("click", () => removeProjectPath(kind, path));
    row.append(value, remove);
    list.append(row);
  });
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
    const next = {
      roots:
        kind === "root"
          ? projectSettings.roots.filter((value) => value !== path)
          : [...projectSettings.roots],
      excludedPaths:
        kind === "exclusion"
          ? projectSettings.excludedPaths.filter((value) => value !== path)
          : [...projectSettings.excludedPaths],
    };
    await saveProjectSettings(next);
    toast(kind === "root" ? "Project folder removed." : "Folder included again.");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

async function addProjectPath(kind, button) {
  try {
    const path = await runButton(button, "Choosing", () =>
      invoke("choose_project_folder", {
        prompt: kind === "root" ? "Choose a project folder to index" : "Choose a folder to exclude",
      }),
    );
    if (!path) return;
    const key = kind === "root" ? "roots" : "excludedPaths";
    if (projectSettings[key].includes(path)) {
      toast(kind === "root" ? "That project folder is already indexed." : "That folder is already excluded.");
      return;
    }
    const next = {
      roots: [...projectSettings.roots],
      excludedPaths: [...projectSettings.excludedPaths],
    };
    next[key].push(path);
    await saveProjectSettings(next);
    toast(kind === "root" ? "Project folder added and indexed." : "Folder excluded from the index.");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

function renderSnippets(activeId = byId("snippet-id").value) {
  const list = byId("snippet-list");
  list.replaceChildren();
  if (!snippets.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No snippets yet.";
    list.append(empty);
    return;
  }
  snippets.forEach((snippet) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `snippet-item ${snippet.id === activeId ? "active" : ""}`.trim();
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
  byId("snippet-id").value = snippet.id;
  byId("snippet-title").value = snippet.title;
  byId("snippet-keyword").value = snippet.keyword;
  byId("snippet-content").value = snippet.content;
  byId("delete-snippet").hidden = !snippet.id;
  renderSnippets(snippet.id);
  byId("snippet-title").focus();
}

function bindEvents() {
  document.querySelectorAll(".nav-item").forEach((button) => {
    button.addEventListener("click", () => selectTab(button.dataset.tab));
  });
  byId("refresh-overview").addEventListener("click", loadOverview);
  byId("clear-activity").addEventListener("click", async () => {
    if (!window.confirm("Clear Samlu's local agent activity?")) return;
    try {
      await invoke("clear_event_history");
      renderActivity([]);
      toast("Agent activity cleared.");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

  [
    "notify-needs-input",
    "notify-task-completed",
    "notify-turn-finished",
    "include-agent-summary",
    "agent-delivery",
    "notification-debounce",
    "close-behavior",
  ].forEach((id) => byId(id).addEventListener("change", () => queuePreferenceSave()));
  byId("clipboard-history-enabled").addEventListener("change", () =>
    queuePreferenceSave("launcher-save-state"),
  );
  byId("test-agent-notification").addEventListener("click", async (event) => {
    try {
      await runButton(event.currentTarget, "Sending", () => invoke("test_agent_notification"));
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("appearance").addEventListener("change", async (event) => {
    const appearance = event.currentTarget.value;
    applyAppearance(appearance);
    try {
      await invoke("set_appearance", { appearance });
      showSaveState("presence-save-state");
    } catch (error) {
      toast(errorMessage(error), true);
      await loadAppearance();
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
    const integration = agentIntegrations.find((item) => item.id === id);
    try {
      if (action === "preview") {
        showAgentPreview(
          `${integration?.name || "Agent"} configuration preview`,
          await invoke("preview_agent_integration", { id }),
        );
        return;
      }
      if (action === "remove") {
        if (!window.confirm(`Disconnect ${integration?.name || "this integration"}?`)) return;
        await runButton(button, "Disconnecting", () => invoke("remove_agent_integration", { id }));
        toast(`${integration?.name || "Integration"} disconnected.`);
      } else {
        await runButton(button, "Installing", () => invoke("apply_agent_integration", { id }));
        toast(`${integration?.name || "Integration"} connected.`);
      }
      await loadAgentStatus();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

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
      showSaveState("voice-provider-save-state");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("save-voice-stt-key").addEventListener("click", async (event) => {
    const input = byId("voice-stt-api-key");
    if (!input.value.trim()) return toast("Paste an API key first.", true);
    try {
      await runButton(event.currentTarget, "Saving", () =>
        invoke("set_voice_api_key", { role: "transcription", key: input.value.trim() }),
      );
      input.value = "";
      showSaveState("voice-provider-save-state", "Saved to Keychain");
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
      showSaveState("voice-text-save-state");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("open-voice-microphone").addEventListener("click", async (event) => {
    const button = event.currentTarget;
    try {
      const status = await invoke("get_voice_microphone_status");
      if (status === "not_determined") {
        await runButton(button, "Requesting", async () => {
          await invoke("request_voice_microphone");
          await refreshMicrophonePermission(20);
        });
        const updated = await refreshMicrophonePermission();
        if (updated === "authorized") {
          toast("Microphone access granted.");
        } else if (updated === "denied") {
          toast("Microphone access is off. Opening macOS Settings.", true);
          await invoke("open_voice_microphone_settings");
        } else if (updated === "not_determined") {
          toast("Waiting for the macOS microphone permission prompt.");
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
      if (!trusted) {
        await new Promise((resolve) => setTimeout(resolve, 700));
      }
      await invoke("open_voice_accessibility_settings");
      await loadVoiceSettings();
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
    showSaveState("voice-text-save-state");
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
      showSaveState("voice-text-save-state");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("save-voice-transform-key").addEventListener("click", async (event) => {
    const input = byId("voice-transform-api-key");
    if (!input.value.trim()) return toast("Paste an API key first.", true);
    try {
      await runButton(event.currentTarget, "Saving", () =>
        invoke("set_voice_api_key", { role: "transformation", key: input.value.trim() }),
      );
      input.value = "";
      showSaveState("voice-text-save-state", "Saved to Keychain");
      await loadVoiceSettings();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("voice-vocabulary").addEventListener("change", async (event) => {
    try {
      const terms = await invoke("set_voice_vocabulary", { text: event.currentTarget.value });
      event.currentTarget.value = terms.join("\n");
      showSaveState("voice-vocabulary-save-state", `${terms.length} terms saved`);
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  voiceHotkeyRecorder = createShortcutRecorder(
    "voice-hotkey",
    async (hotkey) => {
      await invoke("set_voice_hotkey", { hotkey });
      updateVoiceShortcutGuide(hotkey);
      showSaveState("voice-interaction-save-state");
    },
    loadVoiceSettings,
  );
  [
    ["dictation", "voice-dictation-delivery"],
    ["summary", "voice-summary-delivery"],
    ["prompt", "voice-prompt-delivery"],
  ].forEach(([mode, id]) => {
    byId(id).addEventListener("change", async (event) => {
      try {
        await invoke("set_voice_delivery", { mode, delivery: event.currentTarget.value });
        showSaveState("voice-interaction-save-state");
      } catch (error) {
        toast(errorMessage(error), true);
        await loadVoiceSettings();
      }
    });
  });
  byId("voice-interface-sounds").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_interface_sounds", { enabled: event.currentTarget.checked });
    } catch (error) {
      toast(errorMessage(error), true);
      await loadVoiceSettings();
    }
  });
  byId("voice-pet-capsule").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_pet_capsule", { enabled: event.currentTarget.checked });
    } catch (error) {
      toast(errorMessage(error), true);
      await loadVoiceSettings();
    }
  });
  byId("voice-history-search").addEventListener("input", loadVoiceHistory);
  byId("voice-history-raw").addEventListener("change", renderVoiceHistory);
  byId("voice-keep-history").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_keep_history", { enabled: event.currentTarget.checked });
      showSaveState("voice-history-save-state");
      await loadVoiceHistory();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("clear-voice-history").addEventListener("click", async () => {
    try {
      await invoke("clear_voice_history");
      showSaveState("voice-history-save-state", "Cleared");
      await loadVoiceHistory();
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });

  launcherHotkeyRecorder = createShortcutRecorder(
    "launcher-hotkey",
    async (hotkey) => {
      await invoke("set_launcher_hotkey", { hotkey });
      showSaveState("launcher-save-state");
    },
    async () => launcherHotkeyRecorder.set(await invoke("get_launcher_hotkey")),
  );
  byId("open-launcher").addEventListener("click", async () => {
    try {
      await invoke("launcher_toggle");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("add-project-root").addEventListener("click", (event) =>
    addProjectPath("root", event.currentTarget),
  );
  byId("add-project-exclusion").addEventListener("click", (event) =>
    addProjectPath("exclusion", event.currentTarget),
  );
  byId("reindex-projects").addEventListener("click", async (event) => {
    try {
      const projectCount = await runButton(event.currentTarget, "Indexing", () =>
        invoke("reindex_projects"),
      );
      projectSettings.projectCount = projectCount;
      renderProjectSettings();
      toast(`Indexed ${projectCount} projects.`);
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("clear-clipboard").addEventListener("click", async () => {
    if (!window.confirm("Clear Samlu's local clipboard history?")) return;
    try {
      await invoke("clear_clipboard_history");
      toast("Clipboard history cleared.");
    } catch (error) {
      toast(errorMessage(error), true);
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
    if (!id || !window.confirm("Delete this snippet?")) return;
    try {
      await invoke("delete_snippet", { id });
      snippets = snippets.filter((item) => item.id !== id);
      byId("snippet-form").hidden = true;
      renderSnippets("");
      toast("Snippet deleted.");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
}

async function boot() {
  bindEvents();
  window.addEventListener("focus", () => {
    if (byId("tab-voice").classList.contains("active")) loadVoiceSettings();
  });
  await loadOverview();
  await listen("agent://event", () => loadOverview());
  await listen("appearance://changed", (event) => {
    applyAppearance(event.payload);
    byId("appearance").value = event.payload;
  });
  await listen("settings://tab", (event) => {
    const tab = typeof event.payload === "string" ? event.payload : event.payload?.tab;
    if (["overview", "agents", "voice", "launcher", "about"].includes(tab)) selectTab(tab);
  });
}

boot().catch((error) => toast(errorMessage(error), true));
