/* Which desktop Samlu is running on, and the words and keys that go with it.
 * WebView2 names Windows in its user agent; WKWebView never does. Each page
 * loads this after its markup and before its own script, so the page's text
 * is adjusted before anything of the user's is added to it. */
(() => {
  const windows = /Windows/.test(navigator.userAgent);
  document.documentElement.dataset.platform = windows ? "windows" : "macos";

  const MAC_KEYS = {
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

  /* Global shortcuts name the keys themselves; "Cmd" is the Windows key. */
  const WINDOWS_KEYS = {
    cmd: "Win",
    command: "Win",
    super: "Win",
    meta: "Win",
    ctrl: "Ctrl",
    control: "Ctrl",
    alt: "Alt",
    option: "Alt",
    shift: "Shift",
  };

  /* Shortcuts inside Samlu's own windows use Ctrl where macOS uses ⌘: the
   * Windows key belongs to the system. ⌃ moves to the Windows key. */
  const WINDOWS_ACTION_KEYS = { ...WINDOWS_KEYS, cmd: "Ctrl", command: "Ctrl", ctrl: "Win", control: "Win" };

  /* Windows wording for Samlu's own macOS phrases, longest first. */
  const PHRASES = [
    ["Stored in your macOS Keychain", "Stored in Windows Credential Manager"],
    ["kept in your macOS Keychain", "kept in Windows Credential Manager"],
    ["stored in the macOS Keychain", "stored in Windows Credential Manager"],
    ["Saved to Keychain", "Saved to Credential Manager"],
    ["saved to Keychain", "saved to Credential Manager"],
    ["Samlu for macOS", "Samlu for Windows"],
    ["running in the menu bar", "running in the system tray"],
    ["Notification Center", "the notification center"],
    ["Show in Finder", "Show in File Explorer"],
    ["a Finder window", "a File Explorer window"],
    ["Command-Return", "Ctrl+Enter"],
    ["Option-Return", "Alt+Enter"],
    ["Control-Return", "Win+Enter"],
    ["On this Mac", "On this PC"],
    ["this Mac’s", "this PC’s"],
    ["This Mac’s", "This PC’s"],
    ["your Mac’s", "your PC’s"],
    ["this Mac", "this PC"],
    ["System Settings", "Windows Settings"],
    ["Finder", "File Explorer"],
  ];

  /** Samlu's own text, worded for this platform. */
  function text(value) {
    if (!windows || typeof value !== "string") return value;
    return PHRASES.reduce((result, [mac, win]) => result.split(mac).join(win), value);
  }

  /** Whether the primary modifier is held: ⌘ on macOS, Ctrl on Windows. */
  function primary(event) {
    return Boolean(windows ? event?.ctrlKey : event?.metaKey);
  }

  /** The modifier Samlu's chords call "ctrl": ⌃ on macOS, the Windows key
   * on Windows. */
  function secondary(event) {
    return Boolean(windows ? event?.metaKey : event?.ctrlKey);
  }

  window.SAMLU_PLATFORM = Object.freeze({
    windows,
    /** Key names for global shortcuts. */
    hotkeySymbols: windows ? WINDOWS_KEYS : MAC_KEYS,
    /** Key names for shortcuts inside Samlu's windows. */
    actionSymbols: windows ? WINDOWS_ACTION_KEYS : MAC_KEYS,
    /** Between key names: glyphs sit together, words take a plus. */
    keyJoiner: windows ? "+" : "",
    text,
    primary,
    secondary,
  });

  if (!windows) return;
  // Elements with a whole Windows version of their text.
  for (const element of document.querySelectorAll("[data-windows]")) {
    element.textContent = element.dataset.windows;
  }
  const root = document.body || document.documentElement;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const replaced = text(node.nodeValue);
    if (replaced !== node.nodeValue) node.nodeValue = replaced;
  }
  for (const name of ["title", "aria-label", "label", "placeholder"]) {
    for (const element of root.querySelectorAll(`[${name}]`)) {
      element.setAttribute(name, text(element.getAttribute(name)));
    }
  }
})();
