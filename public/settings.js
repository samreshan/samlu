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

async function loadVoiceSettings() {
  try {
    const [settings, microphoneStatus, accessibilityTrusted] = await Promise.all([
      invoke("get_voice_settings"),
      invoke("get_voice_microphone_status"),
      invoke("get_voice_accessibility_status"),
    ]);
    byId("voice-separate-providers").checked = settings.separateProviders;
    renderVoiceEndpoint("stt", settings.transcription);
    renderVoiceEndpoint("transform", settings.transformation);
    byId("voice-transform-provider-block").hidden = !settings.separateProviders;
    byId("voice-stt-provider-note").textContent = settings.separateProviders
      ? "Used only for speech to text."
      : "Also used for summaries and prompt transformation.";
    byId("voice-stt-model").value = settings.transcription.model || "";
    byId("voice-transform-model").value = settings.transformation.model || "";
    voiceHotkeyRecorder.set(settings.hotkey || "Alt+V");
    byId("voice-dictation-delivery").value = settings.delivery?.dictation || "instant_insert";
    byId("voice-summary-delivery").value = settings.delivery?.summary || "instant_insert";
    byId("voice-prompt-delivery").value = settings.delivery?.prompt || "editable_preview";
    byId("voice-interface-sounds").checked = settings.interfaceSounds !== false;
    byId("voice-pet-capsule").checked = settings.petCapsule !== false;
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

function renderVoiceEndpoint(prefix, endpoint) {
  byId(`voice-${prefix}-provider`).value = endpoint.provider;
  byId(`voice-${prefix}-base-url`).value = endpoint.baseUrl || "";
  byId(`voice-${prefix}-base-url-row`).hidden = endpoint.provider !== "custom";
  const keyStatus = byId(`voice-${prefix}-key-status`);
  keyStatus.className = `status-pill ${endpoint.hasApiKey ? "good" : "warn"}`;
  keyStatus.textContent = endpoint.hasApiKey ? "Key saved" : "Key required";
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

function endpointRole(prefix) {
  return prefix === "transform" ? "transformation" : "transcription";
}

async function saveVoiceEndpoint(prefix) {
  const provider = byId(`voice-${prefix}-provider`).value;
  const baseUrl = byId(`voice-${prefix}-base-url`).value.trim();
  if (provider === "custom" && !baseUrl) {
    throw new Error("Enter the OpenAI-compatible API base URL.");
  }
  await invoke("set_voice_endpoint", {
    role: endpointRole(prefix),
    provider,
    baseUrl,
  });
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

  byId("voice-separate-providers").addEventListener("change", async (event) => {
    const enabled = event.currentTarget.checked;
    try {
      await invoke("set_voice_separate_providers", { enabled });
      await loadVoiceSettings();
    } catch (error) {
      toast(errorMessage(error), true);
      await loadVoiceSettings();
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

  ["stt", "transform"].forEach((prefix) => {
    byId(`voice-${prefix}-provider`).addEventListener("change", async (event) => {
      const custom = event.currentTarget.value === "custom";
      byId(`voice-${prefix}-base-url-row`).hidden = !custom;
      if (custom) {
        const baseUrl = byId(`voice-${prefix}-base-url`);
        if (baseUrl.value === "https://api.groq.com/openai/v1") baseUrl.value = "";
        baseUrl.focus();
        return;
      }
      try {
        await saveVoiceEndpoint(prefix);
        await loadVoiceSettings();
        showSaveState("voice-provider-save-state");
      } catch (error) {
        toast(errorMessage(error), true);
      }
    });
    byId(`voice-${prefix}-base-url`).addEventListener("change", async () => {
      try {
        await saveVoiceEndpoint(prefix);
        await loadVoiceSettings();
        showSaveState("voice-provider-save-state");
      } catch (error) {
        toast(errorMessage(error), true);
      }
    });
    byId(`save-voice-${prefix}-key`).addEventListener("click", async (event) => {
      const keyInput = byId(`voice-${prefix}-api-key`);
      const key = keyInput.value.trim();
      if (!key) {
        toast("Paste an API key first.", true);
        return;
      }
      try {
        await runButton(event.currentTarget, "Saving", async () => {
          await saveVoiceEndpoint(prefix);
          await invoke("set_voice_api_key", { role: endpointRole(prefix), key });
        });
        keyInput.value = "";
        showSaveState("voice-provider-save-state", "Saved to Keychain");
        await loadVoiceSettings();
      } catch (error) {
        toast(errorMessage(error), true);
      }
    });
  });
  byId("voice-stt-model").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_stt_model", { model: event.currentTarget.value });
      showSaveState("voice-provider-save-state");
    } catch (error) {
      toast(errorMessage(error), true);
    }
  });
  byId("voice-transform-model").addEventListener("change", async (event) => {
    try {
      await invoke("set_voice_transform_model", { model: event.currentTarget.value });
      showSaveState("voice-provider-save-state");
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
