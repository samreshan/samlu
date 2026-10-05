const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const STEPS = ["Welcome", "Microphone", "Insertion", "Try it", "Alerts", "Done"];
const PRIMARY_LABELS = [
  "Get started",
  "Allow microphone",
  "Allow accessibility",
  "Continue",
  "Continue",
  "Start using Samlu",
];
const SKIPPABLE = new Set([2, 3]);

const byId = (id) => document.getElementById(id);

let step = 0;
let state = {
  microphone: "not_determined",
  accessibility: false,
  voiceReady: false,
  appleAvailable: false,
  recommendedModel: "",
  voiceHotkey: "Alt+V",
  delivery: "island",
};
let launcherHotkey = "Alt+H";
let tryDone = false;
let toastTimer = null;

/* ------------------------------------------------------------------ utils */

function toast(message, error = false) {
  const element = byId("toast");
  element.textContent = String(message);
  element.classList.toggle("error", error);
  element.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => element.classList.remove("show"), 3200);
}

function errorMessage(error) {
  if (typeof error === "string") return error;
  return error?.message || "Something went wrong.";
}

const SYMBOLS = {
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

function formatHotkey(hotkey) {
  return String(hotkey || "")
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => SYMBOLS[part.toLowerCase()] || part.toUpperCase())
    .join("");
}

function applyHotkeyLabels() {
  const voice = formatHotkey(state.voiceHotkey);
  const launcher = formatHotkey(launcherHotkey);
  document.querySelectorAll("[data-voice-hotkey]").forEach((node) => {
    node.textContent = voice;
  });
  document.querySelectorAll("[data-launcher-hotkey]").forEach((node) => {
    node.textContent = launcher;
  });
}

/* ------------------------------------------------------------------ render */

function renderSteps() {
  const list = byId("steps");
  if (!list.children.length) {
    STEPS.forEach((name) => {
      const item = document.createElement("li");
      const label = document.createElement("span");
      label.textContent = name;
      item.append(label);
      list.append(item);
    });
  }
  [...list.children].forEach((item, index) => {
    item.classList.toggle("done", index < step);
    item.classList.toggle("current", index === step);
    if (index === step) item.setAttribute("aria-current", "step");
    else item.removeAttribute("aria-current");
  });
}

function renderMicrophone() {
  const granted = state.microphone === "authorized";
  const blocked = state.microphone === "denied" || state.microphone === "restricted";
  byId("mic-granted").hidden = !granted;
  const button = byId("grant-mic");
  button.hidden = granted;
  button.textContent = blocked ? "Open Settings" : "Allow microphone";
  byId("mic-note").textContent = blocked
    ? "Microphone access was declined earlier. Enable Samlu under Privacy & Security › Microphone, then come back."
    : "macOS will show its own dialog. Samlu never records outside the shortcut.";
}

function renderAccessibility() {
  byId("acc-granted").hidden = !state.accessibility;
  byId("grant-acc").hidden = state.accessibility;
}

function renderTryStep() {
  byId("key-setup").hidden = state.voiceReady;
  if (!state.voiceReady && !tryDone) {
    byId("try-hint").textContent = "Add a transcription key first, then hold the shortcut.";
  }
}

const DELIVERY_CARDS = { pet: "pick-pet", island: "pick-island", system: "pick-system" };

function renderDelivery() {
  Object.entries(DELIVERY_CARDS).forEach(([value, id]) => {
    byId(id).setAttribute("aria-pressed", String(state.delivery === value));
  });
}

function render() {
  document.querySelectorAll(".step").forEach((section) => {
    section.classList.toggle("active", Number(section.dataset.step) === step);
  });
  renderSteps();
  renderMicrophone();
  renderAccessibility();
  renderTryStep();
  renderDelivery();

  byId("back").hidden = step === 0;
  byId("skip").hidden = !SKIPPABLE.has(step);

  let label = PRIMARY_LABELS[step];
  if (step === 1 && state.microphone === "authorized") label = "Continue";
  if (step === 1 && (state.microphone === "denied" || state.microphone === "restricted")) {
    label = "Open Settings";
  }
  if (step === 2 && state.accessibility) label = "Continue";
  byId("next-label").textContent = label;
}

/* ------------------------------------------------------------------- state */

async function refresh() {
  try {
    state = await invoke("onboarding_state");
    applyHotkeyLabels();
    render();
  } catch (error) {
    console.error(error);
  }
}

function applyAppearance(appearance) {
  if (["light", "dark"].includes(appearance)) document.documentElement.dataset.theme = appearance;
  else delete document.documentElement.dataset.theme;
}

async function loadAppearance() {
  try {
    applyAppearance(await invoke("get_appearance"));
  } catch (error) {
    console.error(error);
  }
}

async function loadLauncherHotkey() {
  try {
    launcherHotkey = await invoke("get_launcher_hotkey");
    applyHotkeyLabels();
  } catch (error) {
    console.error(error);
  }
}

/* ------------------------------------------------------------- navigation */

function goTo(next) {
  step = Math.max(0, Math.min(STEPS.length - 1, next));
  render();
}

async function grantMicrophone() {
  try {
    const status = await invoke("onboarding_request_microphone");
    if (status === "denied" || status === "restricted") {
      await invoke("onboarding_open_microphone_settings");
    }
  } catch (error) {
    toast(errorMessage(error), true);
  }
  setTimeout(refresh, 400);
}

async function grantAccessibility() {
  try {
    const trusted = await invoke("request_voice_accessibility");
    if (!trusted) {
      await new Promise((resolve) => setTimeout(resolve, 500));
      await invoke("open_voice_accessibility_settings");
    }
  } catch (error) {
    toast(errorMessage(error), true);
  }
  setTimeout(refresh, 400);
}

async function advance() {
  // On the permission steps the primary button does the granting until macOS
  // has actually answered, so its label always matches what it will do.
  if (step === 1 && state.microphone !== "authorized") {
    await grantMicrophone();
    return;
  }
  if (step === 2 && !state.accessibility) {
    await grantAccessibility();
    return;
  }
  if (step === STEPS.length - 1) {
    await invoke("onboarding_finish", { openAgents: false });
    return;
  }
  goTo(step + 1);
}

/* ----------------------------------------------------------------- wiring */

byId("next").addEventListener("click", advance);
byId("back").addEventListener("click", () => goTo(step - 1));
byId("skip").addEventListener("click", () => goTo(step + 1));
byId("grant-mic").addEventListener("click", grantMicrophone);
byId("grant-acc").addEventListener("click", grantAccessibility);

function showVoiceSetup(kind) {
  byId("voice-local-setup").hidden = kind !== "local";
  byId("voice-cloud-setup").hidden = kind !== "cloud";
  if (kind !== "local") return;
  const note = byId("voice-local-note");
  const go = byId("voice-local-go");
  if (state.appleAvailable) {
    note.textContent = "Uses Apple's on-device speech. macOS downloads the language once.";
    go.textContent = "Use Apple speech";
  } else {
    note.textContent = "Downloads an open Whisper model once. Nothing leaves this Mac.";
    go.textContent = "Download model";
  }
}

byId("voice-local").addEventListener("click", () => showVoiceSetup("local"));
byId("voice-cloud").addEventListener("click", () => showVoiceSetup("cloud"));

byId("voice-local-go").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  const progress = byId("voice-local-progress");
  button.disabled = true;
  try {
    if (state.appleAvailable) {
      await invoke("set_voice_stt", { choice: "apple", baseUrl: "", model: "" });
      progress.textContent = "Installing language…";
      await invoke("voice_apple_install", { language: "auto" });
    } else {
      const models = await invoke("get_voice_models");
      let path = models.models.find((model) => !model.missing)?.path;
      if (!path) {
        const stop = await listen("voice://model-download", ({ payload }) => {
          progress.textContent = `Downloading ${Math.floor((payload.received / payload.total) * 100)}%`;
        });
        try {
          path = await invoke("voice_download_model", { id: state.recommendedModel });
        } finally {
          stop();
        }
      }
      await invoke("set_voice_stt", { choice: "whisper_cpp", baseUrl: "", model: path });
    }
    progress.textContent = "Ready.";
    await refresh();
    byId("try-hint").textContent = "Hold the shortcut, then let go.";
  } catch (error) {
    toast(errorMessage(error), true);
    progress.textContent = "";
  } finally {
    button.disabled = false;
  }
});

byId("save-voice-key").addEventListener("click", async () => {
  const input = byId("voice-key");
  const key = input.value.trim();
  if (!key) {
    toast("Paste a key first.", true);
    return;
  }
  try {
    await invoke("set_voice_stt", {
      choice: byId("voice-cloud-provider").value,
      baseUrl: "",
      model: "",
    });
    await invoke("set_voice_api_key", { role: "transcription", key });
    input.value = "";
    toast("Key saved to Keychain.");
    await refresh();
    byId("try-hint").textContent = "Hold the shortcut, then let go.";
  } catch (error) {
    toast(errorMessage(error), true);
  }
});

async function pickDelivery(delivery) {
  try {
    await invoke("onboarding_set_delivery", { delivery });
    state.delivery = delivery;
    renderDelivery();
    // Fire a real event down the chosen path so the choice is demonstrated,
    // not just described.
    await invoke("test_agent_notification");
  } catch (error) {
    toast(errorMessage(error), true);
  }
}

Object.entries(DELIVERY_CARDS).forEach(([value, id]) => {
  byId(id).addEventListener("click", () => pickDelivery(value));
});

byId("connect-agents").addEventListener("click", async () => {
  try {
    await invoke("onboarding_finish", { openAgents: true });
  } catch (error) {
    toast(errorMessage(error), true);
  }
});

window.addEventListener("keydown", (event) => {
  if (event.target instanceof HTMLInputElement) return;
  if (event.key === "Enter") {
    event.preventDefault();
    advance();
  }
  if (event.key === "Escape" && step > 0) {
    event.preventDefault();
    goTo(step - 1);
  }
});

listen("voice://status", (event) => {
  const status = String(event.payload || "");
  byId("try-box").classList.toggle("listening", status === "listening");
  if (status === "listening") byId("try-hint").textContent = "Listening — release to send.";
  if (status === "processing") byId("try-hint").textContent = "Transcribing…";
});

listen("voice://result", (event) => {
  const text = String(event.payload || "").trim();
  if (!text) return;
  tryDone = true;
  const field = byId("try-field");
  field.textContent = text;
  const caret = document.createElement("span");
  caret.className = "caret";
  field.append(caret);
  byId("try-box").classList.remove("listening");
  byId("try-hint").textContent = "Nice. That is exactly what Samlu would have typed for you.";
});

listen("voice://error", (event) => {
  byId("try-box").classList.remove("listening");
  byId("try-hint").textContent = errorMessage(event.payload);
});

// Both permissions are granted outside this window, so re-read on every focus
// and keep a slow poll running while the guide is open.
window.addEventListener("focus", refresh);
setInterval(refresh, 2000);

listen("appearance://changed", (event) => applyAppearance(event.payload));

loadAppearance();
refresh();
loadLauncherHotkey();
render();
