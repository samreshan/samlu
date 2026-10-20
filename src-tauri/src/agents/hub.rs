//! The one place that turns hook messages into sessions, cards, held
//! approvals and sounds.
//!
//! The decisions live in `AgentHub`'s core methods, which never touch the
//! app: they change the queue, the pending approvals and the sessions, and
//! return what the caller has to record. The `AppHandle` layer around them
//! plays sounds, shows the island and refreshes the tray.

use super::approvals::{Decision, PendingApprovals};
use super::island_queue::{Card, CardKind, IslandQueue};
use super::sessions::{SessionInfo, SessionState, Sessions};
use super::shortcuts;
use super::sounds::{self, SoundKind};
use super::view::{self, ViewTarget};
use super::{
    action::{approval_title, card_text},
    agent_label, project_label, ApprovalRequest, HookMessage, SessionSignal, SignalKind,
};
use crate::events::{AgentEvent, AgentEventKind};
use crate::settings::{AgentAlertSettings, AppConfig};
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// Samlu answers a Claude request "no decision" this long after it arrived,
/// ten seconds inside the hook's own 600 s timeout.
const CLAUDE_HOLD: Duration = Duration::from_secs(590);

/// How long a Claude permission notice waits for its PermissionRequest.
const DUPLICATE_FOLD: Duration = Duration::from_millis(1500);

/// A Codex `notify` turn-complete is dropped when a hook Stop for the same
/// thread arrived this recently, or arrives within `NOTIFY_WAIT` after it.
const NOTIFY_LOOKBACK: Duration = Duration::from_secs(10);
const NOTIFY_WAIT: Duration = Duration::from_secs(2);

/// Sessions silent for 12 hours leave on their own; this is how often to look.
const PRUNE_EVERY: Duration = Duration::from_secs(10 * 60);

/// History keeps the action line only this long.
const HISTORY_ACTION_CHARS: usize = 160;

const PERMISSION_PROMPT_SUMMARY: &str = "Waiting for permission to continue";
const NOT_FOUND_NOTE: &str = "Couldn't find this session";

const OUTCOME_APPROVED: &str = "approved in Samlu";
const OUTCOME_DENIED: &str = "denied in Samlu";
const OUTCOME_ELSEWHERE: &str = "answered in the agent";
const OUTCOME_HANDED_BACK: &str = "handed back";

// ---------------------------------------------------------------- pure rules

/// `Bash(curl -sI https://example.com)`: the first rule of a Claude
/// `addRules` suggestion, as the Always allow button's tooltip shows it. A
/// rule without content is just the tool's name.
pub fn always_allow_rule(suggestion: &Value) -> Option<String> {
    let rule = suggestion.get("rules")?.as_array()?.first()?;
    let tool = rule.get("toolName")?.as_str()?;
    match rule.get("ruleContent").and_then(Value::as_str) {
        Some(content) if !content.is_empty() => Some(format!("{tool}({content})")),
        _ => Some(tool.to_string()),
    }
}

/// How an answer given in Samlu reads in history. `None` for "no decision".
pub fn outcome_for(decision: Decision) -> Option<&'static str> {
    match decision {
        Decision::Allow | Decision::AllowAlways => Some(OUTCOME_APPROVED),
        Decision::Deny => Some(OUTCOME_DENIED),
        Decision::NoDecision => None,
    }
}

/// History stores the action line cut to 160 characters.
pub fn history_action(action: &str) -> String {
    if action.chars().count() <= HISTORY_ACTION_CHARS {
        return action.to_string();
    }
    let mut short: String = action.chars().take(HISTORY_ACTION_CHARS - 1).collect();
    short.push('…');
    short
}

/// Claude's `Notification(permission_prompt)`: the same moment as a
/// PermissionRequest, so it may be folded into one.
pub fn is_permission_prompt_notice(event: &AgentEvent) -> bool {
    event.agent == "claude-code"
        && event.kind == AgentEventKind::NeedsInput
        && event.summary.as_deref() == Some(PERMISSION_PROMPT_SUMMARY)
}

/// What an agent event clears from its session's cards. The permission
/// notice that accompanies a pending approval clears nothing. Only a turn
/// finishing (Stop) means the agent moved on, so only it answers live
/// approvals; a task completing or a notification leaves them waiting and
/// clears just the notices and handed-back cards.
fn event_retirement(event: &AgentEvent) -> Option<Scope> {
    if is_permission_prompt_notice(event) {
        return None;
    }
    Some(match event.kind {
        AgentEventKind::TurnFinished => Scope::Session,
        AgentEventKind::Completed | AgentEventKind::NeedsInput => Scope::Notices,
    })
}

/// The session state an event leaves behind.
pub fn state_after_event(kind: AgentEventKind) -> SessionState {
    match kind {
        AgentEventKind::NeedsInput => SessionState::Waiting,
        AgentEventKind::Completed | AgentEventKind::TurnFinished => SessionState::Idle,
    }
}

/// Claude's window is fixed; Codex's is a setting.
pub fn hold_for(agent: &str, codex_wait_secs: u64) -> Duration {
    if agent == "claude-code" {
        CLAUDE_HOLD
    } else {
        Duration::from_secs(codex_wait_secs)
    }
}

/// `None` when reminders are off.
pub fn reminder_delay(reminder_mins: u64) -> Option<Duration> {
    (reminder_mins > 0).then(|| Duration::from_secs(reminder_mins * 60))
}

/// The sound to play for `kind`, or `None` when sounds are off.
pub fn sound_choice(alerts: &AgentAlertSettings, kind: SoundKind) -> Option<&str> {
    let choice = match kind {
        SoundKind::NeedsInput => alerts.sound_needs_input.as_str(),
        SoundKind::Completed => alerts.sound_completed.as_str(),
        SoundKind::TurnFinished => alerts.sound_turn_finished.as_str(),
    };
    alerts.sounds_enabled.then_some(choice)
}

/// Which of a session's cards a retirement reaches.
enum Scope {
    Session,
    Agent(Option<String>),
    /// Needs-input and handed-back cards; live approvals stay.
    Notices,
}

/// What a card's button asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    Approve,
    Always,
    Deny,
    View,
    Dismiss,
}

impl CardAction {
    pub fn parse(action: &str) -> Option<Self> {
        Some(match action {
            "approve" => Self::Approve,
            "always" => Self::Always,
            "deny" => Self::Deny,
            "view" => Self::View,
            "dismiss" => Self::Dismiss,
            _ => return None,
        })
    }

    fn decision(self) -> Option<Decision> {
        match self {
            Self::Approve => Some(Decision::Allow),
            Self::Always => Some(Decision::AllowAlways),
            Self::Deny => Some(Decision::Deny),
            Self::View | Self::Dismiss => None,
        }
    }
}

/// Tracks hook Stops per Codex thread so the older `notify` path does not
/// report the same turn twice.
#[derive(Default)]
pub struct StopDedupe {
    stops: HashMap<String, Instant>,
}

impl StopDedupe {
    pub fn record_stop(&mut self, thread_id: &str, at: Instant) {
        // Nothing older than the lookback can matter, so the map stays small.
        self.stops
            .retain(|_, seen| at.saturating_duration_since(*seen) <= NOTIFY_LOOKBACK);
        self.stops.insert(thread_id.to_string(), at);
    }

    /// A hook Stop for this thread arrived no more than 10 s before `now`.
    pub fn stopped_recently(&self, thread_id: &str, now: Instant) -> bool {
        self.stops
            .get(thread_id)
            .is_some_and(|seen| now.saturating_duration_since(*seen) <= NOTIFY_LOOKBACK)
    }

    /// A hook Stop for this thread arrived at or after `since`.
    pub fn stopped_since(&self, thread_id: &str, since: Instant) -> bool {
        self.stops.get(thread_id).is_some_and(|seen| *seen >= since)
    }
}

/// How long after a PermissionRequest its twin notice is still folded away.
const RECENT_APPROVAL: Duration = Duration::from_secs(3);

/// Remembers when each session last asked for approval, so a permission
/// notice that trails an approval answered in a hurry is still folded.
#[derive(Default)]
pub struct RecentApprovals {
    seen: HashMap<(String, String), Instant>,
}

impl RecentApprovals {
    pub fn record(&mut self, agent: &str, session_id: &str, at: Instant) {
        self.seen
            .retain(|_, seen| at.saturating_duration_since(*seen) <= RECENT_APPROVAL);
        self.seen
            .insert((agent.to_string(), session_id.to_string()), at);
    }

    pub fn recent(&self, agent: &str, session_id: &str, now: Instant) -> bool {
        self.seen
            .get(&(agent.to_string(), session_id.to_string()))
            .is_some_and(|seen| now.saturating_duration_since(*seen) <= RECENT_APPROVAL)
    }
}

/// The approve/deny chords are live only while an approval is in front and
/// the island is on screen (a hidden island cannot be read), and never while
/// Settings is recording a chord.
pub fn shortcuts_wanted(front_is_approval: bool, island_hidden: bool, suspended: bool) -> bool {
    front_is_approval && !island_hidden && !suspended
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReminderAction {
    Skip,
    Ring,
    /// The island was flicked away: bring it back, then ring.
    PresentAndRing,
}

pub fn reminder_action(card_exists: bool, island_hidden: bool) -> ReminderAction {
    match (card_exists, island_hidden) {
        (false, _) => ReminderAction::Skip,
        (true, false) => ReminderAction::Ring,
        (true, true) => ReminderAction::PresentAndRing,
    }
}

/// What the tray menu shows of a session. The list changes on every tool
/// call (the event time), but the menu only when this does.
fn tray_signature(sessions: &[SessionInfo]) -> Vec<(String, String, String, SessionState)> {
    sessions
        .iter()
        .map(|s| {
            (
                s.agent.clone(),
                s.session_id.clone(),
                s.project_label.clone(),
                s.state,
            )
        })
        .collect()
}

fn approval_card(request_id: &str, request: &ApprovalRequest, now: DateTime<Utc>) -> Card {
    Card {
        id: format!("card-{request_id}"),
        kind: CardKind::Approval,
        agent: request.agent.clone(),
        agent_label: agent_label(&request.agent),
        session_id: request.session_id.clone(),
        agent_id: request.agent_id.clone(),
        project: project_label(&request.project_path),
        title: approval_title(&request.tool_name).to_string(),
        detail: card_text(&request.action),
        request_id: Some(request_id.to_string()),
        always_allow_rule: request.always_allow.as_ref().and_then(always_allow_rule),
        created_at: now,
    }
}

/// The card a Codex approval turns into once Codex has its own prompt up:
/// same place and id, View only.
fn handed_back_card(card: &Card) -> Card {
    Card {
        kind: CardKind::HandedBack,
        request_id: None,
        always_allow_rule: None,
        ..card.clone()
    }
}

// ------------------------------------------------------------------ the core

/// How a held approval ended, to put on the event stored for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryNote {
    pub agent: String,
    pub session_id: String,
    pub at: DateTime<Utc>,
    pub outcome: &'static str,
}

impl HistoryNote {
    fn of(card: &Card, outcome: &'static str) -> Self {
        Self {
            agent: card.agent.clone(),
            session_id: card.session_id.clone(),
            at: card.created_at,
            outcome,
        }
    }
}

pub struct Opened {
    pub id: String,
    pub decisions: Receiver<Decision>,
    pub hold: Duration,
    pub card: Card,
}

#[derive(Debug, Default)]
pub struct Change {
    /// The queue changed, so the island and shortcuts need to follow.
    pub queue: bool,
    pub notes: Vec<HistoryNote>,
    /// Agent and session to open (View).
    pub view: Option<(String, String)>,
}

pub struct AgentHub {
    app_data_dir: PathBuf,
    sessions: Sessions,
    approvals: PendingApprovals,
    queue: Mutex<IslandQueue>,
    /// Serialises "snapshot the queue, show it" so the island never receives
    /// an older snapshot after a newer one.
    present_lock: Mutex<()>,
    stops: Mutex<StopDedupe>,
    recent_approvals: Mutex<RecentApprovals>,
    /// The island was hidden (flicked away) with cards still queued.
    island_hidden: AtomicBool,
    tray_shown: Mutex<Vec<(String, String, String, SessionState)>>,
    card_counter: AtomicU64,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl AgentHub {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            app_data_dir: app_data_dir.to_path_buf(),
            sessions: Sessions::new(),
            approvals: PendingApprovals::new(),
            queue: Mutex::new(IslandQueue::new()),
            present_lock: Mutex::new(()),
            stops: Mutex::new(StopDedupe::default()),
            recent_approvals: Mutex::new(RecentApprovals::default()),
            island_hidden: AtomicBool::new(false),
            tray_shown: Mutex::new(Vec::new()),
            card_counter: AtomicU64::new(0),
        }
    }

    // -- core: no app, no side effects beyond the hub's own state --

    fn card_by_id(&self, card_id: &str) -> Option<Card> {
        lock(&self.queue)
            .snapshot()
            .cards
            .into_iter()
            .find(|card| card.id == card_id)
    }

    fn card_by_request(&self, request_id: &str) -> Option<Card> {
        lock(&self.queue)
            .snapshot()
            .cards
            .into_iter()
            .find(|card| card.request_id.as_deref() == Some(request_id))
    }

    fn has_approval_card(&self, agent: &str, session_id: &str) -> bool {
        lock(&self.queue).snapshot().cards.iter().any(|card| {
            card.kind == CardKind::Approval && card.agent == agent && card.session_id == session_id
        })
    }

    /// An approval for this session is pending, or was opened moments ago.
    fn approval_seen(&self, agent: &str, session_id: &str) -> bool {
        self.has_approval_card(agent, session_id)
            || lock(&self.recent_approvals).recent(agent, session_id, Instant::now())
    }

    pub fn queue_is_empty(&self) -> bool {
        lock(&self.queue).is_empty()
    }

    /// Agent and session of the card in front.
    pub fn front_session(&self) -> Option<(String, String)> {
        lock(&self.queue)
            .front()
            .map(|card| (card.agent.clone(), card.session_id.clone()))
    }

    fn front_approval_id(&self) -> Option<String> {
        lock(&self.queue)
            .front()
            .filter(|card| card.kind == CardKind::Approval)
            .map(|card| card.id.clone())
    }

    fn touch(&self, agent: &str, session_id: &str, state: Option<SessionState>) {
        self.sessions.touch(
            agent,
            session_id,
            "",
            &Default::default(),
            state,
            Utc::now(),
        );
    }

    /// Any card (approval, notice, handed back) still asks about this session.
    fn has_card(&self, agent: &str, session_id: &str) -> bool {
        lock(&self.queue)
            .snapshot()
            .cards
            .iter()
            .any(|card| card.agent == agent && card.session_id == session_id)
    }

    /// The session keeps reading Waiting while any card for it remains.
    fn keep_waiting(&self, agent: &str, session_id: &str) {
        if self.has_card(agent, session_id) {
            self.touch(agent, session_id, Some(SessionState::Waiting));
        }
    }

    /// Sets `state`, unless another card for the session still waits.
    fn settle_state(&self, agent: &str, session_id: &str, state: SessionState) {
        if self.has_card(agent, session_id) {
            self.touch(agent, session_id, Some(SessionState::Waiting));
        } else {
            self.touch(agent, session_id, Some(state));
        }
    }

    /// Answers the session's pending approvals "no decision" and drops all
    /// its cards: it has moved on, so they are stale. Approvals that go this
    /// way read "answered in the agent" in history.
    fn retire(&self, agent: &str, session_id: &str) -> Change {
        self.retire_scoped(agent, session_id, Scope::Session)
    }

    fn retire_scoped(&self, agent: &str, session_id: &str, scope: Scope) -> Change {
        let in_scope = |agent_id: &Option<String>| match &scope {
            Scope::Session | Scope::Notices => true,
            Scope::Agent(only) => agent_id == only,
        };
        if !matches!(scope, Scope::Notices) {
            self.approvals.retire_where(|r| {
                r.agent == agent && r.session_id == session_id && in_scope(&r.agent_id)
            });
        }
        let cards = lock(&self.queue).remove_where(|c| {
            c.agent == agent
                && c.session_id == session_id
                && in_scope(&c.agent_id)
                && (!matches!(scope, Scope::Notices) || c.kind != CardKind::Approval)
        });
        let notes = cards
            .iter()
            .filter(|card| card.kind == CardKind::Approval)
            .map(|card| HistoryNote::of(card, OUTCOME_ELSEWHERE))
            .collect();
        Change {
            queue: !cards.is_empty(),
            notes,
            view: None,
        }
    }

    /// Registers a held approval and queues its card. A live approval is
    /// never retired by a new one (parallel subagents ask side by side), but
    /// the same agent's stale notices and handed-back cards go.
    fn open_approval(
        &self,
        request: ApprovalRequest,
        codex_wait_secs: u64,
        now: DateTime<Utc>,
    ) -> (Opened, Change) {
        let stale = lock(&self.queue).remove_where(|c| {
            c.kind != CardKind::Approval
                && c.agent == request.agent
                && c.session_id == request.session_id
                && c.agent_id == request.agent_id
        });
        let mut change = Change {
            queue: !stale.is_empty(),
            ..Change::default()
        };
        let hold = hold_for(&request.agent, codex_wait_secs);
        let (id, decisions) = self.approvals.open(request.clone());
        let card = approval_card(&id, &request, now);
        lock(&self.queue).push(card.clone());
        self.sessions.touch(
            &request.agent,
            &request.session_id,
            &request.project_path,
            &request.origin,
            Some(SessionState::Waiting),
            now,
        );
        change.queue = true;
        (
            Opened {
                id,
                decisions,
                hold,
                card,
            },
            change,
        )
    }

    /// A button on a card. A card that is already gone is not an error.
    fn apply_action(&self, card_id: &str, action: CardAction) -> Change {
        let Some(card) = self.card_by_id(card_id) else {
            return Change::default();
        };
        let mut change = Change::default();
        match action {
            CardAction::Approve | CardAction::Always | CardAction::Deny => {
                // Only a live approval can be answered; a handed-back card
                // has no request left, so a late click can't send a stale one.
                let (Some(decision), CardKind::Approval, Some(request_id)) =
                    (action.decision(), card.kind, card.request_id.as_deref())
                else {
                    return change;
                };
                lock(&self.queue).remove(card_id);
                change.queue = true;
                if self.approvals.answer(request_id, decision).is_some() {
                    if let Some(outcome) = outcome_for(decision) {
                        change.notes.push(HistoryNote::of(&card, outcome));
                    }
                    self.settle_state(&card.agent, &card.session_id, SessionState::Working);
                }
            }
            CardAction::Dismiss => {
                lock(&self.queue).remove(card_id);
                change.queue = true;
                if let Some(request_id) = card.request_id.as_deref() {
                    // The agent's own prompt takes over.
                    self.approvals.answer(request_id, Decision::NoDecision);
                    change
                        .notes
                        .push(HistoryNote::of(&card, OUTCOME_HANDED_BACK));
                }
                // Nothing left to wait on here: the notice was all that
                // kept the session Waiting.
                if !self.has_card(&card.agent, &card.session_id) {
                    self.touch(&card.agent, &card.session_id, Some(SessionState::Idle));
                }
            }
            CardAction::View => {
                if card.kind == CardKind::Approval && card.agent == "codex" {
                    // Codex shows its own prompt as soon as this answers, so
                    // the island must stop offering Approve and Deny.
                    if let Some(request_id) = card.request_id.as_deref() {
                        self.approvals.answer(request_id, Decision::NoDecision);
                        lock(&self.queue).replace(card_id, handed_back_card(&card));
                        change.queue = true;
                        change
                            .notes
                            .push(HistoryNote::of(&card, OUTCOME_HANDED_BACK));
                    }
                }
                change.view = Some((card.agent, card.session_id));
            }
        }
        change
    }

    /// The hold ran out. Codex keeps a View-only card (the session still
    /// waits for the user); Claude's own prompt has been live all along, so
    /// its card just goes.
    fn expire(&self, request_id: &str) -> Change {
        self.approvals.answer(request_id, Decision::NoDecision);
        let Some(card) = self.card_by_request(request_id) else {
            return Change::default();
        };
        if card.agent == "codex" {
            lock(&self.queue).replace(&card.id, handed_back_card(&card));
        } else {
            lock(&self.queue).remove(&card.id);
        }
        Change {
            queue: true,
            notes: vec![HistoryNote::of(&card, OUTCOME_HANDED_BACK)],
            view: None,
        }
    }

    /// The client dropped the held connection: the user answered in the
    /// agent, so the session runs on (PostToolUse or Stop corrects it). The
    /// card leaves quietly.
    fn abandon(&self, request_id: &str, session_state: SessionState) -> Change {
        self.approvals.answer(request_id, Decision::NoDecision);
        let Some(card) = lock(&self.queue).remove_by_request(request_id) else {
            return Change::default();
        };
        self.settle_state(&card.agent, &card.session_id, session_state);
        Change {
            queue: true,
            notes: vec![HistoryNote::of(&card, OUTCOME_ELSEWHERE)],
            view: None,
        }
    }

    fn signal(&self, signal: &SessionSignal) -> Change {
        let touch = |state| {
            self.sessions.touch(
                &signal.agent,
                &signal.session_id,
                &signal.project_path,
                &signal.origin,
                Some(state),
                Utc::now(),
            );
        };
        match signal.kind {
            SignalKind::Started => {
                touch(SessionState::Idle);
                Change::default()
            }
            SignalKind::Working => {
                touch(SessionState::Working);
                self.retire(&signal.agent, &signal.session_id)
            }
            // One tool call finished: only the agent that ran it has moved
            // on, not a subagent running beside it.
            SignalKind::ToolFinished => {
                touch(SessionState::Working);
                let change = self.retire_scoped(
                    &signal.agent,
                    &signal.session_id,
                    Scope::Agent(signal.agent_id.clone()),
                );
                self.keep_waiting(&signal.agent, &signal.session_id);
                change
            }
            SignalKind::Ended => {
                self.sessions.end(&signal.agent, &signal.session_id);
                self.retire(&signal.agent, &signal.session_id)
            }
        }
    }

    // -- the app layer --

    fn alerts(app: &AppHandle) -> AgentAlertSettings {
        app.state::<Arc<AppConfig>>().agent_alerts()
    }

    pub fn play(&self, app: &AppHandle, kind: SoundKind) {
        let alerts = Self::alerts(app);
        if let Some(choice) = sound_choice(&alerts, kind) {
            sounds::play_choice(choice, alerts.volume, &self.app_data_dir);
        }
    }

    fn record_notes(&self, app: &AppHandle, notes: &[HistoryNote]) {
        let state = app.state::<Arc<AppState>>();
        for note in notes {
            if let Some(event) =
                state.set_event_outcome(&note.agent, &note.session_id, note.at, note.outcome)
            {
                let _ = app.emit_to("main", "agent://event", &event);
            }
        }
    }

    /// Held while the island is shown or hidden, so a hide never lands in
    /// the middle of a presentation.
    pub fn present_guard(&self) -> MutexGuard<'_, ()> {
        lock(&self.present_lock)
    }

    /// Shows the queue as it is now. `Err` means the island can't be shown.
    fn present(&self, app: &AppHandle) -> Result<(), String> {
        let _guard = lock(&self.present_lock);
        let snapshot = lock(&self.queue).snapshot();
        let showing = !snapshot.cards.is_empty();
        let shown = crate::notify::present_queue(app, snapshot);
        if showing && shown.is_ok() {
            self.island_hidden.store(false, Ordering::Release);
        }
        shown
    }

    /// The island left the screen (flicked away, or its open button) with
    /// cards still queued: the chords stop answering what nobody can see.
    /// The next present, or a reminder, brings it back.
    pub fn island_hidden(&self, app: &AppHandle) {
        self.island_hidden.store(true, Ordering::Release);
        self.sync_shortcuts(app);
    }

    /// Registers the approve/deny chords while an approvable card is in
    /// front and visible, and frees them otherwise. The plugin waits on the
    /// main thread while holding its own lock, so registration only ever runs
    /// there, one change at a time, reading the state when it runs.
    pub fn sync_shortcuts(&self, app: &AppHandle) {
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            let hub = handle.state::<Arc<AgentHub>>().inner().clone();
            hub.apply_shortcuts(&handle);
        }) {
            log::warn!("could not sync approval shortcuts: {error}");
        }
    }

    fn apply_shortcuts(&self, app: &AppHandle) {
        let wanted = shortcuts_wanted(
            self.front_approval_id().is_some(),
            self.island_hidden.load(Ordering::Acquire),
            shortcuts::is_suspended(),
        );
        if wanted {
            let alerts = Self::alerts(app);
            if let Err(error) = shortcuts::arm(app, &alerts.approve_shortcut, &alerts.deny_shortcut)
            {
                log::warn!("approval shortcuts unavailable: {error}");
            }
        } else {
            shortcuts::disarm(app);
        }
    }

    /// Sessions changed: tell the pages, and the tray when its menu differs.
    fn broadcast_sessions(&self, app: &AppHandle) {
        let list = self.sessions.list();
        let _ = app.emit("agents://sessions", &list);
        let signature = tray_signature(&list);
        let changed = {
            let mut shown = lock(&self.tray_shown);
            let changed = *shown != signature;
            *shown = signature;
            changed
        };
        if changed {
            crate::tray::refresh_agents(app, &list);
        }
    }

    /// After anything that may have changed the queue or the sessions.
    fn settle(&self, app: &AppHandle, change: &Change) {
        self.record_notes(app, &change.notes);
        if change.queue {
            if let Err(error) = self.present(app) {
                log::warn!("could not update the island: {error}");
            }
            self.sync_shortcuts(app);
        }
        self.broadcast_sessions(app);
    }

    pub fn handle(&self, app: &AppHandle, message: HookMessage) {
        self.note_codex_hooks(app, &message);
        match message {
            HookMessage::Event(event) => self.handle_event(app, event, true),
            HookMessage::Signal(signal) => {
                let change = self.signal(&signal);
                self.settle(app, &change);
            }
            // The listener holds approvals through `begin_approval`.
            HookMessage::Approval(_) => {}
        }
    }

    /// The first hook message from Codex proves it trusted Samlu's hooks.
    fn note_codex_hooks(&self, app: &AppHandle, message: &HookMessage) {
        let agent = match message {
            HookMessage::Event(event) => &event.agent,
            HookMessage::Signal(signal) => &signal.agent,
            HookMessage::Approval(request) => &request.agent,
        };
        let session = match message {
            HookMessage::Event(event) => &event.session_id,
            HookMessage::Signal(signal) => &signal.session_id,
            HookMessage::Approval(request) => &request.session_id,
        };
        if agent == "codex" && session != crate::notify::PREVIEW_SESSION {
            let config = app.state::<Arc<AppConfig>>();
            if !config.codex_hooks_seen() {
                config.mark_codex_hooks_seen();
                let _ = app.emit("agents://integrations", ());
            }
        }
    }

    /// `from_hook`: the event came from an installed hook, not from Codex's
    /// older `notify` command.
    fn handle_event(&self, app: &AppHandle, event: AgentEvent, from_hook: bool) {
        if is_permission_prompt_notice(&event) {
            if self.approval_seen(&event.agent, &event.session_id) {
                return;
            }
            // The PermissionRequest may be a moment behind its notice.
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(DUPLICATE_FOLD);
                let hub = app.state::<Arc<AgentHub>>().inner().clone();
                if !hub.approval_seen(&event.agent, &event.session_id) {
                    hub.process_event(&app, event);
                }
            });
            return;
        }
        if from_hook && event.agent == "codex" {
            lock(&self.stops).record_stop(&event.session_id, Instant::now());
        }
        self.process_event(app, event);
    }

    fn process_event(&self, app: &AppHandle, event: AgentEvent) {
        let state = app.state::<Arc<AppState>>().inner().clone();
        state.record_event(event.clone());
        let _ = app.emit_to("main", "agent://event", &event);

        self.sessions.touch(
            &event.agent,
            &event.session_id,
            &event.project_path,
            &event.origin,
            Some(state_after_event(event.kind)),
            Utc::now(),
        );
        let change = match event_retirement(&event) {
            Some(scope) => self.retire_scoped(&event.agent, &event.session_id, scope),
            None => Change::default(),
        };
        self.keep_waiting(&event.agent, &event.session_id);
        self.settle(app, &change);
        crate::notify::present_event(app.clone(), state, event);
    }

    /// A `--codex-notify` turn-complete. Dropped when a hook Stop for the
    /// same thread was seen in the last 10 s, or arrives in the next 2 s.
    pub fn handle_codex_notify(&self, app: &AppHandle, event: AgentEvent) {
        let arrived = Instant::now();
        if lock(&self.stops).stopped_recently(&event.session_id, arrived) {
            log::info!("dropped Codex notify: a hook Stop was just seen");
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(NOTIFY_WAIT);
            let hub = app.state::<Arc<AgentHub>>().inner().clone();
            if lock(&hub.stops).stopped_since(&event.session_id, arrived) {
                log::info!("dropped Codex notify: a hook Stop followed");
                return;
            }
            hub.handle_event(&app, event, false);
        });
    }

    /// A needs-input notice, queued until the user dismisses it.
    pub fn push_needs_input(&self, app: &AppHandle, event: &AgentEvent, detail: String) {
        let n = self.card_counter.fetch_add(1, Ordering::Relaxed) + 1;
        lock(&self.queue).push(Card {
            id: format!("notice-{n}"),
            kind: CardKind::NeedsInput,
            agent: event.agent.clone(),
            agent_label: event.agent_label(),
            session_id: event.session_id.clone(),
            agent_id: None,
            project: event.project_label(),
            title: "Waiting for you".to_string(),
            detail,
            request_id: None,
            always_allow_rule: None,
            created_at: Utc::now(),
        });
        if let Err(error) = self.present(app) {
            log::error!("failed to show Samlu Island: {error}");
        }
        self.sync_shortcuts(app);
    }

    /// Opens a held approval and returns (request id, receiver, hold).
    pub fn begin_approval(
        &self,
        app: &AppHandle,
        request: ApprovalRequest,
    ) -> (String, Receiver<Decision>, Duration) {
        self.note_codex_hooks(app, &HookMessage::Approval(request.clone()));
        let alerts = Self::alerts(app);
        let notify = app
            .state::<Arc<AppConfig>>()
            .preferences()
            .notify_needs_input;
        let state = app.state::<Arc<AppState>>().inner().clone();
        let agent = request.agent.clone();
        let action = request.action.clone();
        let now = Utc::now();
        let (opened, change) = self.open_approval(request.clone(), alerts.codex_wait_secs, now);
        lock(&self.recent_approvals).record(&agent, &request.session_id, Instant::now());

        // History first, so the outcome has an event to land on.
        let event = AgentEvent {
            agent: request.agent.clone(),
            kind: AgentEventKind::NeedsInput,
            project_path: request.project_path.clone(),
            summary: Some(history_action(&action)),
            session_id: request.session_id.clone(),
            timestamp: opened.card.created_at,
            origin: request.origin.clone(),
            outcome: None,
        };
        state.record_event(event.clone());
        let _ = app.emit_to("main", "agent://event", &event);
        self.record_notes(app, &change.notes);

        self.play(app, SoundKind::NeedsInput);
        // The banner follows the needs-input switch; the card and the sound
        // do not (the sound has its own master switch).
        if notify {
            if let Err(error) = app
                .notification()
                .builder()
                .title(format!("{} needs approval", agent_label(&agent)))
                .body(card_text(&action))
                .show()
            {
                log::error!("failed to show approval notification: {error}");
            }
        }

        // Approvals always use the island, whatever delivery is set to: a
        // banner cannot carry buttons. If it cannot be shown, give the
        // request straight back so the agent's own prompt takes over.
        match self.present(app) {
            Ok(()) => {
                self.sync_shortcuts(app);
                if let Some(delay) = reminder_delay(alerts.reminder_mins) {
                    self.schedule_reminder(app, opened.card.id.clone(), delay);
                }
            }
            Err(error) => {
                log::error!("approval released, island unavailable: {error}");
                let released =
                    self.abandon_with(&opened.id, OUTCOME_HANDED_BACK, SessionState::Waiting);
                self.record_notes(app, &released.notes);
            }
        }
        self.broadcast_sessions(app);
        (opened.id, opened.decisions, opened.hold)
    }

    /// Like `abandon`, with a chosen outcome for history.
    fn abandon_with(
        &self,
        request_id: &str,
        outcome: &'static str,
        session_state: SessionState,
    ) -> Change {
        let mut change = self.abandon(request_id, session_state);
        for note in &mut change.notes {
            note.outcome = outcome;
        }
        change
    }

    /// Rings the needs-input sound once more if the card still waits.
    fn schedule_reminder(&self, app: &AppHandle, card_id: String, delay: Duration) {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let hub = app.state::<Arc<AgentHub>>().inner().clone();
            let hidden = hub.island_hidden.load(Ordering::Acquire);
            match reminder_action(hub.card_by_id(&card_id).is_some(), hidden) {
                ReminderAction::Skip => {}
                ReminderAction::Ring => hub.play(&app, SoundKind::NeedsInput),
                ReminderAction::PresentAndRing => {
                    if hub.present(&app).is_ok() {
                        hub.sync_shortcuts(&app);
                    }
                    hub.play(&app, SoundKind::NeedsInput);
                }
            }
        });
    }

    pub fn hold_elapsed(&self, app: &AppHandle, request_id: &str) {
        let change = self.expire(request_id);
        self.settle(app, &change);
    }

    pub fn client_gone(&self, app: &AppHandle, request_id: &str) {
        let change = self.abandon(request_id, SessionState::Working);
        self.settle(app, &change);
    }

    pub fn card_action(&self, app: &AppHandle, card_id: &str, action: &str) -> Result<(), String> {
        let action =
            CardAction::parse(action).ok_or_else(|| format!("unknown action: {action}"))?;
        let change = self.apply_action(card_id, action);
        self.settle(app, &change);
        if let Some((agent, session_id)) = &change.view {
            if let Err(note) = try_view(self, agent, session_id) {
                let _ = app.emit_to("agent-island", "island://note", note);
            }
        }
        Ok(())
    }

    /// A shortcut press answers the front card, if it is an approval.
    pub fn shortcut_answer(&self, app: &AppHandle, approve: bool) {
        if let Some(card_id) = self.front_approval_id() {
            let action = if approve { "approve" } else { "deny" };
            let _ = self.card_action(app, &card_id, action);
        }
    }

    pub fn step_queue(&self, app: &AppHandle, delta: i32) {
        lock(&self.queue).step(delta);
        let _ = self.present(app);
        self.sync_shortcuts(app);
    }

    pub fn sessions_list(&self) -> Vec<SessionInfo> {
        self.sessions.list()
    }

    /// Drops sessions silent for 12 hours.
    pub fn prune(&self, app: &AppHandle) {
        if self.sessions.prune(Utc::now()) {
            self.broadcast_sessions(app);
        }
    }

    /// Where a session lives. Every approval touches the registry with its
    /// origin, so a card's session is always known until it ends or is pruned.
    fn origin_of(&self, agent: &str, session_id: &str) -> Option<super::origin::SessionOrigin> {
        self.sessions.get(agent, session_id).map(|s| s.origin)
    }

    fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }
}

/// Opens the place a session lives in. `Err` carries the island note.
fn try_view(hub: &AgentHub, agent: &str, session_id: &str) -> Result<(), String> {
    let Some(origin) = hub.origin_of(agent, session_id) else {
        return Err(NOT_FOUND_NOTE.to_string());
    };
    match view::resolve(agent, session_id, &origin) {
        ViewTarget::NotFound => Err(NOT_FOUND_NOTE.to_string()),
        target => view::open(&target),
    }
}

/// Used by the tray and `agent_session_view`.
pub fn view_session(app: &AppHandle, agent: &str, session_id: &str) {
    let hub = app.state::<Arc<AgentHub>>();
    if let Err(note) = try_view(&hub, agent, session_id) {
        let _ = app.emit_to("agent-island", "island://note", note);
    }
}

/// Re-registers the shortcuts for the current front card and settings.
pub fn resync_shortcuts(app: &AppHandle) {
    app.state::<Arc<AgentHub>>().sync_shortcuts(app);
}

/// Starts the timer that drops sessions silent for 12 hours.
pub fn start_prune_timer(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(PRUNE_EVERY);
        app.state::<Arc<AgentHub>>().prune(&app);
    });
}

/// A Codex `--codex-notify` event, deduplicated against hook Stops.
pub fn codex_notify(app: &AppHandle, event: AgentEvent) {
    app.state::<Arc<AgentHub>>().handle_codex_notify(app, event);
}

// ------------------------------------------------------------------ commands

/// `approve` | `always` | `deny` | `view` | `dismiss`
#[tauri::command(async)]
pub fn agent_card_action(app: AppHandle, card_id: String, action: String) -> Result<(), String> {
    let hub = app.state::<Arc<AgentHub>>().inner().clone();
    hub.card_action(&app, &card_id, &action)
}

#[tauri::command(async)]
pub fn agent_queue_step(app: AppHandle, delta: i32) {
    let hub = app.state::<Arc<AgentHub>>().inner().clone();
    hub.step_queue(&app, delta);
}

#[tauri::command]
pub fn agent_sessions(app: AppHandle) -> Vec<SessionInfo> {
    app.state::<Arc<AgentHub>>().sessions_list()
}

#[tauri::command(async)]
pub fn agent_session_view(app: AppHandle, agent: String, session_id: String) -> Result<(), String> {
    let hub = app.state::<Arc<AgentHub>>();
    try_view(&hub, &agent, &session_id)
}

/// `{"system": [names]}`
#[tauri::command]
pub fn agent_sound_catalog() -> serde_json::Value {
    serde_json::json!({ "system": sounds::system_sounds() })
}

/// Plays a choice right now, whether or not sounds are switched on, so the
/// picker can be tried out.
#[tauri::command]
pub fn agent_sound_preview(app: AppHandle, choice: String) {
    let volume = app.state::<Arc<AppConfig>>().agent_alerts().volume;
    let hub = app.state::<Arc<AgentHub>>();
    sounds::play_choice(&choice, volume, hub.app_data_dir());
}

/// Picks a sound file with the system dialog and copies it into Samlu's own
/// folder. `None` when the dialog is cancelled.
#[tauri::command]
pub async fn agent_sound_choose_custom(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let dir = app.state::<Arc<AgentHub>>().app_data_dir().to_path_buf();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose a sound")
            .add_filter("Sounds", sounds::importable_extensions())
            .blocking_pick_file()
            .and_then(|path| path.into_path().ok())
    })
    .await
    .map_err(|error| error.to_string())?;
    match picked {
        Some(path) => sounds::import_custom(&path, &dir).map(Some),
        None => Ok(None),
    }
}

/// The listener's view of the hub.
pub struct HubHost {
    app: AppHandle,
}

impl HubHost {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn hub(&self) -> Arc<AgentHub> {
        self.app.state::<Arc<AgentHub>>().inner().clone()
    }
}

impl crate::server::HookHost for HubHost {
    fn handle(&self, message: HookMessage) {
        self.hub().handle(&self.app, message);
    }

    fn begin_approval(&self, request: ApprovalRequest) -> crate::server::OpenApproval {
        let (id, decisions, hold) = self.hub().begin_approval(&self.app, request);
        crate::server::OpenApproval {
            id,
            decisions,
            hold,
        }
    }

    fn hold_elapsed(&self, request_id: &str) {
        self.hub().hold_elapsed(&self.app, request_id);
    }

    fn client_gone(&self, request_id: &str) {
        self.hub().client_gone(&self.app, request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::origin::SessionOrigin;
    use serde_json::json;

    fn request(agent: &str, session: &str) -> ApprovalRequest {
        ApprovalRequest {
            agent: agent.to_string(),
            session_id: session.to_string(),
            agent_id: None,
            project_path: "/Users/me/samlu".to_string(),
            tool_name: "Bash".to_string(),
            action: "curl -sI https://example.com".to_string(),
            always_allow: None,
            origin: SessionOrigin::default(),
        }
    }

    fn event(agent: &str, kind: AgentEventKind, summary: &str) -> AgentEvent {
        AgentEvent {
            agent: agent.to_string(),
            kind,
            project_path: "/p".to_string(),
            summary: Some(summary.to_string()),
            session_id: "s1".to_string(),
            timestamp: Utc::now(),
            origin: SessionOrigin::default(),
            outcome: None,
        }
    }

    fn hub() -> AgentHub {
        AgentHub::new(Path::new("/nonexistent"))
    }

    fn open(hub: &AgentHub, agent: &str, session: &str) -> Opened {
        hub.open_approval(request(agent, session), 30, Utc::now()).0
    }

    // -- pure rules --

    #[test]
    fn always_allow_rule_joins_tool_and_content() {
        let suggestion = json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash", "ruleContent": "curl -sI https://example.com"}],
            "behavior": "allow",
            "destination": "localSettings",
        });
        assert_eq!(
            always_allow_rule(&suggestion).as_deref(),
            Some("Bash(curl -sI https://example.com)")
        );
    }

    #[test]
    fn always_allow_rule_takes_the_first_rule_and_tolerates_gaps() {
        let two = json!({"rules": [
            {"toolName": "Edit", "ruleContent": "src/**"},
            {"toolName": "Bash", "ruleContent": "ls"},
        ]});
        assert_eq!(always_allow_rule(&two).as_deref(), Some("Edit(src/**)"));
        let bare = json!({"rules": [{"toolName": "WebFetch"}]});
        assert_eq!(always_allow_rule(&bare).as_deref(), Some("WebFetch"));
        assert_eq!(always_allow_rule(&json!({"rules": []})), None);
        assert_eq!(always_allow_rule(&json!({})), None);
    }

    #[test]
    fn outcomes_read_as_history_lines() {
        assert_eq!(outcome_for(Decision::Allow), Some("approved in Samlu"));
        assert_eq!(
            outcome_for(Decision::AllowAlways),
            Some("approved in Samlu")
        );
        assert_eq!(outcome_for(Decision::Deny), Some("denied in Samlu"));
        assert_eq!(outcome_for(Decision::NoDecision), None);
    }

    #[test]
    fn history_action_is_cut_to_160_characters() {
        let long = "x".repeat(400);
        let cut = history_action(&long);
        assert_eq!(cut.chars().count(), 160);
        assert!(cut.ends_with('…'));
        let exact = "y".repeat(160);
        assert_eq!(history_action(&exact), exact);
        assert_eq!(history_action("ls"), "ls");
    }

    #[test]
    fn only_claudes_permission_notice_counts_as_a_duplicate() {
        let notice = event(
            "claude-code",
            AgentEventKind::NeedsInput,
            "Waiting for permission to continue",
        );
        assert!(is_permission_prompt_notice(&notice));
        assert!(event_retirement(&notice).is_none());
        let idle = event(
            "claude-code",
            AgentEventKind::NeedsInput,
            "Has been idle a while",
        );
        assert!(!is_permission_prompt_notice(&idle));
        assert!(matches!(event_retirement(&idle), Some(Scope::Notices)));
        let codex = event(
            "codex",
            AgentEventKind::NeedsInput,
            "Waiting for permission to continue",
        );
        assert!(!is_permission_prompt_notice(&codex));
        let stop = event("claude-code", AgentEventKind::TurnFinished, "done");
        assert!(matches!(event_retirement(&stop), Some(Scope::Session)));
        let task = event("claude-code", AgentEventKind::Completed, "done");
        assert!(matches!(event_retirement(&task), Some(Scope::Notices)));
    }

    #[test]
    fn event_kinds_set_session_state() {
        assert_eq!(
            state_after_event(AgentEventKind::NeedsInput),
            SessionState::Waiting
        );
        assert_eq!(
            state_after_event(AgentEventKind::Completed),
            SessionState::Idle
        );
        assert_eq!(
            state_after_event(AgentEventKind::TurnFinished),
            SessionState::Idle
        );
    }

    #[test]
    fn holds_and_reminders() {
        assert_eq!(hold_for("claude-code", 30), Duration::from_secs(590));
        assert_eq!(hold_for("codex", 45), Duration::from_secs(45));
        assert_eq!(reminder_delay(0), None);
        assert_eq!(reminder_delay(3), Some(Duration::from_secs(180)));
    }

    #[test]
    fn sound_follows_kind_and_master_switch() {
        let mut alerts = AgentAlertSettings::default();
        assert_eq!(sound_choice(&alerts, SoundKind::TurnFinished), Some("none"));
        assert_eq!(
            sound_choice(&alerts, SoundKind::NeedsInput),
            Some(alerts.sound_needs_input.as_str())
        );
        alerts.sounds_enabled = false;
        assert_eq!(sound_choice(&alerts, SoundKind::NeedsInput), None);
    }

    #[test]
    fn card_actions_parse() {
        for (text, action) in [
            ("approve", CardAction::Approve),
            ("always", CardAction::Always),
            ("deny", CardAction::Deny),
            ("view", CardAction::View),
            ("dismiss", CardAction::Dismiss),
        ] {
            assert_eq!(CardAction::parse(text), Some(action));
        }
        assert_eq!(CardAction::parse("nope"), None);
    }

    #[test]
    fn notify_is_dropped_within_ten_seconds_of_a_hook_stop() {
        let t0 = Instant::now();
        let mut stops = StopDedupe::default();
        assert!(!stops.stopped_recently("t1", t0));
        stops.record_stop("t1", t0);
        assert!(stops.stopped_recently("t1", t0 + Duration::from_secs(10)));
        assert!(!stops.stopped_recently("t1", t0 + Duration::from_millis(10_001)));
        assert!(!stops.stopped_recently("other", t0));
    }

    #[test]
    fn notify_is_dropped_when_a_stop_arrives_after_it() {
        let t0 = Instant::now();
        let mut stops = StopDedupe::default();
        assert!(!stops.stopped_since("t1", t0));
        stops.record_stop("t1", t0 + Duration::from_millis(1500));
        assert!(stops.stopped_since("t1", t0));
        assert!(!stops.stopped_since("t1", t0 + Duration::from_secs(2)));
    }

    #[test]
    fn tray_signature_ignores_event_times() {
        let sessions = Sessions::new();
        sessions.touch(
            "codex",
            "a",
            "/p",
            &SessionOrigin::default(),
            Some(SessionState::Idle),
            Utc::now(),
        );
        let before = tray_signature(&sessions.list());
        sessions.touch(
            "codex",
            "a",
            "/p",
            &SessionOrigin::default(),
            None,
            Utc::now(),
        );
        assert_eq!(tray_signature(&sessions.list()), before);
        sessions.touch(
            "codex",
            "a",
            "/p",
            &SessionOrigin::default(),
            Some(SessionState::Waiting),
            Utc::now(),
        );
        assert_ne!(tray_signature(&sessions.list()), before);
    }

    // -- the core --

    #[test]
    fn opening_an_approval_queues_a_card_and_marks_the_session_waiting() {
        let hub = hub();
        let mut req = request("claude-code", "s1");
        req.always_allow = Some(json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash", "ruleContent": "curl -sI https://example.com"}],
            "behavior": "allow",
        }));
        let (opened, change) = hub.open_approval(req, 30, Utc::now());
        assert!(change.queue);
        assert_eq!(opened.hold, Duration::from_secs(590));
        assert_eq!(opened.card.kind, CardKind::Approval);
        assert_eq!(opened.card.title, "Wants to run");
        assert_eq!(opened.card.detail, "curl -sI https://example.com");
        assert_eq!(opened.card.project, "samlu");
        assert_eq!(
            opened.card.always_allow_rule.as_deref(),
            Some("Bash(curl -sI https://example.com)")
        );
        assert_eq!(opened.card.request_id.as_deref(), Some(opened.id.as_str()));
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Waiting
        );
        assert!(hub.front_approval_id().is_some());
    }

    #[test]
    fn codex_hold_uses_the_setting() {
        let hub = hub();
        let (opened, _) = hub.open_approval(request("codex", "t1"), 45, Utc::now());
        assert_eq!(opened.hold, Duration::from_secs(45));
    }

    #[test]
    fn a_new_request_leaves_the_sessions_live_one_pending() {
        let hub = hub();
        let first = open(&hub, "claude-code", "s1");
        let (second, change) = hub.open_approval(request("claude-code", "s1"), 30, Utc::now());
        assert!(first.decisions.try_recv().is_err());
        assert!(change.notes.is_empty());
        let cards = lock(&hub.queue).snapshot().cards;
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[1].request_id.as_deref(), Some(second.id.as_str()));
    }

    #[test]
    fn approve_delivers_removes_the_card_and_resumes_the_session() {
        let hub = hub();
        let opened = open(&hub, "claude-code", "s1");
        let change = hub.apply_action(&opened.card.id, CardAction::Approve);
        assert_eq!(opened.decisions.recv().unwrap(), Decision::Allow);
        assert!(hub.queue_is_empty());
        assert!(change.queue);
        assert_eq!(change.notes[0].outcome, "approved in Samlu");
        assert_eq!(change.notes[0].at, opened.card.created_at);
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Working
        );
        // A second click on the same card is a quiet no-op.
        let again = hub.apply_action(&opened.card.id, CardAction::Approve);
        assert!(!again.queue && again.notes.is_empty());
    }

    #[test]
    fn deny_and_always_deliver_their_decisions() {
        let hub = hub();
        let denied = open(&hub, "codex", "t1");
        let change = hub.apply_action(&denied.card.id, CardAction::Deny);
        assert_eq!(denied.decisions.recv().unwrap(), Decision::Deny);
        assert_eq!(change.notes[0].outcome, "denied in Samlu");

        let always = open(&hub, "claude-code", "s2");
        hub.apply_action(&always.card.id, CardAction::Always);
        assert_eq!(always.decisions.recv().unwrap(), Decision::AllowAlways);
    }

    #[test]
    fn dismissing_an_approval_answers_no_decision() {
        let hub = hub();
        let opened = open(&hub, "claude-code", "s1");
        let change = hub.apply_action(&opened.card.id, CardAction::Dismiss);
        assert_eq!(opened.decisions.recv().unwrap(), Decision::NoDecision);
        assert!(hub.queue_is_empty());
        assert_eq!(change.notes[0].outcome, "handed back");
    }

    #[test]
    fn view_on_a_codex_approval_hands_it_back_and_opens_the_session() {
        let hub = hub();
        let opened = open(&hub, "codex", "t1");
        let change = hub.apply_action(&opened.card.id, CardAction::View);
        assert_eq!(opened.decisions.recv().unwrap(), Decision::NoDecision);
        assert_eq!(change.view, Some(("codex".to_string(), "t1".to_string())));
        let cards = lock(&hub.queue).snapshot().cards;
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, CardKind::HandedBack);
        assert_eq!(cards[0].id, opened.card.id);
        assert!(cards[0].request_id.is_none());
        assert_eq!(change.notes[0].outcome, "handed back");
        // Approve and Deny are dead on the handed-back card.
        let late = hub.apply_action(&opened.card.id, CardAction::Approve);
        assert!(!late.queue && late.notes.is_empty());
        assert_eq!(lock(&hub.queue).snapshot().cards.len(), 1);
    }

    #[test]
    fn view_on_a_claude_approval_keeps_it_pending() {
        let hub = hub();
        let opened = open(&hub, "claude-code", "s1");
        let change = hub.apply_action(&opened.card.id, CardAction::View);
        assert!(!change.queue);
        assert!(change.view.is_some());
        assert!(opened.decisions.try_recv().is_err());
        assert!(hub.front_approval_id().is_some());
    }

    #[test]
    fn codex_hold_elapsed_leaves_a_view_only_card_and_the_session_waiting() {
        let hub = hub();
        let opened = open(&hub, "codex", "t1");
        let change = hub.expire(&opened.id);
        assert_eq!(opened.decisions.recv().unwrap(), Decision::NoDecision);
        let cards = lock(&hub.queue).snapshot().cards;
        assert_eq!(cards[0].kind, CardKind::HandedBack);
        assert_eq!(change.notes[0].outcome, "handed back");
        assert_eq!(
            hub.sessions.get("codex", "t1").unwrap().state,
            SessionState::Waiting
        );
        assert!(hub.front_approval_id().is_none());
    }

    #[test]
    fn claude_hold_elapsed_removes_the_card() {
        let hub = hub();
        let opened = open(&hub, "claude-code", "s1");
        let change = hub.expire(&opened.id);
        assert!(hub.queue_is_empty());
        assert!(change.queue);
    }

    #[test]
    fn client_gone_removes_the_card_quietly() {
        let hub = hub();
        let opened = open(&hub, "claude-code", "s1");
        let change = hub.abandon(&opened.id, SessionState::Working);
        assert!(hub.queue_is_empty());
        assert_eq!(change.notes[0].outcome, "answered in the agent");
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Working
        );
        // Already gone: nothing more to do.
        assert!(!hub.abandon(&opened.id, SessionState::Working).queue);
    }

    #[test]
    fn tool_finished_retires_that_sessions_cards_only() {
        let hub = hub();
        let mine = open(&hub, "claude-code", "s1");
        let other = open(&hub, "claude-code", "s2");
        let change = hub.signal(&SessionSignal {
            agent_id: None,
            agent: "claude-code".to_string(),
            session_id: "s1".to_string(),
            project_path: "/p".to_string(),
            kind: SignalKind::ToolFinished,
            origin: SessionOrigin::default(),
        });
        assert_eq!(mine.decisions.recv().unwrap(), Decision::NoDecision);
        assert!(other.decisions.try_recv().is_err());
        assert!(change.queue);
        assert_eq!(change.notes[0].outcome, "answered in the agent");
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Working
        );
        assert_eq!(lock(&hub.queue).snapshot().cards.len(), 1);
    }

    #[test]
    fn session_end_removes_the_session_and_its_cards() {
        let hub = hub();
        let opened = open(&hub, "codex", "t1");
        hub.signal(&SessionSignal {
            agent_id: None,
            agent: "codex".to_string(),
            session_id: "t1".to_string(),
            project_path: String::new(),
            kind: SignalKind::Ended,
            origin: SessionOrigin::default(),
        });
        assert_eq!(opened.decisions.recv().unwrap(), Decision::NoDecision);
        assert!(hub.queue_is_empty());
        assert!(hub.sessions.get("codex", "t1").is_none());
    }

    #[test]
    fn session_start_and_prompt_set_state() {
        let hub = hub();
        let signal = |kind| SessionSignal {
            agent_id: None,
            agent: "claude-code".to_string(),
            session_id: "s1".to_string(),
            project_path: "/p".to_string(),
            kind,
            origin: SessionOrigin::default(),
        };
        let change = hub.signal(&signal(SignalKind::Started));
        assert!(!change.queue);
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Idle
        );
        hub.signal(&signal(SignalKind::Working));
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Working
        );
    }

    #[test]
    fn a_permission_notice_folds_into_a_pending_approval() {
        let hub = hub();
        assert!(!hub.has_approval_card("claude-code", "s1"));
        open(&hub, "claude-code", "s1");
        assert!(hub.has_approval_card("claude-code", "s1"));
        assert!(!hub.has_approval_card("claude-code", "s2"));
        assert!(!hub.has_approval_card("codex", "s1"));
    }

    impl AgentHub {
        fn push_notice_for_test(&self, agent: &str, session: &str) {
            lock(&self.queue).push(Card {
                id: "notice-test".to_string(),
                kind: CardKind::NeedsInput,
                agent: agent.to_string(),
                agent_label: agent_label(agent),
                session_id: session.to_string(),
                agent_id: None,
                project: String::new(),
                title: String::new(),
                detail: String::new(),
                request_id: None,
                always_allow_rule: None,
                created_at: Utc::now(),
            });
        }
    }

    fn request_with_agent_id(session: &str, agent_id: Option<&str>) -> ApprovalRequest {
        ApprovalRequest {
            agent_id: agent_id.map(str::to_string),
            ..request("claude-code", session)
        }
    }

    fn tool_finished(session: &str, agent_id: Option<&str>) -> SessionSignal {
        SessionSignal {
            agent: "claude-code".to_string(),
            session_id: session.to_string(),
            agent_id: agent_id.map(str::to_string),
            project_path: "/p".to_string(),
            kind: SignalKind::ToolFinished,
            origin: SessionOrigin::default(),
        }
    }

    #[test]
    fn parallel_subagent_approvals_do_not_retire_each_other() {
        let hub = hub();
        let (a, _) = hub.open_approval(request_with_agent_id("s1", Some("a")), 30, Utc::now());
        let (b, _) = hub.open_approval(request_with_agent_id("s1", Some("b")), 30, Utc::now());
        assert!(
            a.decisions.try_recv().is_err(),
            "B's request must not retire A"
        );
        assert_eq!(lock(&hub.queue).snapshot().cards.len(), 2);

        // B's tool finished: only B's card goes.
        let change = hub.signal(&tool_finished("s1", Some("b")));
        assert_eq!(b.decisions.try_recv().unwrap(), Decision::NoDecision);
        assert!(a.decisions.try_recv().is_err());
        assert_eq!(change.notes.len(), 1);
        assert_eq!(lock(&hub.queue).snapshot().cards.len(), 1);

        // The parent's PostToolUse carries no agent_id: A's card stays.
        hub.signal(&tool_finished("s1", None));
        assert!(a.decisions.try_recv().is_err());
        assert_eq!(lock(&hub.queue).snapshot().cards.len(), 1);
    }

    #[test]
    fn parent_tool_finished_does_not_retire_a_subagents_request_but_its_own() {
        let hub = hub();
        let (parent, _) = hub.open_approval(request_with_agent_id("s1", None), 30, Utc::now());
        let (sub, _) = hub.open_approval(request_with_agent_id("s1", Some("a")), 30, Utc::now());
        hub.signal(&tool_finished("s1", None));
        assert_eq!(parent.decisions.try_recv().unwrap(), Decision::NoDecision);
        assert!(sub.decisions.try_recv().is_err());
    }

    #[test]
    fn a_prompt_or_session_end_still_retires_the_whole_session() {
        let hub = hub();
        let (a, _) = hub.open_approval(request_with_agent_id("s1", Some("a")), 30, Utc::now());
        let (b, _) = hub.open_approval(request_with_agent_id("s1", Some("b")), 30, Utc::now());
        hub.signal(&SessionSignal {
            kind: SignalKind::Working,
            ..tool_finished("s1", None)
        });
        assert_eq!(a.decisions.try_recv().unwrap(), Decision::NoDecision);
        assert_eq!(b.decisions.try_recv().unwrap(), Decision::NoDecision);
        assert!(hub.queue_is_empty());
    }

    #[test]
    fn task_completed_and_notifications_leave_live_approvals_pending() {
        let hub = hub();
        let live = open(&hub, "claude-code", "s1");
        hub.push_notice_for_test("claude-code", "s1");
        let change = hub.retire_scoped("claude-code", "s1", Scope::Notices);
        assert!(live.decisions.try_recv().is_err());
        let cards = lock(&hub.queue).snapshot().cards;
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, CardKind::Approval);
        assert!(change.queue);
        assert!(change.notes.is_empty());
        // A turn finishing does answer it.
        hub.retire_scoped("claude-code", "s1", Scope::Session);
        assert_eq!(live.decisions.try_recv().unwrap(), Decision::NoDecision);
    }

    #[test]
    fn session_stays_waiting_while_another_card_remains() {
        let hub = hub();
        let a = open(&hub, "claude-code", "s1");
        let b = hub
            .open_approval(request("claude-code", "s1"), 30, Utc::now())
            .0;
        hub.apply_action(&a.card.id, CardAction::Approve);
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Waiting
        );
        hub.apply_action(&b.card.id, CardAction::Approve);
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Working
        );
    }

    #[test]
    fn dismissing_the_last_card_sets_the_session_idle() {
        let hub = hub();
        let opened = open(&hub, "claude-code", "s1");
        hub.apply_action(&opened.card.id, CardAction::Dismiss);
        assert_eq!(
            hub.sessions.get("claude-code", "s1").unwrap().state,
            SessionState::Idle
        );
    }

    #[test]
    fn a_new_request_clears_the_same_agents_handed_back_card_only() {
        let hub = hub();
        let first = open(&hub, "codex", "t1");
        hub.apply_action(&first.card.id, CardAction::View);
        let other = open(&hub, "codex", "t2");
        hub.open_approval(request("codex", "t1"), 30, Utc::now());
        let cards = lock(&hub.queue).snapshot().cards;
        assert_eq!(cards.len(), 2, "t1's handed-back card replaced, t2 kept");
        assert!(cards.iter().all(|c| c.kind == CardKind::Approval));
        assert!(other.decisions.try_recv().is_err());
    }

    #[test]
    fn a_notice_trailing_a_quick_answer_is_still_folded() {
        let t0 = Instant::now();
        let mut recent = RecentApprovals::default();
        assert!(!recent.recent("claude-code", "s1", t0));
        recent.record("claude-code", "s1", t0);
        assert!(recent.recent("claude-code", "s1", t0 + Duration::from_millis(1500)));
        assert!(recent.recent("claude-code", "s1", t0 + Duration::from_secs(3)));
        assert!(!recent.recent("claude-code", "s1", t0 + Duration::from_millis(3001)));
        assert!(!recent.recent("claude-code", "s2", t0));
        assert!(!recent.recent("codex", "s1", t0));
    }

    #[test]
    fn a_hidden_island_disarms_the_chords_and_the_reminder_brings_it_back() {
        assert!(shortcuts_wanted(true, false, false));
        assert!(!shortcuts_wanted(true, true, false));
        assert!(!shortcuts_wanted(false, false, false));
        assert!(
            !shortcuts_wanted(true, false, true),
            "suspended for recording"
        );
        assert_eq!(reminder_action(false, true), ReminderAction::Skip);
        assert_eq!(reminder_action(true, false), ReminderAction::Ring);
        assert_eq!(reminder_action(true, true), ReminderAction::PresentAndRing);
    }
}
