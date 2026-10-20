# Agent Sessions, Approvals and Sounds Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Approve/Deny agent permission requests from the Samlu Island, ring a chosen sound for every agent notification, and make View open the exact Warp tab, Claude desktop conversation or Codex thread. Also: a queue, keyboard answers, a live sessions list, a waiting count and a reminder.

**Architecture:** A new `agents` module owns the pure models: session origin, action line, view routing, pending approvals, island queue, sessions registry and sounds. `agents::hub` wires them to the listener, the island, the tray and the launcher. The listener moves to a thread per request, so permission hooks can be held open until the user answers. Hook installers write the new events, carrying `X-Samlu-*` headers that tell Samlu where a session lives.

**Tech Stack:** Rust (Tauri 2, tiny_http, serde_json, toml_edit, chrono), plain JS/CSS webviews. No new crates. Sounds play through `afplay` on macOS and `PlaySoundW` + `waveOutSetVolume` on Windows (windows-sys feature `Win32_Media_Audio`).

**Spec:** `docs/superpowers/specs/2026-10-20-agent-sessions-design.md`

## Global Constraints

- The listener binds `127.0.0.1:47823` only. Every hook route keeps the token as the last path segment.
- Approvals are answered **only** through Tauri commands, never through an HTTP route.
- Every hold has a deadline. Every failure path answers "no decision": `{}` for Claude, an empty body for Codex.
- Claude hold: hook `timeout: 600`, Samlu answers at 590 s. Codex hold: the `codexWaitSecs` setting (default 30, range 10–120). Codex hook `timeout: 130` and curl `--max-time 125`, both fixed.
- Deny message is exactly `Denied from Samlu`. Claude deny also sends `"interrupt": false`.
- No card or sound for SessionStart, SessionEnd, UserPromptSubmit or PostToolUse. They only update sessions and retire cards.
- New persisted fields are `#[serde(default)]`. `event-history.json` from today must still load.
- Hook merges: back up first, preserve everything that isn't Samlu's, preview equals what is written.
- Claude settings: global `~/.claude/settings.json` only.
- Codex: keep Samlu's `notify`. Drop a notify turn-complete if a hook Stop for the same thread arrived in the last 10 s, or arrives within 2 s after.
- Default sounds: macOS needs_input `system:Glass`, completed `system:Hero`, turn_finished `none`. Windows needs_input `system:Windows Exclamation`, completed `system:Windows Notify System Generic`, turn_finished `none`. Volume 70, enabled.
- Shortcut defaults: macOS `Alt+Cmd+Y` / `Alt+Cmd+N`, Windows `Ctrl+Alt+Y` / `Ctrl+Alt+N`. Registered only while an approvable card is in front.
- Reminder choices: 0 (off), 1, 3, 5, 10 minutes. Default 3. At most once per card.
- History stores the action line truncated to 160 characters.
- Sessions in memory only. Pruned after 12 h without an event.
- The Samlu mark is never drawn on. The waiting count is tray **title** text on macOS and **tooltip** text on Windows.
- UI copy: "Approve", "Deny", "Always allow", "View", "Answer in Codex", "Couldn't find this session", "Update needed", "Update".
- Checks: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (run in `src-tauri/`).

## Review Focus

1. **Unset env vars:** Claude interpolates them as `""`, and Windows `cmd.exe` leaves them as `%VAR%`. Both must read as "absent", or View routes to a garbage URL. Test owner: Task 2.
2. **Answered elsewhere:** if the user answers in Warp, the card must go away on the session's next event and never answer a later request with a stale decision. Test owner: Task 3, `retire_session` sends NoDecision only to that session's ids.
3. **Two sessions waiting at once:** a second request queues behind the first. Approving card 1 answers request 1, not the front of some other list. Test owner: Task 3, queue removal by id.
4. **Samlu not running, or restarted mid-hold:** the Codex curl line exits 0 with empty stdout, and Claude treats a connection failure as non-blocking. Test owner: Task 6 asserts `|| true` / `exit /b 0` and `2>/dev/null` in the command strings.
5. **Existing users after upgrade:** today's three-event Claude install must read as `NeedsUpdate`, not `NotInstalled` and not `Installed`. Test owner: Task 5.

---

## Execution waves

- **Wave 0, Task 1** (lead, in the main checkout): the skeleton. Every module, type, signature, command and platform hook from this plan exists, with stub bodies that compile, registered in `lib.rs`. Commit. From here on, no wave-1 task edits `lib.rs`, `Cargo.toml`, `agents/mod.rs` or `agents/hub.rs`.
- **Wave 1, Tasks 2–11:** parallel. Each runs in its own git worktree off the skeleton commit and edits only the files it lists. Use the shared target dir: `export CARGO_TARGET_DIR=/Users/samreshan/Programming/samlu-companion/src-tauri/target`. Commit on the worktree branch.
- **Spike S:** parallel with wave 1, scratch dir only. Verifies the chunked-heartbeat disconnect detection against the Claude CLI.
- **Wave 2, Task 12:** the hub, listener, notify and shortcuts wiring, after all wave-1 branches are merged.
- **Wave 3, Task 13:** live verification. **Task 14:** docs.

---

### Task 1: Skeleton and contracts (lead)

**Files:**
- Create: `src-tauri/src/agents/{mod.rs,origin.rs,action.rs,view.rs,approvals.rs,island_queue.rs,sessions.rs,sounds.rs,shortcuts.rs,hub.rs}`, `src-tauri/src/adapters/codex_hooks.rs`, `src-tauri/src/setup/codex_hooks.rs`
- Modify: `src-tauri/src/lib.rs` (mod + every new command), `src-tauri/src/adapters/mod.rs` (parse returns `HookMessage`, takes `&SessionOrigin`), `src-tauri/src/events.rs` (`origin`, `outcome` fields), `src-tauri/src/settings.rs` (struct + stub commands), `src-tauri/src/macos.rs`, `src-tauri/src/windows/mod.rs`, `src-tauri/src/other_platform.rs` (sound + activate stubs), `src-tauri/Cargo.toml` (windows-sys `Win32_Media_Audio`)

**Interfaces (Produces): the contract every later task codes against.**

```rust
// agents/origin.rs
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionOrigin {
    pub entrypoint: Option<String>,      // X-Samlu-Entrypoint
    pub host_session: Option<String>,    // X-Samlu-Host-Session
    pub warp_focus_url: Option<String>,  // X-Samlu-Warp-Focus
    pub term_program: Option<String>,    // X-Samlu-Term
    pub bundle_id: Option<String>,       // X-Samlu-Bundle
    pub originator: Option<String>,      // X-Samlu-Originator
}
impl SessionOrigin {
    pub fn from_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self;
    /// Fields present in `newer` replace ours; absent ones keep ours.
    pub fn merge(&mut self, newer: &SessionOrigin);
}

// agents/action.rs
pub fn action_line(tool_name: &str, tool_input: &serde_json::Value) -> String;
pub fn approval_title(tool_name: &str) -> &'static str; // "Wants to run" | "Wants to edit" | "Needs approval"

// agents/mod.rs
pub enum HookMessage { Event(AgentEvent), Approval(ApprovalRequest), Signal(SessionSignal) }
#[derive(Debug, Clone, Serialize)] #[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub agent: String, pub session_id: String, pub project_path: String,
    pub tool_name: String, pub action: String,
    /// Claude only: the first `addRules` + `behavior: "allow"` permission suggestion, verbatim.
    pub always_allow: Option<serde_json::Value>,
    pub origin: SessionOrigin,
}
#[derive(Debug, Clone)]
pub struct SessionSignal { pub agent: String, pub session_id: String, pub project_path: String, pub kind: SignalKind, pub origin: SessionOrigin }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind { Started, Ended, Working, ToolFinished }
pub fn agent_label(agent: &str) -> String;   // moved from AgentEvent::agent_label, which now calls it
pub fn project_label(path: &str) -> String;  // moved from AgentEvent::project_label

// agents/view.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewTarget { Url(String), App(String), NotFound }
pub fn resolve(agent: &str, session_id: &str, origin: &SessionOrigin) -> ViewTarget;
pub fn open(target: &ViewTarget) -> Result<(), String>;

// agents/approvals.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision { Allow, AllowAlways, Deny, NoDecision }
pub struct PendingApprovals;  // Mutex inside
impl PendingApprovals {
    pub fn new() -> Self;
    pub fn open(&self, request: ApprovalRequest) -> (String, std::sync::mpsc::Receiver<Decision>);
    pub fn answer(&self, request_id: &str, decision: Decision) -> Option<ApprovalRequest>;
    pub fn retire_session(&self, agent: &str, session_id: &str) -> Vec<String>;
    pub fn get(&self, request_id: &str) -> Option<ApprovalRequest>;
    pub fn len(&self) -> usize;
}
pub fn response_body(agent: &str, decision: Decision, request: &ApprovalRequest) -> Vec<u8>;

// agents/island_queue.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum CardKind { Approval, NeedsInput, HandedBack }
#[derive(Debug, Clone, Serialize)] #[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String, pub kind: CardKind,
    pub agent: String, pub agent_label: String, pub session_id: String,
    pub project: String, pub title: String, pub detail: String,
    pub request_id: Option<String>, pub always_allow_rule: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Debug, Clone, Serialize)] #[serde(rename_all = "camelCase")]
pub struct QueueSnapshot { pub cards: Vec<Card>, pub front: usize }
pub struct IslandQueue;
impl IslandQueue {
    pub fn new() -> Self;
    pub fn push(&mut self, card: Card);
    pub fn remove(&mut self, card_id: &str) -> Option<Card>;
    pub fn remove_by_request(&mut self, request_id: &str) -> Option<Card>;
    pub fn remove_session(&mut self, agent: &str, session_id: &str) -> Vec<Card>;
    pub fn replace(&mut self, card_id: &str, card: Card) -> bool;
    pub fn front(&self) -> Option<&Card>;
    pub fn step(&mut self, delta: i32);
    pub fn is_empty(&self) -> bool;
    pub fn snapshot(&self) -> QueueSnapshot;
}

// agents/sessions.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum SessionState { Working, Waiting, Idle }
#[derive(Debug, Clone, Serialize)] #[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent: String, pub agent_label: String, pub session_id: String,
    pub project_path: String, pub project_label: String, pub state: SessionState,
    pub origin: SessionOrigin, pub started_at: DateTime<Utc>, pub last_event_at: DateTime<Utc>,
}
pub struct Sessions;
impl Sessions {
    pub fn new() -> Self;
    pub fn touch(&self, agent: &str, session_id: &str, project_path: &str, origin: &SessionOrigin, state: Option<SessionState>, now: DateTime<Utc>) -> bool;
    pub fn end(&self, agent: &str, session_id: &str) -> bool;
    pub fn get(&self, agent: &str, session_id: &str) -> Option<SessionInfo>;
    pub fn list(&self) -> Vec<SessionInfo>;      // waiting first, then most recent event
    pub fn waiting_count(&self) -> usize;
    pub fn prune(&self, now: DateTime<Utc>) -> bool; // drops sessions silent > 12 h
}

// agents/sounds.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind { NeedsInput, Completed, TurnFinished }
pub fn system_sounds() -> Vec<String>;                       // display names, sorted
pub fn resolve(choice: &str, app_data_dir: &Path) -> Option<PathBuf>; // "none" | "system:<Name>" | "custom:<file>"
pub fn import_custom(source: &Path, app_data_dir: &Path) -> Result<String, String>; // returns "custom:<file>"
pub fn play_choice(choice: &str, volume: u8, app_data_dir: &Path);   // no-op on none/missing

// agents/shortcuts.rs
pub fn arm(app: &AppHandle, approve: &str, deny: &str) -> Result<(), String>;
pub fn disarm(app: &AppHandle);
pub fn is_armed() -> bool;

// agents/hub.rs — Tauri commands (stubs in Task 1, real in Task 12)
#[tauri::command] pub fn agent_card_action(app: AppHandle, card_id: String, action: String) -> Result<(), String>; // approve|always|deny|view|dismiss
#[tauri::command] pub fn agent_queue_step(app: AppHandle, delta: i32);
#[tauri::command] pub fn agent_sessions(app: AppHandle) -> Vec<SessionInfo>;
#[tauri::command] pub fn agent_session_view(app: AppHandle, agent: String, session_id: String) -> Result<(), String>;
#[tauri::command] pub fn agent_sound_catalog() -> serde_json::Value;          // {"system": [names]}
#[tauri::command] pub fn agent_sound_preview(app: AppHandle, choice: String);
#[tauri::command] pub async fn agent_sound_choose_custom(app: AppHandle) -> Result<Option<String>, String>;
pub fn view_session(app: &AppHandle, agent: &str, session_id: &str); // used by tray + agent_session_view
pub struct AgentHub; // managed as Arc<AgentHub>
impl AgentHub {
    pub fn new(app_data_dir: &Path) -> Self;
    pub fn handle(&self, app: &AppHandle, message: HookMessage);
    /// Opens a held approval and returns (request id, receiver, hold).
    pub fn begin_approval(&self, app: &AppHandle, request: ApprovalRequest) -> (String, Receiver<Decision>, Duration);
    pub fn hold_elapsed(&self, app: &AppHandle, request_id: &str);
    pub fn client_gone(&self, app: &AppHandle, request_id: &str);
}

// settings.rs
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)] #[serde(default, rename_all = "camelCase")]
pub struct AgentAlertSettings {
    pub sounds_enabled: bool, pub volume: u8,
    pub sound_needs_input: String, pub sound_completed: String, pub sound_turn_finished: String,
    pub codex_wait_secs: u64, pub approve_shortcut: String, pub deny_shortcut: String, pub reminder_mins: u64,
}
impl AppConfig { pub fn agent_alerts(&self) -> AgentAlertSettings; pub fn set_agent_alerts(&self, s: AgentAlertSettings) -> AgentAlertSettings; }
#[tauri::command] pub fn get_agent_alert_settings(..) -> AgentAlertSettings;
#[tauri::command] pub fn set_agent_alert_settings(app: AppHandle, settings: AgentAlertSettings, ..) -> AgentAlertSettings;

// adapters/mod.rs
impl AdapterRegistry { pub fn parse(&self, adapter: &str, route: &str, body: &[u8], origin: &SessionOrigin) -> Result<HookMessage, AdapterError>; }
// AgentAdapter::parse gains `origin: &SessionOrigin` and returns HookMessage.

// events.rs: AgentEvent gains  #[serde(default)] pub origin: SessionOrigin,  #[serde(default)] pub outcome: Option<String>

// setup/claude_hooks.rs & setup/codex_hooks.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum InstallState { NotInstalled, NeedsUpdate, Installed }   // lives in setup/mod.rs

// platform (macos.rs / windows/mod.rs / other_platform.rs)
pub fn play_sound(path: &std::path::Path, volume: u8);
pub fn system_sound_dir() -> Option<std::path::PathBuf>;
pub fn activate_bundle(bundle_id: &str) -> bool;   // macOS `open -b`; false elsewhere
pub fn set_tray_count(app: &AppHandle, count: usize); // macOS tray title; Windows tooltip

// tray.rs (stub here, real in Task 9)
pub fn refresh_agents(app: &AppHandle, sessions: &[crate::agents::sessions::SessionInfo]);
```

- [ ] **Step 1:** Create every file above with these exact signatures. Bodies are minimal stubs (`Default`, empty `Vec`, `ViewTarget::NotFound`, `Ok(())`). Add `#![allow(dead_code)]` at the top of `agents/mod.rs` for now (removed in Task 12). The existing adapters return `HookMessage::Event(..)` and stamp `origin`.
- [ ] **Step 2:** Register all new commands in `lib.rs` `generate_handler!`. `app.manage(Arc::new(agents::hub::AgentHub::new(&app_data_dir)))`. Pass `SessionOrigin::default()` where the listener calls `parse`.
- [ ] **Step 3:** Run `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test` in `src-tauri/`. Expected: all pass (the existing tests).
- [ ] **Step 4:** Commit `agents: skeleton and contracts for sessions, approvals and sounds`.

---

### Task 2: Origin, action line, view routing (Haiku)

**Files:** Modify `agents/origin.rs`, `agents/action.rs`, `agents/view.rs`, plus `activate_bundle` in `macos.rs`. Tests inline.

**Interfaces:** Produces what Task 1 declares. Consumes `crate::open_path(&str)`.

- [ ] **Step 1: Failing tests**

```rust
// origin.rs
#[test] fn reads_samlu_headers_case_insensitively() {
    let o = SessionOrigin::from_headers([("x-samlu-warp-focus","warp://session/abc"),("X-Samlu-Entrypoint","cli"),("X-Samlu-Term","WarpTerminal")]);
    assert_eq!(o.warp_focus_url.as_deref(), Some("warp://session/abc"));
    assert_eq!(o.entrypoint.as_deref(), Some("cli"));
}
#[test] fn empty_and_unexpanded_values_are_absent() {
    let o = SessionOrigin::from_headers([("X-Samlu-Warp-Focus",""),("X-Samlu-Bundle","%__CFBundleIdentifier%"),("X-Samlu-Term","  ")]);
    assert_eq!(o, SessionOrigin::default());
}
#[test] fn merge_keeps_known_fields() { /* old has host_session, newer has only term → both present */ }
// action.rs
#[test] fn bash_uses_command() { assert_eq!(action_line("Bash", &json!({"command":"curl -sI https://example.com","description":"x"})), "curl -sI https://example.com"); }
#[test] fn edit_uses_file_path() { assert_eq!(action_line("Edit", &json!({"file_path":"/p/src/app.ts"})), "/p/src/app.ts"); }
#[test] fn multiline_command_is_one_line() { assert_eq!(action_line("Bash", &json!({"command":"a\n  b"})), "a b"); }
#[test] fn falls_back_to_description_then_tool() { assert_eq!(action_line("WebFetch", &json!({"url":"https://x.dev"})), "https://x.dev"); assert_eq!(action_line("mcp__x__y", &json!({})), "mcp__x__y"); }
#[test] fn titles() { assert_eq!(approval_title("Bash"), "Wants to run"); assert_eq!(approval_title("Write"), "Wants to edit"); assert_eq!(approval_title("WebFetch"), "Needs approval"); }
// view.rs
#[test] fn warp_wins() { /* origin.warp_focus_url = warp://session/1, entrypoint claude-desktop → Url("warp://session/1") */ }
#[test] fn claude_desktop_link() { /* entrypoint claude-desktop, host local_ab-1 → Url("claude://code/continue?session=local_ab-1") */ }
#[test] fn rejects_bad_host_session() { /* host "local_a b" or "x" → not a claude link */ }
#[test] fn codex_app_by_bundle_or_originator_or_absence() { /* bundle com.openai.codex → Url("codex://threads/T"); originator "Codex Desktop" → same; codex with no warp & no term → same; codex with term "WarpTerminal" and no warp → App("dev.warp.Warp-Stable") */ }
#[test] fn term_program_maps_to_app() { /* Apple_Terminal→com.apple.Terminal, iTerm.app→com.googlecode.iterm2, vscode→com.microsoft.VSCode, ghostty→com.mitchellh.ghostty, WarpTerminal→dev.warp.Warp-Stable, WezTerm→com.github.wez.wezterm */ }
#[test] fn nothing_known_is_not_found() { assert_eq!(resolve("claude-code","s",&SessionOrigin::default()), ViewTarget::NotFound); }
```

- [ ] **Step 2:** `cargo test agents::` fails.
- [ ] **Step 3:** Implement.
  - Header names match case-insensitively.
  - Values are trimmed. Empty values, and values that start and end with `%`, are None.
  - `action_line` field order: `command`, `file_path`, `notebook_path`, `url`, `pattern`, `path`, `description`, then the tool name. Whitespace runs collapse to one space. Truncate to 300 chars with `…`.
  - Titles: `Bash`/`exec`/`shell`/`local_shell` give "Wants to run"; `Edit`/`Write`/`MultiEdit`/`NotebookEdit`/`apply_patch` give "Wants to edit".
  - Session ids in URLs are percent-encoded. A host session must match `^local_[A-Za-z0-9-]{1,64}$`.
  - `open`: `Url` → `crate::open_path`, `App` → `platform::activate_bundle`, `NotFound` → `Err("Couldn't find this session")`.
  - macOS `activate_bundle` runs `/usr/bin/open -b <id>`.
- [ ] **Step 4:** Tests pass, clippy clean. Commit `agents: session origin, action line and view routing`.

### Task 3: Pending approvals and island queue (Sonnet)

**Files:** Modify `agents/approvals.rs`, `agents/island_queue.rs`. Tests inline.

- [ ] **Step 1: Failing tests**

```rust
#[test] fn answer_delivers_once() { let p = PendingApprovals::new(); let (id, rx) = p.open(req("claude-code","s1")); assert!(p.answer(&id, Decision::Allow).is_some()); assert_eq!(rx.recv().unwrap(), Decision::Allow); assert!(p.answer(&id, Decision::Deny).is_none()); }
#[test] fn retire_session_only_touches_that_session() { /* open s1 twice and s2 once; retire s1 → 2 ids; both s1 receivers get NoDecision; s2 still pending (len()==1) */ }
#[test] fn claude_allow_body() { assert_eq!(json(response_body("claude-code", Decision::Allow, &r)), json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}})); }
#[test] fn claude_always_body_carries_suggestion() { /* r.always_allow = Some(s) → decision.updatedPermissions == [s] */ }
#[test] fn claude_deny_body() { /* decision == {"behavior":"deny","message":"Denied from Samlu","interrupt":false} */ }
#[test] fn codex_deny_body_has_no_interrupt() { /* decision == {"behavior":"deny","message":"Denied from Samlu"} */ }
#[test] fn codex_always_falls_back_to_allow() { /* AllowAlways for codex → plain allow */ }
#[test] fn no_decision_bodies() { assert_eq!(response_body("claude-code", Decision::NoDecision, &r), b"{}"); assert!(response_body("codex", Decision::NoDecision, &r).is_empty()); }
// island_queue.rs
#[test] fn push_keeps_oldest_first_and_front_stays_put() { /* push a, b; front a; push c; front still a */ }
#[test] fn remove_front_shows_next() { /* a,b,c front a; remove a → front b */ }
#[test] fn remove_before_front_keeps_same_card_in_front() { /* a,b,c step(+1) front b; remove a → front b */ }
#[test] fn step_wraps() { /* a,b; step(+1) → b; step(+1) → a; step(-1) → b */ }
#[test] fn remove_session_and_by_request() { /* … */ }
#[test] fn snapshot_front_index_clamped_when_empty() { assert_eq!(IslandQueue::new().snapshot().front, 0); }
```

- [ ] **Step 2:** Tests fail.
- [ ] **Step 3:** Implement. Request ids: `format!("req-{}", counter)` plus 6 random hex chars (`rand`). The channel is `mpsc::sync_channel(1)`; sending to a dropped receiver is ignored. `answer` removes before sending.
- [ ] **Step 4:** Tests pass. Commit `agents: pending approvals and island queue`.

### Task 4: Sessions registry, sounds and platform playback (Haiku)

**Files:** Modify `agents/sessions.rs`, `agents/sounds.rs`, plus `play_sound`, `system_sound_dir` and `set_tray_count` in `macos.rs`, `windows/mod.rs` and `other_platform.rs`. Tests inline.

- [ ] **Step 1: Failing tests**

```rust
#[test] fn touch_registers_and_updates_state() { /* touch(..., Some(Working)) → list()[0].state==Working; touch(..., Some(Waiting)) → Waiting; touch(..., None) keeps Waiting */ }
#[test] fn waiting_sorted_first_then_recent() { }
#[test] fn waiting_count_counts_sessions() { }
#[test] fn end_removes() { }
#[test] fn prune_drops_after_12h() { /* last_event 12h01m ago → removed; 11h59m → kept */ }
#[test] fn origin_merges_on_touch() { /* second touch with only term keeps host_session */ }
// sounds.rs
#[test] fn resolve_none_and_unknown() { assert!(resolve("none", dir).is_none()); assert!(resolve("bogus", dir).is_none()); }
#[test] fn resolve_custom_inside_sounds_dir_only() { /* "custom:../x.wav" → None; "custom:a.wav" existing → Some(dir/sounds/a.wav) */ }
#[test] fn import_custom_copies_and_dedupes_name() { /* importing ding.aiff twice → custom:ding.aiff, custom:ding-2.aiff; files exist */ }
#[test] fn import_rejects_unsupported_extension() { /* macOS: aiff aif wav mp3 m4a caf ok; Windows: wav only; .txt → Err */ }
```

- [ ] **Step 2:** Tests fail.
- [ ] **Step 3:** Implement.
  - `system:<Name>` resolves to `system_sound_dir()/<Name>.aiff` on macOS (`/System/Library/Sounds`) and `.wav` on Windows (`%WINDIR%\Media`).
  - `play_choice` spawns a thread and never blocks the caller.
  - macOS `play_sound`: `/usr/bin/afplay -v <volume/100> <path>`.
  - Windows `play_sound`: `waveOutSetVolume(0, v | v << 16)` with `v = volume * 0xFFFF / 100`, then `PlaySoundW(path, 0, SND_FILENAME | SND_ASYNC | SND_NODEFAULT)`.
  - `set_tray_count`: macOS `tray_by_id("main-tray").set_title(if n>0 {Some(n.to_string())} else {None})`. Windows sets the tooltip to `Samlu · N agent(s) waiting`, or `Samlu` when zero.
  - Linux: all no-ops.
- [ ] **Step 4:** Tests pass. Commit `agents: sessions registry and sounds`.

### Task 5: Claude hooks installer and adapter (Sonnet)

**Files:** Modify `setup/claude_hooks.rs`, `adapters/claude_code.rs`. Tests inline.

Events → routes (exact): `SessionStart→session-start`, `SessionEnd→session-end`, `UserPromptSubmit→prompt`, `PermissionRequest→permission`, `PostToolUse→post-tool`, `Notification→notification`, `Stop→stop`, `TaskCompleted→task-completed`. Each Samlu handler:

```json
{ "type": "http", "url": "http://127.0.0.1:47823/hooks/claude-code/<route>/<token>", "timeout": 5,
  "headers": { "X-Samlu-Entrypoint": "$CLAUDE_CODE_ENTRYPOINT", "X-Samlu-Host-Session": "$CLAUDE_CODE_HOST_SESSION_ID",
               "X-Samlu-Warp-Focus": "$WARP_FOCUS_URL", "X-Samlu-Term": "$TERM_PROGRAM" },
  "allowedEnvVars": ["CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_HOST_SESSION_ID", "WARP_FOCUS_URL", "TERM_PROGRAM"] }
```
PermissionRequest uses `"timeout": 600`.

- [ ] **Step 1: Failing tests**
  - `status_of_old_three_event_install_is_needs_update`: today's Notification/Stop/TaskCompleted install with the current token gives `InstallState::NeedsUpdate`.
  - `status_full_install_is_installed`: `merge_hooks(&json!({}), …)` gives `Installed`.
  - `missing_headers_is_needs_update`.
  - `permission_timeout_600`.
  - `merge_replaces_stale_and_keeps_foreign_handlers`: the existing test, extended to the 8 events.
  - `uninstall_removes_all_eight`.
  - Adapter: `permission_route_builds_approval`, using the captured live payload (Bash, `curl -sI https://example.com`, `permission_suggestions` addRules). Expect `Approval` with action `curl -sI https://example.com` and `always_allow == Some(suggestion)`.
  - `permission_without_allow_suggestion_has_no_always`.
  - `session_routes_are_signals`: session-start → Started, session-end → Ended, prompt → Working, post-tool → ToolFinished. Origin is passed through.
  - `stop_and_notification_carry_origin`.
- [ ] **Step 2:** Tests fail.
- [ ] **Step 3:** Implement.
  - Add `pub fn status(value: &Value, port: u16, token: &str) -> InstallState`.
  - `is_installed` becomes `status == Installed`.
  - "Samlu present at all" means any handler URL contains `/hooks/claude-code/`.
  - SubagentStop stays ignored.
  - Subagent PermissionRequests (`agent_id` present) are still approvals; the user must answer them.
- [ ] **Step 4:** Tests pass. Commit `claude: install session, permission and tool hooks with origin headers`.

### Task 6: Codex hooks installer, adapter, integrations status (Sonnet)

**Files:** Modify `setup/codex_hooks.rs`, `adapters/codex_hooks.rs`, `setup/integrations.rs`, `setup/gemini_cli.rs`, `setup/antigravity.rs`. Tests inline.

Codex events → routes: `SessionStart→session-start`, `SessionEnd→session-end`, `UserPromptSubmit→prompt`, `PermissionRequest→permission`, `PostToolUse→post-tool`, `Stop→stop`. TOML shape, one per event, no matcher:

```toml
[[hooks.PermissionRequest]]
[[hooks.PermissionRequest.hooks]]
type = "command"
command = "<curl line>"
timeout = 130
```
Other events use `timeout = 5`. The macOS curl line for `permission`:

```
/usr/bin/curl --silent --max-time 125 -X POST -H 'Content-Type: application/json' -H "X-Samlu-Warp-Focus: $WARP_FOCUS_URL" -H "X-Samlu-Term: $TERM_PROGRAM" -H "X-Samlu-Bundle: $__CFBundleIdentifier" -H "X-Samlu-Originator: $CODEX_INTERNAL_ORIGINATOR_OVERRIDE" --data-binary @- 'http://127.0.0.1:47823/hooks/codex/permission/<token>' 2>/dev/null || true
```

Other routes: `--max-time 3` and `>/dev/null 2>&1 || true`. On Windows: `curl.exe`, `%VAR%` expansion, `"` quoting, `2>NUL || exit /b 0` (permission) and `>NUL 2>&1 || exit /b 0` (others), following `gemini_cli.rs`.

- [ ] **Step 1: Failing tests**
  - `merge_into_existing_config_preserves_comments_and_keys`: `model = "gpt-5"` plus a comment survive; the six events are present.
  - `merge_replaces_stale_samlu_entries_only`: a foreign `[[hooks.Stop]]` survives; a Samlu one with an old token is replaced.
  - `status_states`: empty → NotInstalled, partial → NeedsUpdate, full → Installed.
  - `uninstall_removes_only_samlu`.
  - `permission_command_prints_body_and_never_fails`: contains `--max-time 125`, `|| true`, `2>/dev/null`, and not `>/dev/null 2>&1`.
  - `other_commands_discard_output`.
  - Adapter: `permission_payload` (`{"session_id":"t1","turn_id":"u","cwd":"/p","hook_event_name":"PermissionRequest","tool_name":"exec","tool_input":{"command":"ls","description":null}}`) gives an Approval with action `ls`, `always_allow` None.
  - `stop_is_completed_event`, with summary from `last_assistant_message`.
  - Session routes are signals.
  - Integrations: `IntegrationStatus` gains `needs_update: bool`, computed from `InstallState`. The Codex row is `installed` when the hooks are Installed. Apply writes the hooks **and** keeps/ensures `notify`. Remove deletes both.
  - Gemini/Antigravity curl lines gain the four `-H "X-Samlu-*"` headers. Update their existing tests.
- [ ] **Step 2:** Tests fail.
- [ ] **Step 3:** Implement with `toml_edit`: `ArrayOfTables` under `hooks.<Event>`. Samlu entries are recognised by `/hooks/codex/` in `command`. Backups go to `hook-backups/codex-config.<stamp>.toml.bak`, as `codex_notify.rs` does. `preview` returns the merged document.
- [ ] **Step 4:** Tests pass. Commit `codex: install lifecycle and permission hooks`.

### Task 7: Alert settings (Haiku)

**Files:** Modify `settings.rs`. Tests inline.

- [ ] **Step 1: Failing tests**
  - `agent_alert_defaults`: platform defaults from Global Constraints; volume 70; codex 30; reminder 3.
  - `clamps_and_normalizes`: volume 150 → 100; codex 5 → 10, 500 → 120; reminder 4 → 3 (nearest allowed of 0/1/3/5/10, ties go lower); an empty shortcut keeps the previous value; a sound choice not matching `none|system:*|custom:*` keeps the previous value.
  - `old_config_without_alerts_loads_defaults`.
- [ ] **Step 2–4:** Implement as a `#[serde(default)] agent_alerts: AgentAlertSettings` field inside `Data`. `set_agent_alerts` returns the stored (normalized) value and saves. `set_agent_alert_settings` also emits `agents://alerts-changed` with the new value. Commit `settings: agent alert preferences`.

### Task 8: Island approval cards and queue UI (Sonnet)

**Files:** Modify `public/agent-island/island.html`, `island.js`, `island.css`.

**Contract:** the `island://show` payload keeps today's fields and adds `queue: { cards: Card[], front: number } | null`. `Card` is serialized as in Task 1 (camelCase; `kind` is `approval` | `needs_input` | `handed_back`). When `queue` is non-null and non-empty, the island renders `cards[front]` and stays persistent. Otherwise it behaves as today. The window is **404 × 176** CSS px inside a 420 × 184 window. Task 12 sets the window size.

- Approval card:
  - Meta: title, agent · project.
  - Action line: monospaced, one line, ellipsis, full text in `title`.
  - Buttons, in this order: **Deny** (secondary), **Always allow** (only when `alwaysAllowRule`; its `title` is the rule), **Approve** (primary), **View** (text button).
- `needs_input`: View + Dismiss.
- `handed_back`: detail "Answer in Codex", View + Dismiss.
- More than one card: "1 of N" with ‹ › buttons calling `invoke("agent_queue_step", { delta: ±1 })`.
- Buttons call `invoke("agent_card_action", { cardId, action })` with `approve|always|deny|view|dismiss`.
- Keep the arrive/swap motion: a card change uses `swap()`.
- After an action, the island waits for the next payload; it doesn't hide itself. A payload with an empty queue and `kind: "dismiss"` plays the exit.
- Bloom and Buddy both get the layout. Light and dark use the existing tokens in `presence.css`. Respect reduced motion.
- [ ] **Step 1:** Implement. Add a `?demo=approval` query handler that renders a static two-card queue, for manual checking.
- [ ] **Step 2:** Verify with `node --check public/agent-island/island.js`, then open `island.html?demo=approval` in the browser pane and read the accessibility tree: the buttons exist and are labelled.
- [ ] **Step 3:** Commit `island: approval cards and queue`.

### Task 9: Tray agents section (Haiku)

**Files:** Modify `tray.rs`.

**Interfaces:** Produces `pub fn refresh_agents(app: &AppHandle, sessions: &[crate::agents::sessions::SessionInfo])`. Rebuilds the tray menu with an "Agents" header item (disabled), one item per session (max 8) labelled `"{agent_label} — {project_label} · {Waiting|Working|Idle}"` with id `session|{agent}|{session_id}`, a separator, then the existing items. No sessions → no Agents section. A menu event with a `session|` prefix calls `crate::agents::hub::view_session(app, agent, session_id)` (declared in Task 1 as `pub fn view_session(app: &AppHandle, agent: &str, session_id: &str)`). Also calls `crate::platform::set_tray_count(app, waiting)`.

- [ ] **Step 1:** Test `session_menu_id_round_trips`. Make `fn menu_id(agent, session_id) -> String` and `fn parse_menu_id(&str) -> Option<(String, String)>` pure and test them (ids containing `|` in session id: split on the first two `|` only).
- [ ] **Step 2:** Implement, keeping `UpdaterState::set_menu_item` working after a rebuild (store the new update item).
- [ ] **Step 3:** Tests and clippy. Commit `tray: agents section and waiting count`.

### Task 10: Launcher `#` sessions mode (Haiku)

**Files:** Modify `public/launcher/launcher.js` (and `launcher.css` only if needed).

- `SESSIONS_PREFIX = "#"`, mirroring `TASKS_PREFIX`. `invoke("agent_sessions")` gives `SessionInfo[]` (camelCase). Rows: `{ id: "session:"+agent+":"+sessionId, kind: "agent_session", section: "Sessions", title: agentLabel + " — " + projectLabel, subtitle: stateLabel + " · " + relative(lastEventAt), target: sessionId, agent }`. The accessory is a status mark: waiting = warn, working = running, idle = done (reuse `statusMark` styles).
- Return → `hide()` then `invoke("agent_session_view", { agent, sessionId })`.
- Filter by agent/project text after `#`. Empty → "No agent sessions yet".
- Listen to `agents://sessions` and refresh while in `#` mode.
- The hint row mentions `#` alongside the existing prefixes, if the launcher lists prefixes.
- [ ] Verify with `node --check public/launcher/launcher.js`. Commit `launcher: # lists agent sessions`.

### Task 11: Settings Agents pane (Sonnet)

**Files:** Modify `public/index.html` (Agents pane), `public/settings.js`, `public/settings.css`.

- **Sounds** group:
  - Master switch `agent-sounds-enabled`.
  - Rows "Needs you", "Task completed", "Every finished turn", each with a select (None, the `agent_sound_catalog().system` names as `system:<name>`, the current custom choice if any, and "Choose File…") and a ▶ preview button (`agent_sound_preview`). "Choose File…" calls `agent_sound_choose_custom`; on `null`, revert the select.
  - Volume range 0–100.
- **Approvals** group: "Codex waits for you" number (10–120 s); "Approve shortcut" and "Deny shortcut" via `createShortcutRecorder`; "Remind me" select (Off, 1, 3, 5, 10 min).
- Save through `set_agent_alert_settings` with the existing saved-indicator pattern. Load via `get_agent_alert_settings` in `loadPane("agents")`.
- **Sessions** group, above Recent Activity: `agent_sessions()` rows (dot by state, agent — project, state · time). Clicking a row calls `agent_session_view`. Refresh on `agents://sessions`.
- Connections rows: `needsUpdate` shows pill "Update needed" (warn) and primary button "Update" (applies). Codex row note when installed: "Codex asks once to trust Samlu's hooks."
- [ ] Verify with `node --check public/settings.js`, then the browser-pane accessibility tree of `index.html` shows the new controls. Commit `settings: sounds, approvals and sessions`.

### Spike S: Heartbeat disconnect detection (Sonnet, scratch only)

- [ ] Scratch HTTP server: on POST it sends `HTTP/1.1 200`, `Transfer-Encoding: chunked`, `Content-Type: application/json`, then a `" "` chunk every 2 s. It logs write failures. Mode A: after 6 s, send the final chunk `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}` then `0\r\n\r\n`. Mode B: never decide.
- [ ] Use the `expect` harness from the design session (claude `--permission-mode default --settings <tmp> --model haiku`, in the trusted samlu-companion folder, command `curl -sI https://example.com`, PermissionRequest **http** hook to the server). Mode A: the command ran without a dialog answer (check output for headers). Mode B: press Esc at the dialog. Does the server log a write failure within 4 s?
- [ ] Report: mode A works yes/no, mode B detects disconnect yes/no and latency. Delete scratch files.

### Task 12: Hub, listener, notify, shortcuts wiring (Sonnet, after merges)

**Files:** Modify `agents/hub.rs`, `agents/shortcuts.rs`, `agents/mod.rs` (remove `allow(dead_code)`), `server/mod.rs`, `notify.rs`, `lib.rs` (suspend/resume, Codex notify dedupe, sessions prune timer), `state.rs` if needed.

Behaviour (each line pinned by a test where pure):

- **Listener:**
  - `incoming_requests()` → `thread::spawn` per request.
  - Origin from headers: `request.headers()` → `SessionOrigin::from_headers`.
  - `HookMessage::Approval` → `hub.begin_approval`, then `rx.recv_timeout(hold)`. On timeout → `hub.hold_elapsed`, answer NoDecision. Write `response_body(agent, decision, &req)` with status 200.
  - If Spike S succeeded, Claude approvals use the chunked heartbeat, and a write failure → `hub.client_gone`. Otherwise a single response.
  - Other messages → `hub.handle`, 204.
- **Hold:** Claude 590 s; Codex `agent_alerts().codex_wait_secs`.
- **`handle(Event)`:**
  1. Record history (approval outcomes are set on the stored event).
  2. Sessions touch: `NeedsInput` → Waiting, `Completed`/`TurnFinished` → Idle.
  3. Retire the session's pending approvals and its Approval/HandedBack cards (`retire_session` + `remove_session`). Exception: an incoming `permission_prompt` Notification doesn't retire.
  4. Duplicate folding: a Claude `NeedsInput` whose summary is "Waiting for permission to continue" is held 1.5 s and dropped if an approval for that session exists by then.
  5. Notify as today, plus the sound for its kind when `sounds_enabled`.
  6. A `NeedsInput` that is shown also pushes a `needs_input` card.
- **`handle(Signal)`:**
  - Started → touch(Idle).
  - Working → touch(Working).
  - ToolFinished → touch(Working) and retire, as above.
  - Ended → end, plus remove that session's cards.
  - No sound.
- **`begin_approval`:**
  - `approvals.open`; push an `approval` card (title `approval_title`, detail = action, `always_allow_rule` = `"{toolName}({ruleContent})"` from the suggestion's first rule); session Waiting.
  - Play the needs_input sound.
  - Notification Center entry "{Agent} needs approval" with the action as body.
  - Island shown regardless of delivery; if the island can't be created → `answer(NoDecision)` immediately.
  - Arm shortcuts if this card is in front.
  - Schedule the reminder (if `reminder_mins > 0`): after the delay, if the card still exists and isn't reminded → play needs_input once.
  - Emit `agents://sessions`; refresh tray.
- **`agent_card_action`:**
  - approve/always/deny → `approvals.answer`, remove the card, record the outcome ("approved in Samlu" / "denied in Samlu").
  - view → for a Codex approval, first `answer(NoDecision)` and replace the card with `handed_back`; Claude cards stay. Then `view::open(view::resolve(..))`. On `NotFound`, emit `island://note` "Couldn't find this session".
  - dismiss → remove the card; a dismissed approval answers NoDecision.
  - After any change: re-emit the queue to the island (a `dismiss` payload when empty), re-arm or disarm shortcuts, update sessions, tray.
- **`hold_elapsed`:** Codex → replace the card with `handed_back` (session stays Waiting; the reminder stays). Claude → remove the card.
- **`client_gone`:** remove the card silently, outcome "answered in the agent".
- **Shortcuts:**
  - `arm` registers both chords with `global_shortcut().on_shortcut`. Pressed → `agent_card_action(front card, approve|deny)`.
  - `disarm` unregisters exactly those chords.
  - `suspend_global_shortcuts` disarms.
  - `resume_global_shortcuts` re-arms when the front card is an approval.
  - `set_agent_alert_settings` re-arms with the new chords.
- **Codex notify dedupe:** a `--codex-notify` event is dropped if the hub saw a Codex hook Stop for that thread ≤ 10 s ago, or is delayed 2 s and dropped if one arrives.
- **notify.rs:**
  - Window 420 × 184.
  - `IslandPayload.queue: Option<QueueSnapshot>`.
  - `agent_island_open_activity` is replaced by card actions. Keep the command, but route it to `view` for the shown event's session.
  - Completion/turn notices don't replace a non-empty queue (they still ring and go to Notification Center).
- **Prune timer:** every 10 min, `sessions.prune(now)`, then emit + tray if changed.
- [ ] **Step 1:** Unit tests for the pure parts (dedupe window, `always_allow_rule` formatting, outcome strings). Add an integration test in `server/mod.rs` `#[cfg(test)]`: spawn the listener logic on an ephemeral port with a fake hub trait, POST a Codex permission, answer through the hub, and assert the body.
- [ ] **Step 2:** Implement until `cargo fmt --check`, clippy and `cargo test` are all clean.
- [ ] **Step 3:** Commit `agents: hub wiring for approvals, sessions, sounds and shortcuts`.

### Task 13: Live verification (lead)

- [ ] `cargo run`. Settings → Agents → Update Claude Code and Codex. Read the preview first.
- [ ] Simulated: the `curl` scripts POST each route with the `X-Samlu-*` headers. Check the island (via accessibility tree, no screenshots), sound, tray title, launcher `#`, Settings Sessions, approve/deny/always responses, Codex hand-back at 30 s, the shortcut, and the reminder (set to 1 min).
- [ ] Real Claude CLI in Warp: approve on the island → command runs; deny → Claude reports the denial; answer in Warp → card leaves; View → Warp tab.
- [ ] Real Claude desktop session: View → that conversation.
- [ ] Real Codex CLI: trust the hooks, approve from the island; let one hand back.
- [ ] Note any spec item that couldn't be verified live in the final report.

### Task 14: Docs (Haiku)

- [ ] README "Agent notifications": Approve/Deny, Always allow, View targets, sounds, queue, shortcuts, sessions (`#`, tray), waiting count, reminder, Codex trust note, Windows differences row additions. PRODUCT.md surface 3 updated; "Reply from the island" and "answering agent questions" added to Deferred work. Commit `docs: agent sessions, approvals and sounds`.
