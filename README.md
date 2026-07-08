# Samlu

A desktop companion that reacts in real time when your AI coding agents
finish a task or need your input — starting with
[Claude Code](https://code.claude.com). A native notification, and a mascot
that actually notices, instead of a silent terminal you have to keep
tabbing back to check.

Built on Tauri v2 (macOS + Windows), spun out of the
[Kettles](https://kettles.works) project's mascot overlay (same author).

## Why

Long-running agent CLIs leave you babysitting a terminal tab, wondering if
it's still working, stuck waiting for a permission prompt, or done ten
minutes ago. Samlu watches for exactly that and tells you — no polling, no
guessing.

## v1 scope: "Agent Watch"

- **Claude Code only** for now. Other agent CLIs (Codex, Cursor, Windsurf,
  Copilot CLI, Aider, opencode — all of which have their own native hook or
  notification systems) are a clean extension of the same adapter interface,
  not a rewrite — see [Adding a new agent](#adding-a-new-agent) below.
- No AI Q&A overlay yet ("Ask the Kettle" is a planned later phase).
- Click-to-jump to the source terminal is intentionally out of scope for v1
  — focusing the right terminal/IDE window by working directory is
  genuinely OS-specific and nontrivial on both platforms.

## How it works

Claude Code supports an `http` hook type that POSTs a JSON payload to a
local URL on `Notification` and `Stop` events (task needs input / task
finished / an ordinary turn ended). Samlu runs a tiny local HTTP server
(bound to `127.0.0.1` only, on port `47823`, behind a random per-install
token) that receives those POSTs, normalizes them into a generic
`AgentEvent`, and reacts: the mascot animates, and a native notification
fires (debounced for the noisier "ordinary turn ended" signal so you're not
pinged on every back-and-forth).

## Quick start

```bash
# Install Rust if you don't have it: https://rustup.rs
cargo install tauri-cli --version "^2.0.0" --locked

git clone https://github.com/<org>/samlu.git
cd samlu/src-tauri
cargo tauri dev
```

No Node/npm involved anywhere in this repo — the settings window is plain
HTML/CSS/JS, served straight from `public/`.

On first launch, open the **Setup** tab and either let Samlu install the
Claude Code hook automatically (into `~/.claude/settings.json` for all
projects, or a single project's `.claude/settings.json`) or copy the JSON
snippet and add it yourself. Samlu backs up your existing `settings.json`
before writing to it, and only ever appends its own hook entries — any
other hooks you already have configured are left untouched.

## Building a release locally

```bash
./scripts/build-mac.sh          # macOS, Apple Silicon
./scripts/build-windows.ps1     # Windows
```

Both produce an **unsigned** build (no code-signing/notarization budget
spent yet) — you'll see a Gatekeeper or SmartScreen warning on first launch.
Right-click → Open (macOS) or "Run anyway" (Windows), or `xattr -cr` the
`.app` on macOS.

## Adding a new agent

Every agent integration implements one trait in `src-tauri/src/events.rs`:

```rust
pub trait AgentAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn parse(&self, event_route: &str, body: &[u8]) -> Result<AgentEvent, AdapterError>;
}
```

Add a new module under `src-tauri/src/adapters/`, implement the trait
(see `adapters/claude_code.rs` for a complete example), and register it in
`adapters/mod.rs`'s `AdapterRegistry::new()`. Nothing else in the codebase
(`notify.rs`, `pet.rs`, the tray) needs to change — normalization into the
shared `AgentEvent` type is the whole point of the adapter boundary.

## License

Source code is Apache-2.0 — see [`LICENSE`](LICENSE).

The mascot spritesheet art (`public/pet/assets/`) is licensed separately
under CC BY-NC 4.0 — see [`public/pet/assets/LICENSE`](public/pet/assets/LICENSE)
and [`NOTICE`](NOTICE). It is **not** covered by the Apache-2.0 grant above.

## Roadmap

- More agent adapters (Codex, Cursor, Windsurf, Copilot CLI, Aider, opencode).
- "Ask the Kettle" — a global-hotkey BYOK AI Q&A overlay (Claude/Gemini/OpenAI).
- Click-to-jump to the source terminal/IDE window.
- Code-signing and notarization once there's a budget for it.
