<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/samlu-mark-dark.svg">
  <img src=".github/assets/samlu-mark-light.svg" alt="Samlu" width="104" height="104">
</picture>

# Samlu

**Speak an instruction. Find any tool. Leave an agent running.**

A development kit for macOS and Windows, for people working with Claude Code and Codex —
global voice dictation, a keyboard-driven developer launcher, and agent
completion signals that reach you without watching a terminal.

[![Download](https://img.shields.io/github/v/release/samreshan/samlu?style=for-the-badge&label=Download&color=efd01d&labelColor=44390c)](https://github.com/samreshan/samlu/releases/latest)

[![macOS 11+](https://img.shields.io/badge/macOS-11%2B-44390c?style=flat-square)](https://github.com/samreshan/samlu/releases/latest)
[![Windows 10+](https://img.shields.io/badge/Windows-10%2B%20x64-44390c?style=flat-square)](https://github.com/samreshan/samlu/releases/latest)
[![Universal](https://img.shields.io/badge/Apple%20Silicon%20%2B%20Intel-universal-44390c?style=flat-square)](https://github.com/samreshan/samlu/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/samreshan/samlu/total?style=flat-square&color=44390c)](https://github.com/samreshan/samlu/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-44390c?style=flat-square)](LICENSE)

</div>

Samlu runs on macOS 11 or later and on Windows 10 and 11 (64-bit). iOS is
outside the current build boundary. This README describes the Mac; [On
Windows](#on-windows) lists what differs there.

## Download

Grab the latest `.dmg` from the
[Releases page](https://github.com/samreshan/samlu/releases/latest). Builds are
universal — one download covers both Apple Silicon and Intel Macs running
macOS 11 or later. A `.zip` of the raw `.app` is published alongside it.

1. Open the disk image and drag **Samlu** into Applications.
2. Clear the quarantine flag (see below).
3. Launch it. Samlu lives in the menu bar rather than the Dock.

> [!IMPORTANT]
> Released builds are not signed with an Apple Developer ID yet, so macOS
> quarantines them on download and refuses to open them. Clear the flag once,
> after moving the app into Applications:
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/Samlu.app
> ```
>
> On macOS 15 and later this step is required — right-clicking and choosing
> *Open* no longer bypasses Gatekeeper. Because each unsigned build carries a
> different ad-hoc signature, macOS treats an upgrade as a new app, so
> Microphone and Accessibility access have to be granted again after updating.
>
> This is needed once. Updates after that are installed by Samlu itself (see
> [Updates](#updates)) and do not need the quarantine step.

Samlu asks for two permissions, both on first use and both revocable in
System Settings → Privacy & Security:

| Permission | Why |
| --- | --- |
| Microphone | Recording dictation. Audio stays on this Mac with on-device engines, or goes only to the speech provider you configure. |
| Accessibility | Returning the result to the field you were typing in. |

### Windows

Download `Samlu-<version>-windows-x64-setup.exe` from the same
[Releases page](https://github.com/samreshan/samlu/releases/latest) and run it.
It installs for your account only, so no administrator rights are needed, and
fetches Microsoft Edge WebView2 if the PC doesn't have it yet. Samlu lives in
the system tray: left-click the icon for Settings, right-click it for the menu.

> [!NOTE]
> The installer isn't code-signed yet, so Microsoft Defender SmartScreen may
> warn the first time it runs. Choose **More info**, then **Run anyway**.

Windows asks for no permissions up front. Dictation needs **Settings › Privacy
& security › Microphone › Let desktop apps access your microphone**, which is
on by default; Samlu links there if it is off. Pasting the result needs no
permission at all.

## What works

### Voice

- Tap `Option+V` to start or stop dictation.
- Hold `Option+V`, speak, and release to process.
- Add `Shift` for a concise summary.
- Add `Command` for a detailed coding-agent prompt.
- Transcribe on this Mac with Whisper (whisper.cpp, Metal), Underdog Bark 3.0
  for English (Core ML on the Neural Engine, Apple silicon with macOS 15+), or
  Apple's on-device speech (macOS 26+), with no account and no network.
- Use Whisper models you already have (MacWhisper, Superwhisper, Hugging Face
  cache, or any `.bin` file), or download one from Settings.
- Or use a cloud provider: Groq, OpenAI, Deepgram, ElevenLabs, or any
  OpenAI-compatible endpoint.
- Optional AI cleanup removes filler words and applies spoken corrections,
  using a cloud model or a local one through Ollama or LM Studio.
- Custom vocabulary and language selection.
- Searchable local transcript history (text only, last 200 results).
- Review the result in an optional compact editable preview before pasting.
- Store API keys in macOS Keychain.

The shortcut, models, provider layout, and preview behavior are configurable
in the Voice settings tab.

### Developer launcher

The default launcher shortcut is `Option+H`. It avoids taking over macOS
Spotlight's `Command+Space` binding and can be changed in Launcher settings.

- Search installed applications, projects, and source files. Results are
  ranked by how well they match — an exact or leading match on an app's name
  puts that app first — and by what you open most. Samlu's own commands only
  appear when you type their name, or everything behind `>`.
- An empty search shows what you open most, then Samlu's commands.
- Return opens the selection, Command-Return shows it in Finder, and the first
  Escape clears the search before the second closes the launcher. On a
  project, every Return key runs the action you choose (below).
- Type `=` for calculator expressions.
- Type `;` for snippets.
- Type `@` for opt-in local clipboard history.
- Type `>` for commands, including the macOS screen color picker.
- Type `!` for the tasks Samlu is running for your projects.
- Open Terminal and Samlu settings from the launcher.

Project discovery starts with common existing development folders. Settings ›
Projects can add or remove indexed roots, exclude folders, and trigger a manual
reindex without running a permanent file-system watcher.

#### Project actions

Every Return key on a project result runs an action you choose in Settings ›
Projects: Return, Command-Return, Option-Return, Control-Return and
Shift-Return. Out of the box Return opens the folder and Command-Return shows
it in Finder, as before; a key with nothing set opens the project's actions.

- Tab (or Command-K) on a project lists everything you can do with it: your
  actions, the scripts the project defines, and its running tasks.
- An action opens the project in an app (VS Code, Antigravity, Antigravity
  IDE, the Claude or Codex app, a terminal, a Git client, or any app you
  pick), runs a command in a terminal (Claude Code, Codex, Gemini, `agy`, or
  your own, such as a tunnel script), runs the project's own script, copies
  its path, opens a link, or runs a list of steps in the background.
- Samlu suggests actions for the editors, agent apps, CLIs and terminals it
  finds on this Mac, and you can add, edit, reorder or remove any of them.
- Scripts come from `package.json` (run with the package manager its lockfile
  names), `Cargo.toml`, `Makefile`, `justfile`, `go.mod`, `Package.swift`,
  `deno.json`, `pyproject.toml`, `manage.py` and Compose files, in the
  project folder and the folders directly inside it.
- Commands start in the project folder with your login shell's PATH, so tools
  installed through Homebrew or into `~/.local/bin` are found. `{path}` and
  `{name}` stand for the project.
- Background tasks keep their output, pick out any link they print (a tunnel
  URL, say), and notify you when they finish or fail. Type `!` to see them;
  Stop, Run Again, the link and the full log are a key away. Quitting Samlu
  stops them.

#### Descriptive Find (optional add-on)

Descriptive Find searches inside your files by what they say. Turn it on in
Launcher settings, choose the folders to search (Documents and Desktop to
start), then type `?` and describe what you're looking for: `? can I keep a
pet in the flat` finds the clause in the lease, `? bill from the branding
agency` finds the invoice. The result names the page or line and quotes the
passage. Strong matches also join the end of ordinary searches.

- Reads text, Markdown, Word (`.docx`), OpenDocument (`.odt`), RTF, HTML and
  PDF files, plus source code if you turn on code search. Keys, password files,
  hidden folders, `~/Library` and dependency folders are never read.
- Meaning and exact words are ranked together, so a description finds the
  passage and an invoice number or error code is still matched as written.
  Words match while you type; meaning results take over once you pause.
- Choose the model that supplies meaning in Launcher settings:
  - **Large: EmbeddingGemma 2**, Google DeepMind's multilingual embedding
    model, the one DigUp uses (about 865 MB with its image and audio
    encoders, downloaded once and verified). It runs on the GPU through
    llama.cpp's official `llama-server` build (Metal on macOS, Vulkan or CPU
    on Windows), in a helper process that starts when you search and quits
    after ten minutes unused. A query takes about 20 ms.
  - **Small: Model2Vec `potion-retrieval-32M`** (about 125 MB). A query embeds
    in microseconds on any CPU.
  - **None**: exact words only, nothing to download.
- A folder watcher keeps the index current; a saved file is searchable a
  moment later. The index is one SQLite file in Samlu's app data folder, and
  **Remove…** deletes it and the model.
- The engine is portable Rust (SQLite FTS5, Model2Vec, `notify`) and builds for
  macOS and Windows.

The search design (hybrid scoring, a per-query meaning scale with an honest
"nothing found", words-then-meaning while typing) follows
[DigUp](https://github.com/ARahim3/DigUp), an on-device file
search app built on EmbeddingGemma 2. Samlu uses its own implementation, offers
EmbeddingGemma 2 or a lighter model, and searches text-bearing files rather
than photos, audio or video.

### On Windows

Everything above works on Windows 10 and 11 (64-bit). What differs:

| | macOS | Windows |
| --- | --- | --- |
| Global shortcuts | Option and Command (`Option+V`, `Option+H`) | Alt and Win (`Alt+V`, `Alt+H`) |
| Approve and deny shortcuts | `Option+Command+Y` and `Option+Command+N` | `Ctrl+Alt+Y` and `Ctrl+Alt+N` |
| Shortcuts in Samlu's windows | Command | Ctrl: Ctrl-Enter shows a project in File Explorer, Ctrl-K lists its actions |
| On-device speech | Whisper on Metal, Underdog Bark (Apple silicon, macOS 15+), Apple speech (macOS 26+) | Whisper on the CPU |
| API keys | Keychain | Windows Credential Manager, under the same names |
| Putting the result back | Accessibility permission | Ctrl+V into the window you started in; no permission |
| Modifier-only shortcuts | Accessibility permission | No permission needed |
| Launcher apps | Applications folders | Start Menu shortcuts and Microsoft Store apps |
| File search | Spotlight | A quick walk of Desktop, Documents, Downloads and your home folder |
| Color picker | The macOS color sampler | Click anywhere on screen; Esc or a right click cancels |
| Project terminals | Terminal, iTerm, Warp, Ghostty | Windows Terminal, PowerShell, Command Prompt, WezTerm, Alacritty |
| Project commands run in | Your login shell, with its PATH | `cmd.exe` (as npm scripts do), with PATH as Windows has it now |
| Samlu Island | Hangs from the notch or the menu bar | Top centre of the active display |
| System sounds for notifications | `/System/Library/Sounds` (`.aiff`) | `C:\Windows\Media` (`.wav`) |
| Waiting count | Number beside the menu-bar icon | Tray tooltip, "Samlu · 2 agents waiting" |
| View | Opens the session's Warp tab, Claude or Codex conversation, or brings the agent's app forward by bundle id; otherwise says it couldn't find the session | Opens the session through its deep link (`claude://`, `codex://`, `warp://` where registered); otherwise says it couldn't find the session |
| Updates | Replaced in place, applied on restart | Through the installer, which restarts Samlu |

Gemini CLI and Antigravity hooks call `curl.exe`, which Windows 10 and 11
include. Claude Code hooks are HTTP hooks and need nothing extra.

### Updates

Samlu checks the latest GitHub release every few hours (Settings → General →
Software Update, or **Check for Updates…** in the menu bar). Updates are
downloaded, verified against Samlu's signing key, and installed in place; the
new version starts the next time Samlu does, and **Restart to Update** applies
it straight away. Automatic checks and automatic installs can each be turned
off. Samlu never asks for an administrator password on its own: if it cannot
replace itself silently, it waits for you to choose Install. On Windows the
installer closes Samlu while it runs, so updates there always wait for
Install.

### Agent notifications

Samlu receives events from Claude Code, Codex, Gemini CLI and Antigravity. It
tells you when an agent needs you, finishes a turn, or completes a task.

- Claude Code HTTP hooks cover permission and input requests, stopped turns,
  and completed tasks. Codex sends permission requests through its hooks and
  completed turns through its external `notify` command. Gemini CLI and
  Antigravity post through `curl`.
- Hook authentication persists across Samlu restarts.
- Native macOS notification triggers, summary content, and turn debounce are
  configurable.
- Notices can also appear on the Samlu Island at the top of the screen, in one
  of two styles chosen in Settings › Agents: Bloom (grows out of the notch) or
  Buddy (the Samlu mark comes along and reacts). It follows the System, Light
  or Dark appearance.
- Recent normalized agent events are stored locally for the status view.
- Existing Claude and Codex configuration is preserved. Samlu previews the
  change, backs up the file, then writes a merged configuration.
- A Claude Code or Codex connection set up before this version shows **Update
  needed** in Settings › Agents. **Update** previews, backs up and merges the
  new hooks, the same as the first setup.

#### Approvals

- When Claude Code or Codex asks permission to run a tool, the island shows the
  request with **Deny**, **Approve** and **View**. Claude Code also offers
  **Always allow** when it suggests a rule. The rule is shown in the button's
  tooltip.
- The agent's own prompt stays open in its terminal or app. Whichever answers
  first wins. A request answered there leaves the island quietly.
- Several requests queue, oldest first, with "1 of 3" and arrows to move
  between them. A new request never replaces one you haven't answered.
- Keyboard: `Option+Command+Y` approves and `Option+Command+N` denies the front
  card. These shortcuts work only while an approval card is showing. Change
  them in Settings › Agents.
- Approval cards use the island even when notifications are set to
  Notification Center, because banners can't carry buttons. Notification Center
  still gets its entry.
- Notices that need input but have no decision to make, such as an idle prompt,
  stay on the island with **View** and **Dismiss**.
- Gemini CLI and Antigravity get sounds and View only.

#### Codex

- Codex waits 30 seconds for an answer by default (10 to 120 seconds, set in
  Settings › Agents). Then Samlu stops waiting and Codex shows its own prompt.
  The card changes to **Answer in Codex**, with View only. It counts as waiting
  until that session's next event or until you dismiss it.
- View on a Codex request hands the request back to Codex first, then opens the
  thread.
- Codex runs Samlu's hooks only after you trust them. Samlu never trusts them
  for you. After you connect Codex, Settings › Agents asks you to trust them
  once, with `/hooks` in the Codex CLI or in the prompt the Codex app shows. The
  Codex row says "Waiting for Codex to trust Samlu's hooks" until the first
  Codex hook event arrives.
- A Codex connection set up before hooks existed (only the `notify` command)
  shows **Update needed**. **Update** adds the hooks and keeps `notify`.

#### View

- View opens the session itself: the Warp tab, the Claude desktop conversation,
  or the Codex thread. If Samlu knows which app runs the session, it brings
  that app forward; otherwise the island says it couldn't find the session.
  View never opens Samlu's own pages.

#### Sounds

- Three kinds: **needs you** (approvals, input requests and reminders),
  **task completed**, and **turn finished**. Each can be None, a macOS system
  sound, or a file of your own (Custom…). The play button previews the choice.
- Defaults: Glass for needs you, Hero for task completed, None for turn
  finished. The master switch and the volume (70% by default) are in Settings
  › Agents.
- Samlu's own banners stay silent, so each event rings once.

#### Reminders, waiting count and sessions

- An unanswered request rings again after 1, 3, 5 or 10 minutes (3 by default),
  once per request. Choose Off to turn reminders off.
- The number beside the menu-bar icon is how many sessions are waiting for you.
  It is hidden when none are.
- The menu-bar menu has an Agents section listing each agent session Samlu
  knows, with its state: working, waiting for you, or idle. Click one to View
  it. The same list is in the launcher (type `#`; Return views the selection)
  and in the Sessions group of Settings › Agents.
- A session drops off when it ends, or after 12 hours without an event. The
  list is kept in memory, so it starts empty after a restart and fills in as
  each session sends its next event.

#### History

- Approvals are recorded in Recent Activity as needs-input events, with the
  outcome: approved or denied in Samlu, answered in the agent, or handed back to
  Codex. Only the action line, up to 160 characters, is stored. Clear deletes
  it.

## Development

Requirements:

- macOS 11 or later
- Rust stable
- Xcode Command Line Tools
- Tauri CLI v2 for bundled builds

On Windows: Rust stable with the MSVC toolchain, the Visual Studio C++ build
tools, CMake (for whisper.cpp) and the Tauri CLI v2. The same commands below
work there, and `cargo tauri build` writes the installer under
`src-tauri/target/release/bundle/nsis/`. A Linux build compiles and runs the
tests, with native features stubbed out; it is not a supported platform.

Run a development build:

```bash
cd src-tauri
cargo run
```

Run checks:

```bash
cd src-tauri
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### Releases

Every push to `main` runs CI on macOS and Windows and, if it passes, publishes
a release with a universal macOS build and a Windows installer
(`.github/workflows/release.yml`). The version in `src-tauri/tauri.conf.json`
ships first; later pushes bump the patch number (0.2.0, 0.2.1, …). Raise that
version to start a new minor or major series.

Self-update needs the updater signing key in the repository's Actions secrets
as `TAURI_SIGNING_PRIVATE_KEY` (the contents of the private key file) and, if
the key has a password, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Its public half
is `plugins.updater.pubkey` in `tauri.conf.json`. With the secret set, each
release also carries `Samlu-<version>-macos-universal.app.tar.gz` and the
Windows installer's `.sig`, plus the `latest.json` feed that installed copies
read on both platforms. Keep the private key
backed up: a release signed with any other key cannot update existing installs.

To try an update end to end without publishing, run a debug bundle with
`SAMLU_UPDATER_ENDPOINT` pointing at a local `latest.json`, and
`SAMLU_UPDATER_FIRST_CHECK_SECS` to shorten the first check. Both are read
only by debug builds.

### Local build

Create an Apple Silicon app and DMG:

```bash
./scripts/build-mac.sh
```

Artifacts are written under
`src-tauri/target/aarch64-apple-darwin/release/bundle/`.

Without `APPLE_SIGNING_IDENTITY`, the local script uses an ad-hoc signature, so
Gatekeeper may warn on first launch and macOS privacy grants can need to be
re-added after rebuilding. For a stable production identity, export a
Developer ID Application identity as `APPLE_SIGNING_IDENTITY` and provide
Apple notarization credentials to Tauri.

## Security and data

- The agent hook server binds to `127.0.0.1`.
- Claude hook requests use an installation-specific bearer token in the local
  callback URL.
- Only you, acting in Samlu's island, menu, launcher or shortcuts, can approve
  or deny an agent request. The local hook server can't.
- Cloud provider API keys are stored in macOS Keychain, not Samlu JSON files.
- Snippets, launcher history, event history, and optional clipboard history are
  local to the Mac.
- Project actions run only commands you configured, in your own login
  environment. Task output is written to Samlu's app data folder (`tasks/`,
  the last 30 runs) and never leaves the Mac.
- Descriptive Find reads only the folders you choose, and its index stays on
  this computer. It goes online only to download the model you choose, once.
- With on-device engines, voice audio never leaves the Mac. With a cloud
  engine, audio goes only to the provider you configure.
- Transcript history is stored locally as text and can be turned off, which
  deletes it. Audio is never stored.

The source Icon Composer package is preserved at
`src-tauri/icons/samlu.icon`. The unsigned Tauri beta uses an `.icns` generated
from the same artwork.

The proposed iPhone notification relay is not implemented in this release.
Its pairing, encryption, and hosted APNs boundary are documented in
[`docs/APNS_RELAY.md`](docs/APNS_RELAY.md).

## License

Source code is Apache-2.0 licensed. The previous mascot implementation and art
have been removed from this build.
