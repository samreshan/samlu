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

- Search installed applications, projects, and source files.
- Type `=` for calculator expressions.
- Type `;` for snippets.
- Type `@` for opt-in local clipboard history.
- Type `>` for commands, including the macOS screen color picker.
- Open Terminal and Samlu settings from the launcher.

Project discovery starts with common existing development folders. Launcher
settings can add or remove indexed roots, exclude folders, and trigger a manual
reindex without running a permanent file-system watcher.

### Agent notifications

- Claude Code HTTP hooks cover permission/input requests, stopped turns, and
  completed tasks.
- Codex uses its external `notify` command for completed turns.
- Hook authentication persists across Samlu restarts.
- Native macOS notification triggers, summary content, and turn debounce are
  configurable.
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
