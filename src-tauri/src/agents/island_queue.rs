//! The ordered stack of cards the island shows.

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Approval,
    NeedsInput,
    HandedBack,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub kind: CardKind,
    pub agent: String,
    pub agent_label: String,
    pub session_id: String,
    pub project: String,
    pub title: String,
    pub detail: String,
    pub request_id: Option<String>,
    pub always_allow_rule: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    pub cards: Vec<Card>,
    pub front: usize,
}

pub struct IslandQueue {
    cards: Vec<Card>,
    front: usize,
}

impl Default for IslandQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl IslandQueue {
    pub fn new() -> Self {
        Self {
            cards: Vec::new(),
            front: 0,
        }
    }

    /// New cards go to the back so the one the user is reading never moves.
    pub fn push(&mut self, card: Card) {
        self.cards.push(card);
    }

    pub fn remove(&mut self, card_id: &str) -> Option<Card> {
        let pos = self.cards.iter().position(|c| c.id == card_id)?;
        Some(self.take_at(pos))
    }

    pub fn remove_by_request(&mut self, request_id: &str) -> Option<Card> {
        let pos = self
            .cards
            .iter()
            .position(|c| c.request_id.as_deref() == Some(request_id))?;
        Some(self.take_at(pos))
    }

    pub fn remove_session(&mut self, agent: &str, session_id: &str) -> Vec<Card> {
        let mut removed = Vec::new();
        while let Some(pos) = self
            .cards
            .iter()
            .position(|c| c.agent == agent && c.session_id == session_id)
        {
            removed.push(self.take_at(pos));
        }
        removed
    }

    /// Swaps a card in place, keeping its position (and so the front).
    pub fn replace(&mut self, card_id: &str, card: Card) -> bool {
        match self.cards.iter().position(|c| c.id == card_id) {
            Some(pos) => {
                self.cards[pos] = card;
                true
            }
            None => false,
        }
    }

    pub fn front(&self) -> Option<&Card> {
        self.cards.get(self.front)
    }

    /// Moves the front by `delta`, wrapping around either end.
    pub fn step(&mut self, delta: i32) {
        let len = self.cards.len() as i64;
        if len == 0 {
            return;
        }
        self.front = (self.front as i64 + delta as i64).rem_euclid(len) as usize;
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            cards: self.cards.clone(),
            front: self.front,
        }
    }

    /// Removes the card at `pos`, then keeps the same card in front: removing
    /// an earlier card shifts the index down; removing the front shows the
    /// next card (or the last one when the front was last).
    fn take_at(&mut self, pos: usize) -> Card {
        let card = self.cards.remove(pos);
        if pos < self.front {
            self.front -= 1;
        }
        if self.front >= self.cards.len() {
            self.front = self.cards.len().saturating_sub(1);
        }
        card
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, agent: &str, session: &str, request: Option<&str>) -> Card {
        Card {
            id: id.into(),
            kind: CardKind::Approval,
            agent: agent.into(),
            agent_label: agent.into(),
            session_id: session.into(),
            project: "p".into(),
            title: "t".into(),
            detail: "d".into(),
            request_id: request.map(String::from),
            always_allow_rule: None,
            created_at: Utc::now(),
        }
    }

    fn c(id: &str) -> Card {
        card(id, "claude-code", "s", None)
    }

    fn front_id(q: &IslandQueue) -> String {
        q.front().unwrap().id.clone()
    }

    #[test]
    fn push_keeps_oldest_first_and_front_stays_put() {
        let mut q = IslandQueue::new();
        q.push(c("a"));
        q.push(c("b"));
        assert_eq!(front_id(&q), "a");
        q.push(c("c"));
        assert_eq!(front_id(&q), "a");
        assert_eq!(q.snapshot().cards.len(), 3);
    }

    #[test]
    fn remove_front_shows_next() {
        let mut q = IslandQueue::new();
        for i in ["a", "b", "c"] {
            q.push(c(i));
        }
        assert!(q.remove("a").is_some());
        assert_eq!(front_id(&q), "b");
    }

    #[test]
    fn remove_last_front_shows_new_last() {
        let mut q = IslandQueue::new();
        for i in ["a", "b"] {
            q.push(c(i));
        }
        q.step(1);
        q.remove("b");
        assert_eq!(front_id(&q), "a");
        q.remove("a");
        assert!(q.front().is_none());
        assert_eq!(q.snapshot().front, 0);
    }

    #[test]
    fn remove_before_front_keeps_same_card_in_front() {
        let mut q = IslandQueue::new();
        for i in ["a", "b", "c"] {
            q.push(c(i));
        }
        q.step(1);
        assert_eq!(front_id(&q), "b");
        q.remove("a");
        assert_eq!(front_id(&q), "b");
    }

    #[test]
    fn step_wraps() {
        let mut q = IslandQueue::new();
        q.push(c("a"));
        q.push(c("b"));
        q.step(1);
        assert_eq!(front_id(&q), "b");
        q.step(1);
        assert_eq!(front_id(&q), "a");
        q.step(-1);
        assert_eq!(front_id(&q), "b");
        IslandQueue::new().step(1);
    }

    #[test]
    fn remove_session_and_by_request() {
        let mut q = IslandQueue::new();
        q.push(card("a", "claude-code", "s1", Some("r1")));
        q.push(card("b", "claude-code", "s2", Some("r2")));
        q.push(card("c", "claude-code", "s1", None));
        q.push(card("d", "codex", "s1", None));
        assert_eq!(q.remove_by_request("r2").unwrap().id, "b");
        assert!(q.remove_by_request("r2").is_none());
        let gone: Vec<String> = q
            .remove_session("claude-code", "s1")
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(gone, vec!["a", "c"]);
        assert_eq!(q.snapshot().cards.len(), 1);
        assert_eq!(front_id(&q), "d");
    }

    #[test]
    fn replace_keeps_position() {
        let mut q = IslandQueue::new();
        q.push(c("a"));
        q.push(c("b"));
        q.step(1);
        let mut n = c("b2");
        n.kind = CardKind::HandedBack;
        assert!(q.replace("b", n));
        assert_eq!(front_id(&q), "b2");
        assert!(!q.replace("zzz", c("x")));
    }

    #[test]
    fn snapshot_front_index_clamped_when_empty() {
        assert_eq!(IslandQueue::new().snapshot().front, 0);
    }
}
