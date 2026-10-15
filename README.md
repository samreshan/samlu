<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/samlu-mark-dark.svg">
  <img src=".github/assets/samlu-mark-light.svg" alt="Samlu" width="104" height="104">
</picture>

# Samlu

**Speak an instruction. Find any tool. Leave an agent running.**

A macOS development kit for people working with Claude Code and Codex —
global voice dictation, a keyboard-driven developer launcher, and agent
completion signals that reach you without watching a terminal.

[![Download](https://img.shields.io/github/v/release/samreshan/samlu?style=for-the-badge&label=Download&color=efd01d&labelColor=44390c)](https://github.com/samreshan/samlu/releases/latest)

[![macOS 11+](https://img.shields.io/badge/macOS-11%2B-44390c?style=flat-square)](https://github.com/samreshan/samlu/releases/latest)
[![Universal](https://img.shields.io/badge/Apple%20Silicon%20%2B%20Intel-universal-44390c?style=flat-square)](https://github.com/samreshan/samlu/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/samreshan/samlu/total?style=flat-square&color=44390c)](https://github.com/samreshan/samlu/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-44390c?style=flat-square)](LICENSE)

</div>

This release intentionally targets macOS only. Windows and iOS are outside the
current build boundary.

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

## What works

### Voice

- Tap `Option+V` to start or stop dictation.
- Hold `Option+V`, speak, and release to process.
- Add `Shift` for a concise summary.
- Add `Command` for a detailed coding-agent prompt.
- Transcribe on this Mac with Whisper (whisper.cpp, Metal) or Apple's
  on-device speech (macOS 26+), with no account and no network.
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

### Updates

Samlu checks the latest GitHub release every few hours (Settings → General →
Software Update, or **Check for Updates…** in the menu bar). Updates are
downloaded, verified against Samlu's signing key, and installed in place; the
new version starts the next time Samlu does, and **Restart to Update** applies
it straight away. Automatic checks and automatic installs can each be turned
off. Samlu never asks for an administrator password on its own: if it cannot
replace itself silently, it waits for you to choose Install.

### Agent notifications

- Claude Code HTTP hooks cover permission/input requests, stopped turns, and
  completed tasks.
- Codex uses its external `notify` command for completed turns.
- Hook authentication persists across Samlu restarts.
- Native macOS notification triggers, summary content, and turn debounce are
  configurable.
- Updates can also appear on the Samlu Island at the top of the screen, in one
  of two styles chosen in Settings › Agents: Bloom (grows out of the notch) or
  Buddy (the Samlu mark comes along and reacts). It follows the System, Light
  or Dark appearance.
- Recent normalized agent events are stored locally for the status view.
- Existing Claude and Codex configuration is preserved and backed up before
  Samlu writes a merged configuration.

## Development

Requirements:

- macOS 11 or later
- Rust stable
- Xcode Command Line Tools
- Tauri CLI v2 for bundled builds

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

Every push to `main` runs CI and, if it passes, publishes a universal release
(`.github/workflows/release.yml`). The version in `src-tauri/tauri.conf.json`
ships first; later pushes bump the patch number (0.2.0, 0.2.1, …). Raise that
version to start a new minor or major series.

Self-update needs the updater signing key in the repository's Actions secrets
as `TAURI_SIGNING_PRIVATE_KEY` (the contents of the private key file) and, if
the key has a password, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Its public half
is `plugins.updater.pubkey` in `tauri.conf.json`. With the secret set, each
release also carries `Samlu-<version>-macos-universal.app.tar.gz`, its `.sig`
and the `latest.json` feed that installed copies read. Keep the private key
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
