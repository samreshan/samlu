# Samlu product boundary

## Current release

Samlu is a macOS-only AI development kit for developers using Claude Code and
Codex. It should let someone speak an instruction, find a developer utility,
or leave an agent running without monitoring a terminal.

The current release has three surfaces:

1. Global speech to text with normal dictation, summary, and detailed prompt
   modes.
2. A developer-focused launcher for apps, projects, source files, calculator
   expressions, snippets, clipboard history, commands, and screen colors.
3. Configurable native macOS notifications for agent input and completion.

## Deferred work

- Windows support
- Local speech or text models
- Full Spotlight replacement behavior
- iPhone companion app
- Hosted APNs relay
- New Samlu mascot and contextual character feedback

The old mascot is removed from the app runtime. A future mascot will be a new
design and may appear contextually during voice, launcher search, or
notification delivery.

## Product principles

1. macOS native first. Keychain, menu bar behavior, native notifications,
   global shortcuts, and system color sampling should feel at home on macOS.
2. Developer-focused and compact. Favor fast keyboard interaction, high
   information density, direct copy, and predictable behavior.
3. User-controlled interruption. Notification triggers, summaries, voice
   previews, and clipboard capture are explicit preferences.
4. Preserve user configuration. Agent integration edits are previewable,
   merged, and backed up.
5. Keep secrets out of preference files. Credentials belong in Keychain.
6. Make cloud boundaries visible. Voice data goes to the configured provider;
   future mobile content is encrypted before it reaches the relay.

## Success criteria

- Voice works with either tap-toggle or hold-to-talk and pastes into the app
  that was active when recording began.
- Prompt mode produces a useful coding brief without inventing requirements.
- Claude and Codex notifications still work after Samlu restarts.
- A developer can reach any launcher tool from one keyboard-driven surface.
- Optional data collection, especially clipboard history and agent summaries,
  is configurable and understandable.
