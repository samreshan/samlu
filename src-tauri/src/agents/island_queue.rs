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

    pub fn push(&mut self, card: Card) {
        let _ = card;
    }

    pub fn remove(&mut self, card_id: &str) -> Option<Card> {
        let _ = card_id;
        None
    }

    pub fn remove_by_request(&mut self, request_id: &str) -> Option<Card> {
        let _ = request_id;
        None
    }

    pub fn remove_session(&mut self, agent: &str, session_id: &str) -> Vec<Card> {
        let _ = (agent, session_id);
        Vec::new()
    }

    pub fn replace(&mut self, card_id: &str, card: Card) -> bool {
        let _ = (card_id, card);
        false
    }

    pub fn front(&self) -> Option<&Card> {
        self.cards.get(self.front)
    }

    pub fn step(&mut self, delta: i32) {
        let _ = delta;
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
}
