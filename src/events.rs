use serde::Serialize;
use tokio::sync::mpsc::{self, error::TrySendError};

use crate::game_state::validate_game_date;

/// A game event in the format of the `get_recent_reports` tool.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GameEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Game date, `YYYY-MM-DD:T`.
    pub date: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub report_type: Option<String>,
    pub category: String,
    pub text: String,
}

impl GameEvent {
    /// Checks the fields a sender controls.
    pub fn validate(&self) -> Result<(), String> {
        let category = self.category.trim();
        if category.is_empty() || category.len() > 128 {
            return Err("Category must contain between 1 and 128 bytes.".to_owned());
        }
        if self.report_type.as_ref().is_some_and(|t| t.len() > 128) {
            return Err("Type must not exceed 128 bytes.".to_owned());
        }
        if self.text.trim().is_empty() {
            return Err("Event text is required.".to_owned());
        }
        if self.text.len() > 8_000 {
            return Err("Event text exceeds the 8000-byte limit.".to_owned());
        }
        validate_game_date(&self.date).map_err(|error| format!("Invalid date: {error}"))
    }
}

/// Items that can be queued for processing by the companion AI.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueItem {
    UserInput(String),
    AiRetry,
    /// An AI configuration was just selected: give the AI a turn to look around.
    Orient,
    GameEvent(GameEvent),
}

#[derive(Clone)]
pub struct EventQueue {
    queue_tx: mpsc::Sender<QueueItem>,
}

impl EventQueue {
    pub fn new(queue_tx: mpsc::Sender<QueueItem>) -> Self {
        Self { queue_tx }
    }
    pub fn try_send(&self, item: QueueItem) -> Result<(), String> {
        match self.queue_tx.try_send(item) {
            Ok(_) => Ok(()),
            Err(TrySendError::Full(_)) => Err(format!(
                "The event queue is full (capacity {}), failed to send.",
                self.queue_tx.max_capacity()
            )),
            Err(TrySendError::Closed(_)) => {
                Err("The event queue is closed, failed to send.".to_owned())
            }
        }
    }
}
