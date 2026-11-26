use serde::Serialize;
use tokio::sync::mpsc::{self, error::TryRecvError, error::TrySendError};

use crate::{
    ai::History,
    game_state::{validate_game_date, LifecycleSnapshot},
};

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

/// Events from external sources such as user input and game events, which are
/// intended for processing by the companion AI.
#[derive(Clone, Debug, PartialEq)]
pub enum ExternalEvent {
    UserInput(String),
    /// An AI configuration was just selected: give the AI a turn to look around.
    Orient,
    GameEvent(GameEvent),
    /// A high-priority observation of the game's loading state.
    Lifecycle(LifecycleEvent),
    /// Completion of a site-bound chat restore operation.
    ChatStateLoaded {
        generation: u64,
        site_id: i64,
        result: Result<Option<LoadedChatState>, String>,
    },
    /// Lifecycle recheck associated with a failed restore.
    ChatRestoreLifecycle {
        generation: u64,
        site_id: i64,
        snapshot: LifecycleSnapshot,
    },
    /// A committed history snapshot that should be coalesced with other
    /// pending saves and written to the current fortress.
    ChatStateSaveRequested(ChatSaveRequest),
    /// Completion of one serialized site-persistent save.
    ChatStateSaved {
        save_id: u64,
        generation: u64,
        site_id: i64,
        result: Result<(), String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleEvent {
    Snapshot(LifecycleSnapshot),
    Unreachable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedChatState {
    pub state: String,
    pub current_game_date: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatSaveRequest {
    pub generation: u64,
    pub site_id: i64,
    pub history: History,
}

/// Receives normal events in bounded FIFO order and control events (lifecycle
/// and persistence) on a priority channel.
pub struct EventReceiver {
    pub(crate) normal: mpsc::Receiver<ExternalEvent>,
    pub(crate) lifecycle: mpsc::UnboundedReceiver<ExternalEvent>,
}

impl EventReceiver {
    pub async fn recv(&mut self) -> Option<ExternalEvent> {
        let lifecycle_open = !self.lifecycle.is_closed();
        tokio::select! {
            biased;
            item = self.lifecycle.recv(), if lifecycle_open => {
                if let Some(item) = item {
                    Some(item)
                } else {
                    self.normal.recv().await
                }
            },
            item = self.normal.recv() => item,
        }
    }

    pub fn try_recv(&mut self) -> Result<ExternalEvent, TryRecvError> {
        match self.lifecycle.try_recv() {
            Ok(event) => Ok(event),
            Err(TryRecvError::Empty) => self.normal.try_recv(),
            Err(error) => Err(error),
        }
    }

    pub fn len(&self) -> usize {
        self.normal.len() + self.lifecycle.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn drain_normal(&mut self) -> usize {
        let mut dropped = 0;
        while self.normal.try_recv().is_ok() {
            dropped += 1;
        }
        dropped
    }
}

#[derive(Clone)]
pub struct EventQueue {
    queue_tx: mpsc::Sender<ExternalEvent>,
    lifecycle_tx: mpsc::UnboundedSender<ExternalEvent>,
}

impl EventQueue {
    pub fn new(
        queue_tx: mpsc::Sender<ExternalEvent>,
        lifecycle_tx: mpsc::UnboundedSender<ExternalEvent>,
    ) -> Self {
        Self {
            queue_tx,
            lifecycle_tx,
        }
    }
    pub fn try_send(&self, item: ExternalEvent) -> Result<(), String> {
        match item {
            ExternalEvent::Lifecycle(event) => self.try_send_lifecycle(event),
            item @ (ExternalEvent::ChatStateLoaded { .. }
            | ExternalEvent::ChatRestoreLifecycle { .. }
            | ExternalEvent::ChatStateSaveRequested(_)
            | ExternalEvent::ChatStateSaved { .. }) => self.try_send_priority(item),
            item => self.try_send_normal(item),
        }
    }

    fn try_send_normal(&self, item: ExternalEvent) -> Result<(), String> {
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

    pub fn capacity_remaining(&self) -> usize {
        self.queue_tx.capacity()
    }

    /// Enqueues a lifecycle observation without sharing the capacity of the
    /// bounded user/report queue.
    pub fn try_send_lifecycle(&self, event: LifecycleEvent) -> Result<(), String> {
        self.lifecycle_tx
            .send(ExternalEvent::Lifecycle(event))
            .map_err(|_| "The lifecycle event queue is closed, failed to send.".to_owned())
    }

    pub(crate) fn try_send_chat_state_loaded(
        &self,
        generation: u64,
        site_id: i64,
        result: Result<Option<LoadedChatState>, String>,
    ) -> Result<(), String> {
        self.try_send_priority(ExternalEvent::ChatStateLoaded {
            generation,
            site_id,
            result,
        })
    }

    pub(crate) fn try_send_chat_restore_lifecycle(
        &self,
        generation: u64,
        site_id: i64,
        snapshot: LifecycleSnapshot,
    ) -> Result<(), String> {
        self.try_send_priority(ExternalEvent::ChatRestoreLifecycle {
            generation,
            site_id,
            snapshot,
        })
    }

    pub(crate) fn try_send_chat_state_save_requested(
        &self,
        request: ChatSaveRequest,
    ) -> Result<(), String> {
        self.try_send_priority(ExternalEvent::ChatStateSaveRequested(request))
    }

    pub(crate) fn try_send_chat_state_saved(
        &self,
        save_id: u64,
        generation: u64,
        site_id: i64,
        result: Result<(), String>,
    ) -> Result<(), String> {
        self.try_send_priority(ExternalEvent::ChatStateSaved {
            save_id,
            generation,
            site_id,
            result,
        })
    }

    fn try_send_priority(&self, item: ExternalEvent) -> Result<(), String> {
        self.lifecycle_tx
            .send(item)
            .map_err(|_| "The priority event queue is closed, failed to send.".to_owned())
    }
}
