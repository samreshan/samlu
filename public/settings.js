/* Samlu settings window — Status / Setup / About tabs. Plain JS, no build
 * step. Talks to the Rust backend via window.__TAURI__ (withGlobalTauri).
 */

const tauri = window.__TAURI__ || {};
const invoke = tauri.core?.invoke || (async () => null);

window.addEventListener("error", (e) => console.error("[settings]", e.error || e.message));

// ---------------------------------------------------------------------------
// tabs
// ---------------------------------------------------------------------------

function wireTabs() {
  const buttons = document.querySelectorAll(".tab-btn");
  buttons.forEach((btn) => {
    btn.addEventListener("click", () => {
      buttons.forEach((b) => {
        b.classList.toggle("active", b === btn);
        b.setAttribute("aria-selected", String(b === btn));
      });
      document.querySelectorAll(".tab-panel").forEach((panel) => {
        panel.classList.toggle("active", panel.id === `tab-${btn.dataset.tab}`);
      });
    });
  });
}

// ---------------------------------------------------------------------------
// status tab
// ---------------------------------------------------------------------------

function currentScope() {
  const checked = document.querySelector('input[name="scope"]:checked');
  return checked ? checked.value : "global";
}

function currentProjectDir() {
  return document.getElementById("project-dir").value.trim();
}

async function refreshStatus() {
  const global = currentScope() === "global";
  const projectDir = global ? undefined : currentProjectDir();

  try {
    const status = await invoke("get_hook_status", { global, projectDir });
    if (!status) return; // running outside Tauri (shouldn't happen for this window)

    setPill("server-status", status.serverRunning ?? status.server_running, "Running", "Not running");
    document.getElementById("server-port").textContent = status.port ?? "—";
    setPill("hook-status", status.installed, "Installed", "Not installed");
    document.getElementById("last-event").textContent = status.lastEvent ?? status.last_event ?? "None yet";
  } catch (err) {
    console.error("[settings] get_hook_status failed", err);
  }
}

function setPill(id, ok, okText, notOkText) {
  const el = document.getElementById(id);
  el.textContent = ok ? okText : notOkText;
  el.classList.remove("pill-unknown", "pill-ok", "pill-bad");
  el.classList.add(ok ? "pill-ok" : "pill-bad");
}

// ---------------------------------------------------------------------------
// setup tab
// ---------------------------------------------------------------------------

function wireSetup() {
  const projectRow = document.getElementById("project-dir-row");
  document.querySelectorAll('input[name="scope"]').forEach((radio) => {
    radio.addEventListener("change", () => {
      projectRow.hidden = currentScope() !== "project";
    });
  });

  document.getElementById("preview-btn").addEventListener("click", async () => {
    const message = document.getElementById("setup-message");
    const diffView = document.getElementById("diff-view");
    message.textContent = "";
    try {
      const global = currentScope() === "global";
      const projectDir = global ? undefined : currentProjectDir();
      const merged = await invoke("preview_hook_merge", { global, projectDir });
      diffView.textContent = merged;
      diffView.hidden = false;
    } catch (err) {
      message.textContent = String(err);
    }
  });

  document.getElementById("apply-btn").addEventListener("click", async () => {
    const message = document.getElementById("setup-message");
    message.textContent = "Applying…";
    try {
      const global = currentScope() === "global";
      const projectDir = global ? undefined : currentProjectDir();
      await invoke("apply_hook_merge", { global, projectDir });
      message.textContent = "Done — a backup of your previous settings.json was saved automatically.";
      refreshStatus();
    } catch (err) {
      message.textContent = String(err);
    }
  });

  document.getElementById("copy-snippet-btn").addEventListener("click", async () => {
    try {
      const snippet = await invoke("get_hook_snippet");
      await navigator.clipboard.writeText(snippet);
      const message = document.getElementById("setup-message");
      message.textContent = "Copied to clipboard.";
    } catch (err) {
      console.error("[settings] copy snippet failed", err);
    }
  });
}

// ---------------------------------------------------------------------------
// boot
// ---------------------------------------------------------------------------

wireTabs();
wireSetup();
refreshStatus();
document.getElementById("recheck-btn").addEventListener("click", refreshStatus);
