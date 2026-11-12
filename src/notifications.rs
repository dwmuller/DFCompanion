use std::collections::VecDeque;
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayMessageKind {
    User,
    Companion,
    GameEvent,
    AiUsage,
    Status,
    Warning,
    Diagnostic,
}

impl DisplayMessageKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Companion => "Companion",
            Self::GameEvent => "Game event",
            Self::AiUsage => "AI usage",
            Self::Status => "Status",
            Self::Warning => "Warning",
            Self::Diagnostic => "Diagnostic",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Companion => "companion",
            Self::GameEvent => "game-event",
            Self::AiUsage => "ai-usage",
            Self::Status => "status",
            Self::Warning => "warning",
            Self::Diagnostic => "diagnostic",
        }
    }

    pub fn is_markdown(self) -> bool {
        match self {
            Self::User | Self::Companion => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayMessage {
    pub kind: DisplayMessageKind,
    pub text: String,
    pub beep: bool,
}

impl DisplayMessage {
    pub fn new(kind: DisplayMessageKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            beep: false,
        }
    }

    pub fn with_beep(mut self, beep: bool) -> Self {
        self.beep = beep;
        self
    }
}

#[derive(Clone)]
pub struct Notifier {
    history: NotificationHistory,
}

impl Notifier {
    pub fn new(history: NotificationHistory) -> Self {
        Self { history }
    }
    /// Sends a notification and returns its assigned id.
    pub fn notify(&self, message: DisplayMessage) -> NotificationId {
        self.history.push(message)
    }
}

/// Keeps recent UI notifications so a page that reloads (for example after
/// visiting the configuration page) can replay the conversation.
#[derive(Clone)]
pub struct NotificationHistory {
    inner: std::sync::Arc<std::sync::Mutex<HistoryState>>,
    live_tx: broadcast::Sender<(u64, DisplayMessage)>,
}

#[derive(Default)]
struct HistoryState {
    entries: VecDeque<(u64, DisplayMessage)>,
    last_id: u64,
}

const NOTIFICATION_HISTORY_LIMIT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NotificationId(pub u64);

impl NotificationHistory {
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
            live_tx: broadcast::channel(128).0,
        }
    }

    fn push(&self, message: DisplayMessage) -> NotificationId {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.entries.len() == NOTIFICATION_HISTORY_LIMIT {
            inner.entries.pop_front();
        }
        inner.last_id += 1;
        let id = inner.last_id;
        inner.entries.push_back((id, message.clone()));
        // Sent under the lock so subscribers never see a gap or a duplicate.
        let _ = self.live_tx.send((id, message));
        NotificationId(id)
    }

    /// The recorded messages with an id greater than `after` (all of them when
    /// `after` is `None`) and a receiver for every message after those. Ids
    /// increase with each message, so a reconnecting browser can resume where
    /// it left off.
    pub fn subscribe(
        &self,
        after: Option<u64>,
    ) -> (
        Vec<(u64, DisplayMessage)>,
        broadcast::Receiver<(u64, DisplayMessage)>,
    ) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let backlog = inner
            .entries
            .iter()
            .filter(|(id, _)| after.map_or(true, |after| *id > after))
            .cloned()
            .collect();
        (backlog, self.live_tx.subscribe())
    }

    pub fn display_history_markdown(&self) -> String {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .entries
            .iter()
            .map(|(_, message)| {
                let label = message.kind.label();
                format!("**{label}:**\n\n{}", message.text)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
