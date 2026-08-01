# Samlu

Samlu is a macOS AI development kit. The current build combines global voice
dictation, a compact developer launcher, and completion notifications from
Claude Code and Codex.

This release intentionally targets macOS only. Windows and iOS are outside the
current build boundary.

## Download

Grab the latest `.dmg` from the
[Releases page](https://github.com/samreshan/samlu/releases/latest). Builds are
universal — one download covers both Apple Silicon and Intel Macs running
macOS 11 or later.

Open the disk image, drag **Samlu** into Applications, and launch it. Samlu
lives in the menu bar rather than the Dock.

Released builds are not signed with an Apple Developer ID yet, so macOS
quarantines them on download. Clear the flag once, after moving the app into
Applications:

```bash
xattr -dr com.apple.quarantine /Applications/Samlu.app
```

On macOS 15 and later this step is required; right-clicking and choosing
*Open* no longer bypasses Gatekeeper. Because each unsigned build carries a
different ad-hoc signature, macOS treats an upgrade as a new app, so Microphone
and Accessibility access have to be granted again after updating.

## What works

### Voice

- Tap `Option+V` to start or stop dictation.
- Hold `Option+V`, speak, and release to process.
- Add `Shift` for a concise summary.
- Add `Command` for a detailed coding-agent prompt.
- Review the result in an optional compact editable preview before pasting.
- Use Groq or another OpenAI-compatible cloud endpoint.
- Use one shared endpoint by default, or configure separate speech and text
  transformation providers.
- Store endpoint-specific API keys in macOS Keychain.

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
- Voice audio is sent only to the cloud endpoint configured by the user.

The source Icon Composer package is preserved at
`src-tauri/icons/samlu.icon`. The unsigned Tauri beta uses an `.icns` generated
from the same artwork.

The proposed iPhone notification relay is not implemented in this release.
Its pairing, encryption, and hosted APNs boundary are documented in
[`docs/APNS_RELAY.md`](docs/APNS_RELAY.md).

## License

Source code is Apache-2.0 licensed. The previous mascot implementation and art
have been removed from this build.
