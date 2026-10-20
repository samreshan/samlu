# Agent Sessions, Approvals and Sounds — design

Date: 2026-10-20
Status: Approved 2026-10-20

## Context

Samlu already receives agent events. Claude Code posts HTTP hooks to the local
listener (`server/mod.rs`, `127.0.0.1:47823`, token in the path); Codex runs
`samlu --codex-notify`; Gemini CLI and Antigravity post through `curl`. Events
become `AgentEvent`s (`events.rs`), and `notify.rs` shows them in Notification
Center and on the Samlu Island (`public/agent-island/`).

The user's moment: an agent stopped for permission while they were elsewhere.
The island said it needed approval, made **no sound**, and offered **View**,
which opened Samlu's own Agents tab. That page can't answer the agent, and it
isn't where the agent was running.

What the user said they want:

- **Approve / Deny on the island**, with no window switching.
- **A sound on every notification**, with sounds they can choose.
- **View goes to the session itself**, never to a Samlu page.
- Where they run agents: **Claude desktop app, Warp, Codex app, Codex CLI**
  (Claude Code CLI and Codex CLI both run inside Warp).
- **Codex:** answer on the island, with Codex's own prompt waiting a short
  while.
- Extras chosen: (1) several pending requests at once, (2) keyboard
  approve/deny, (3) a live list of sessions, (4) a waiting count in the menu
  bar, (6) a reminder when a request is ignored.

## Goals

1. **Approve / Deny** permission requests from Claude Code (CLI and desktop
   app) and Codex (CLI, and the app if it honours hooks; see Verified facts) on
   the island. **Always allow** where the agent offers a rule (Claude only).
2. **View** on every agent notification opens the exact place the session
   lives: the Warp tab, the Claude desktop conversation, or the Codex thread.
   Otherwise it brings the right app forward. It never opens Samlu's Agents tab.
3. **Sounds:** one per event kind, chosen from the system's sounds or a file of
   the user's own, with a preview, a volume and a master switch.
4. **A queue:** several waiting requests stack ("1 of 3"). A new one never
   replaces an unanswered one.
5. **Keyboard:** global approve/deny shortcuts while a request is showing.
6. **Sessions list:** every known agent session and its state (working,
   waiting for you, idle), in the menu-bar menu, the launcher and Settings.
   Choosing one opens it, like View.
7. **Waiting count** beside the menu-bar icon.
8. **Reminder:** ring once more if a request is still unanswered after a
   while.

## Non-goals (this round)

- Skipping the sound and island when the session is already in front
  (offered as #5, not chosen).
- Replying to an agent from the island; answering an agent's multiple-choice
  questions (AskUserQuestion / elicitations) on the island.
- Approve/Deny for Gemini CLI and Antigravity (they get sounds and View
  only).
- Tab-level focus in terminals other than Warp. Terminal, iTerm, Ghostty and
  VS Code get the app brought forward only.
- Tab-level focus inside Windows Terminal.
- Action buttons inside native Notification Center banners.
- Persisting the sessions list across Samlu restarts.

## Verified facts (2026-10-20, on this Mac)

| Fact | Evidence |
|---|---|
| Claude Code `PermissionRequest` hook receives `tool_name`, `tool_input`, `permission_suggestions`, `session_id`, `cwd`, `permission_mode` | Live CLI test, v2.1.296; payload captured |
| In the Claude CLI the terminal prompt appears **at the same time** the hook starts; whichever answers first wins | Live test: dialog seen in the same second the hook started, while the hook slept 20 s |
| Claude desktop: hooks run alongside the app's own prompt ("whichever decides first applies") | Claude Code hooks docs, `canUseTool` note |
| Claude HTTP hooks forward env vars through `headers` + `allowedEnvVars` | Live test: `WARP_FOCUS_URL`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_HOST_SESSION_ID`, `TERM_PROGRAM` all arrived. `__CFBundleIdentifier` can't be forwarded (both `$…` and `${…}` fail, apparently because of the leading underscores), so Claude hooks don't carry a bundle id |
| HTTP hook default timeout 600 s; non-2xx, timeout, connection failure are non-blocking | Hooks docs |
| `CLAUDE_CODE_ENTRYPOINT` is `cli` in a terminal, `claude-desktop` in the desktop app | Live env |
| Desktop deep link `claude://code/continue?session=local_<id>`; id is `CLAUDE_CODE_HOST_SESSION_ID` (`^local_[A-Za-z0-9-]{1,64}$`). On for any signed-in user without the enterprise "disable deep links" policy. The same handler is registered on Windows | Claude.app 2.42304.2 handler source |
| Codex hooks (GA since v0.124; user has 0.160): `PermissionRequest`, `PostToolUse`, `Stop`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, inline `[[hooks.<Event>]]` in `~/.codex/config.toml`; **command hooks only, no http** | OpenAI Codex hooks docs |
| Codex `PermissionRequest` blocks the approval prompt until the hook returns; no decision → normal prompt; `updatedPermissions`/`interrupt` are reserved and fail closed | Codex hooks docs |
| Codex app = `/Applications/ChatGPT.app`, bundle `com.openai.codex`, deep link `codex://threads/<threadId>` | App bundle + source |
| Warp exports `WARP_FOCUS_URL` (`warp://session/<WARP_TERMINAL_SESSION_UUID>`) to every shell, and opening it brings Warp forward on that session | Warp binary strings; forwarded in the Claude test; a live focus test reported "works" before it was stopped (cross-window case unconfirmed) |
| The Codex app runs the same hook engine as the CLI (bundled `codex` 0.162-alpha, `app-server`, `~/.codex` config, hook UI wired). Hooks must be **trusted** in Codex before they run (`[hooks.state]` `trusted_hash`). Whether an allow skips the app's approval UI is very likely but not runtime-tested | Bundle and binary inspection |
| The Codex app's server runs with `CODEX_INTERNAL_ORIGINATOR_OVERRIDE=Codex Desktop`; whether that reaches hook commands is unconfirmed | Bundle inspection |

Must be confirmed at the start of the build (each changes only a fallback,
not the design): a logging Codex hook run once from the Codex app shows
which env vars arrive and that the app waits for PermissionRequest; and
`warp://session/<id>` focuses a tab in a second Warp window.

## Decisions

| Topic | Decision |
|---|---|
| Approval source of truth | Claude `PermissionRequest` and Codex `PermissionRequest` hooks. Claude's `Notification(permission_prompt)` for the same moment is folded into the pending request (see Duplicates) |
| Holding a request | The listener keeps the HTTP request open until the user answers, the request is resolved elsewhere, or the window elapses |
| Claude window | Hook `timeout: 600`. Samlu answers "no decision" at 590 s if nobody has answered. The terminal/app prompt has been live the whole time |
| Codex window | **30 s by default**, adjustable 10–120 s in Settings. Then Samlu answers "no decision", and Codex shows its own prompt |
| After a Codex hand-back | The card switches to **"Answer in Codex"** with View only. Approve/Deny are removed, so a late click can't send a stale answer. The card leaves on the next event from that session or when dismissed |
| View, pressed on a Codex request | Hands the request back immediately (no decision), then opens the session |
| Always allow | Claude only. Shown when `permission_suggestions` has an `addRules` entry with `behavior: allow`. Sends `updatedPermissions` with that suggestion unchanged. The button reads "Always allow" with the rule (e.g. `Bash(curl -sI …)`) in its tooltip |
| Deny | Claude: `behavior: deny`, `message: "Denied from Samlu"`, `interrupt: false`. Codex: `behavior: deny`, `message: "Denied from Samlu"` |
| Approvals and delivery | Approval cards **always use the island**, even when delivery is set to Notification Center, because banners there can't carry buttons. Notification Center still gets its entry. *(Builder's call; flagged for the user.)* |
| Island queue order | Oldest first. Approving, denying or resolving the front card shows the next. "1 of N" plus previous/next arrows |
| Other events while approvals wait | Completions and turns don't take over an approval card. They still ring, go to Notification Center, and update the sessions list |
| Needs-input notices without a decision (idle prompt, elicitation, Codex hand-back) | Persistent queue entries with View and Dismiss only |
| View routing order | 1. `WARP_FOCUS_URL` if present → `open` it. 2. Claude with entrypoint `claude-desktop` and a host session id → `claude://code/continue?session=<local_id>`. 3. Codex from the app → `codex://threads/<session_id>`. "From the app" = bundle id `com.openai.codex` or originator `Codex Desktop` when those reach the hook; otherwise, a Codex event with neither `WARP_FOCUS_URL` nor `TERM_PROGRAM` is treated as the app. 4. A known app bundle id (Codex/Gemini/Antigravity curl lines can send `$__CFBundleIdentifier`; Claude can't) or `TERM_PROGRAM` mapped to an app (`Apple_Terminal`, `iTerm.app`, `vscode`, `ghostty`, `WarpTerminal`) → bring that app forward. 5. Otherwise the island says "Couldn't find this session" briefly. **Never** Samlu's Agents tab |
| Codex hook trust | Samlu never writes Codex's trust hashes itself. After connecting, Settings says that Codex will ask once to trust Samlu's hooks (`/hooks` in the CLI or the app's prompt), and the Codex row shows "Waiting for Codex to trust Samlu's hooks" until the first Codex hook event arrives |
| Sounds | Kinds: `needs_input` (approvals, needs-input, reminders), `completed`, `turn_finished`. Choices: None, the system sounds, or Custom (file picked through the dialog plugin, copied into `app_data/sounds/`). Default needs_input = Glass, completed = Hero, turn_finished = None. Master switch on; volume 70 % |
| Sound playback | Rust side, behind the `platform` module, so it works whatever the delivery setting. Recommended: `rodio` (already on `cpal` 0.15) for volume on both platforms. Fallback `afplay -v` on macOS / `PlaySound` on Windows if rodio's formats or build cause trouble. Samlu's own notification banners stay silent, so nothing rings twice |
| System sound list | macOS: `/System/Library/Sounds/*.aiff`. Windows: `C:\Windows\Media\*.wav` |
| Shortcuts | Approve ⌥⌘Y, Deny ⌥⌘N on macOS. Ctrl+Alt+Y / Ctrl+Alt+N on Windows. Registered **only while an approvable card is in front**; act on the front card. Changeable in Settings with the existing recorder. Included in `suspend_global_shortcuts` / `resume_global_shortcuts` |
| Reminder | Off / 1 / 3 / 5 / 10 min, default 3. Rings the needs_input sound once per request, no more |
| Waiting count | Number of sessions waiting on the user. macOS: tray title beside the mark (`set_title("2")`); none when zero. The mark itself is never drawn on. Windows: tray tooltip "Samlu · 2 agents waiting" |
| Sessions list states | `working` (UserPromptSubmit, PostToolUse), `waiting` (pending approval, needs-input, or a handed-back Codex request), `idle` (Stop, completion). Registered on SessionStart or any first event. Removed on SessionEnd or after 12 h of silence. In memory only; sessions that started before Samlu appear on their next event |
| Sessions in the tray menu | An "Agents" section at the top of the menu, one row per session: agent, project, state; click = View. Rebuilt when the list changes |
| Sessions in the launcher | Prefix `#`. Rows show agent · project · state · age; Return = View. *(Prefix is the builder's call.)* |
| Sessions in Settings | A "Sessions" group in the Agents pane above Recent Activity, live |
| History | Approvals are recorded as `needs your input` events. The summary is what the agent wanted, plus an outcome when known: approved / denied in Samlu, answered in the agent, handed back |
| Upgrade path | An integration with Samlu hooks present but missing the new events reports **"Update needed"** with an **Update** button. It uses the same preview → backup → merge flow. Nothing is rewritten silently |
| Codex `notify` | Kept. Codex won't run new hooks until the user trusts them, and removing `notify` early would leave a silent gap. A `--codex-notify` turn-complete is dropped when a hook `Stop` for the same thread arrived within the last 10 s (or arrives within 2 s after), so turns aren't reported twice |
| Claude settings files | Global `~/.claude/settings.json` only, as today. Project-level installs are not offered, so no event fires twice. "Update needed" means Samlu's URL is present for some events but not all eight, the headers are missing, or PermissionRequest lacks `timeout: 600` |
| Island can't be shown | If the island window can't be created or shown, pending approvals are released at once with "no decision". The agent's own prompt takes over, and Notification Center still gets the entry. Full-screen Spaces are already handled (overlays show on every Space) |
| Island clicks | The island is a non-activating overlay that already takes clicks (View, Dismiss, Buddy). The new buttons and arrows use the same path. It never takes keyboard focus; keys come only through the global shortcuts |
| Handed-back Codex request | Counts as **waiting**: it stays in the waiting count, keeps its persistent card, and gets the reminder until the session's next event or Dismiss |
| What history stores | The action line, truncated to 160 characters. History stays local and is deleted by Clear. Full `tool_input` is kept only in memory while the request is pending |

## How it works

### Listener (`server/`)

- Handle each request on its own thread (or a small pool). Today one request
  blocks the loop, and a held approval would freeze every other hook.
- New routes: `POST /hooks/claude-code/permission/<token>`,
  `/hooks/codex/permission/<token>`, plus the session-tracking routes for
  each tool (`session-start`, `session-end`, `prompt`, `post-tool`, `stop`).
- Read the session-location headers (below) into the event.
- A `PendingApprovals` store in shared state, keyed by a generated request id.
  Each entry holds the event, agent, session, created-at, a hold deadline, and
  a one-shot channel. The handler thread waits on the channel until the
  deadline, then writes the response.
- Answers come only from Tauri commands invoked by the island, tray, launcher
  or shortcut. Never from HTTP. A web page that can reach the port still
  can't approve anything.

### Knowing where a session lives

Claude (HTTP hooks): every Samlu hook entry carries

```json
"headers": {
  "X-Samlu-Entrypoint": "$CLAUDE_CODE_ENTRYPOINT",
  "X-Samlu-Host-Session": "$CLAUDE_CODE_HOST_SESSION_ID",
  "X-Samlu-Warp-Focus": "$WARP_FOCUS_URL",
  "X-Samlu-Term": "$TERM_PROGRAM"
},
"allowedEnvVars": ["CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_HOST_SESSION_ID",
                   "WARP_FOCUS_URL", "TERM_PROGRAM"]
```

Codex (command hooks, run through the shell): a `curl` line like Gemini's
that POSTs stdin with the same `X-Samlu-*` headers taken from the shell
environment, plus `X-Samlu-Bundle: $__CFBundleIdentifier` and
`X-Samlu-Originator: $CODEX_INTERNAL_ORIGINATOR_OVERRIDE` (the shell expands
these fine), **prints the response body to stdout**, and has
`--max-time` = the Codex window + 5 s. If Samlu isn't running, curl fails
silently, prints nothing, and Codex carries on normally. Windows uses
`curl.exe`, as the Gemini integration already does.

`AgentEvent` gains an optional `origin` (entrypoint, host session, Warp focus
URL, terminal, bundle id) and, for approvals, `request_id`, `tool_name`, a
one-line `action` (derived from `tool_input.command`, `file_path`,
`url`/`pattern`, else `description`, else the tool name), and the
suggestions. Every new field is `#[serde(default)]`, because
`event-history.json` is persisted.

### Response bodies

| Answer | Claude (`PermissionRequest`) | Codex (`PermissionRequest`) |
|---|---|---|
| Approve | `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}` | same |
| Always allow | as Approve, plus `"updatedPermissions":[<the addRules suggestion, unchanged>]` inside `decision` | not offered |
| Deny | `…"decision":{"behavior":"deny","message":"Denied from Samlu","interrupt":false}` | `…"decision":{"behavior":"deny","message":"Denied from Samlu"}` |
| No decision (deadline, hand-back, island unavailable) | `{}` | empty body (curl prints nothing) |

Claude requests may use the chunked heartbeat below; the final chunk carries
one of these bodies. Codex requests are short (≤ 120 s) and only ever
answered by Samlu or handed back, so they get a single plain response with
no heartbeat.

### Resolution elsewhere

The user can answer in Warp or the desktop app instead. Two signals retire a
pending card:

1. **Any later event from the same session**: PostToolUse, Stop,
   UserPromptSubmit, the next PermissionRequest, SessionEnd. Samlu installs
   PostToolUse for this reason.
2. **The client dropping the held connection.** Claude cancels the hook when
   the prompt is answered in the terminal or app. Denying in the terminal
   often ends the turn without a Stop hook, so (1) alone could leave a stale
   card until the next prompt. Suggested mechanism: answer held requests with
   a chunked `200` whose body starts with JSON-legal whitespace heartbeats
   every ~2 s; a failed write means the client is gone. **The builder must
   verify this against the Claude CLI and desktop app before relying on it.**
   If it doesn't work, ship (1) alone and say so. Codex doesn't need it: its
   requests are short, and after a hand-back the card is View-only anyway.

A card retired this way leaves quietly. No sound, nothing in Notification
Center.

### Duplicates

Claude sends `Notification(permission_prompt)` and `PermissionRequest` for one
prompt, in either order. A `permission_prompt` notification is held about
1.5 s. If a PermissionRequest for that session arrives (or is already
pending), the notification is dropped. Otherwise it becomes a View-only
needs-input card as today. Only one sound plays per prompt.

### Island

- Payload becomes the queue state: list of cards (kind, agent, project,
  title, action line, buttons available, request id, position) plus the
  style.
- Approval card: title "Wants to run" / "Wants to edit" / "Needs approval",
  agent · project, the action line in a monospaced, single-line, truncated
  style (full text in its tooltip), and buttons **Deny**, **Approve**, an
  optional **Always allow**, and **View**. Plus "1 of N" and arrows when
  N > 1.
- The window may need to be taller than 138 pt. Keep Bloom and Buddy as they
  are; the existing arrive/swap/leave motion applies to card changes.
- Persistent while any approval or needs-input card is queued. Completions
  keep their timed lifetime when nothing is queued.

### Sessions registry

A `Sessions` store keyed by `(agent, session_id)`: agent, project path, origin
(for View), state, last event time, pending count. It drives the tray
section, the tray count, the launcher `#` mode and the Settings group. It
emits an `agents://sessions` event when it changes.

### Hooks Samlu installs

| Tool | Events |
|---|---|
| Claude Code (`~/.claude/settings.json`, HTTP) | SessionStart, SessionEnd, UserPromptSubmit, PermissionRequest (timeout 600), PostToolUse, Notification, Stop, TaskCompleted (existing ones keep working) |
| Codex (`~/.codex/config.toml`, inline `[[hooks.*]]`, command) | SessionStart, SessionEnd, UserPromptSubmit, PermissionRequest (timeout = window + 10), PostToolUse, Stop |
| Gemini CLI, Antigravity | Unchanged events. Add the `X-Samlu-*` headers to their curl lines so View can route |

Merges keep the current rules: preserve everything else, replace only
Samlu's own entries, back up first, previewable.

## Settings (Agents pane)

- **Sounds** group: master switch; one row per kind (needs you, task
  completed, turn finished), each with a picker (None, system sounds,
  Custom…) and a play button; volume slider.
- **Approvals** group: Codex wait time; Approve / Deny shortcuts (recorders);
  reminder interval.
- **Sessions** group (live) above Recent Activity.
- Connections rows gain the "Update needed" state.

## Windows

The same code paths compile and run. Sounds, queue, shortcuts, sessions list,
deep links (`claude://`, `codex://`, `warp://` where registered) and the
tooltip count all work. Bringing an app forward by bundle id is macOS-only;
on Windows View uses the deep links and otherwise shows the "couldn't find"
note. `other_platform.rs` gets stand-ins so the Linux test build compiles.

## Anti-goals

- An approval that Samlu answers on its own. Every allow or deny comes from a
  user action.
- A held request that keeps the agent stuck. Every hold has a deadline, and
  every failure path means "no decision".
- Rewriting a user's agent configuration without preview and backup.
- Sounds ringing twice for one event.
- Drawing anything on the Samlu mark (the count sits beside it).

## Testing

- Unit: payload parsing for the new routes (both tools), action-line
  derivation, merge/uninstall/needs-update for Claude JSON and Codex TOML,
  View routing table, pending-store deadlines and resolution, duplicate
  folding, sessions state transitions.
- Integration (scripted, no screenshots): fake hook POSTs with `curl` that
  hold, answer via Tauri command, and assert the HTTP response body and
  timing; island state checked through its window and accessibility tree.
- Live: one Claude CLI session in Warp and one in the desktop app, and one
  Codex CLI session. Approve, deny, Always allow, answer in the agent instead,
  Codex hand-back, View from each.

## Left to the builder

- Exact island layout and motion for the approval card and queue controls,
  within Bloom and Buddy.
- Thread-per-request vs pool; channel types; module layout.
- `rodio` vs `afplay`/`PlaySound`.
- Launcher prefix, row design, and tray menu wording.
- Whether the Settings Sessions group shares code with the tray rows.
