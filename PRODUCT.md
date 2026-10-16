# Samlu product boundary

## Current release

Samlu is an AI development kit for macOS and Windows, for developers using
Claude Code and Codex. It should let someone speak an instruction, find a developer utility,
or leave an agent running without monitoring a terminal.

The current release has three surfaces:

1. Speech to text on this Mac (Whisper, Apple) or through a chosen cloud provider, with normal dictation, optional cleanup, summary, and detailed prompt modes.
2. A developer-focused launcher for apps, projects, source files, calculator
   expressions, snippets, clipboard history, commands, and screen colors,
   with Descriptive Find as an optional add-on that searches inside files by
   description (portable Rust, built to run on macOS and Windows). Projects
   have configurable actions on every Return key — open in an editor or agent
   app, start an agent CLI, run the project's scripts — and background tasks
   Samlu tracks to completion.
3. Configurable native macOS notifications for agent input and completion,
   with a playful Samlu Island in two styles (Bloom and Buddy) that follows
   the System, Light or Dark appearance.

## Deferred work

- Linux support
- Code-signed Windows installer
- Full Spotlight replacement behavior
- iPhone companion app
- Hosted APNs relay
- Contextual character feedback beyond the island (voice, launcher search)

The old mascot is removed from the app runtime. On the island, the Buddy style
now uses the Samlu mark itself as the character. Character feedback anywhere
else is still deferred.

## Product principles

1. Native on each platform. Keychain, menu bar behavior, native notifications,
   global shortcuts, and system color sampling should feel at home on macOS;
   on Windows the same jobs fall to Credential Manager, the system tray,
   Windows notifications and the platform's own keys and terminals.
2. Developer-focused and compact. Favor fast keyboard interaction, high
   information density, direct copy, and predictable behavior.
3. User-controlled interruption. Notification triggers, summaries, voice
   previews, and clipboard capture are explicit preferences.
4. Preserve user configuration. Agent integration edits are previewable,
   merged, and backed up.
5. Keep secrets out of preference files. Credentials belong in Keychain, or
   Credential Manager on Windows.
6. Make cloud boundaries visible. Voice data goes to the configured provider;
   future mobile content is encrypted before it reaches the relay.

## Success criteria

- Voice works with either tap-toggle or hold-to-talk and pastes into the app
  that was active when recording began.
- Prompt mode produces a useful coding brief without inventing requirements.
- Claude and Codex notifications still work after Samlu restarts.
- A developer can reach any launcher tool from one keyboard-driven surface.
- A developer can open a project in their tool of choice, start an agent in it,
  or run it, from one key on the project.
- Optional data collection, especially clipboard history and agent summaries,
  is configurable and understandable.
