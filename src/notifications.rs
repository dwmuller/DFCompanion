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

/// Where a message that is also an input to the AI stands in the AI's queue.
///
/// Inputs are processed strictly in order from the head of the queue, so at any
/// time the `Done` inputs precede the `Processing` ones, which precede the
/// `Pending` ones. The state of a run of inputs is therefore changed by naming
/// the last notification id in the run (see
/// [`NotificationHistory::transition_inputs`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputState {
    /// Queued; not yet part of a turn sent to the AI.
    Pending,
    /// Part of the turn currently being processed.
    Processing,
    /// Consumed by a successfully completed turn.
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayMessage {
    pub kind: DisplayMessageKind,
    pub text: String,
    pub beep: bool,
    /// `None` unless this message is also an input to the AI.
    pub input_state: Option<InputState>,
}

impl DisplayMessage {
    pub fn new(kind: DisplayMessageKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            beep: false,
            input_state: None,
        }
    }

    pub fn with_beep(mut self, beep: bool) -> Self {
        self.beep = beep;
        self
    }

    /// Marks the message as an AI input in the given state.
    pub fn with_input_state(mut self, state: InputState) -> Self {
        self.input_state = Some(state);
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

    /// See [`NotificationHistory::transition_inputs`].
    pub fn transition_inputs(
        &self,
        through: NotificationId,
        from: InputState,
        to: InputState,
    ) -> usize {
        self.history.transition_inputs(through, from, to)
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

    /// Moves every AI input with an id up to and including `through` that is in
    /// state `from` to state `to`, and returns how many changed. Messages that
    /// are not AI inputs (no input state) are left alone, as are inputs in
    /// other states. Ids increase along the history, so the scan stops at the
    /// watermark.
    pub fn transition_inputs(
        &self,
        through: NotificationId,
        from: InputState,
        to: InputState,
    ) -> usize {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = 0;
        for (id, message) in inner.entries.iter_mut() {
            if *id > through.0 {
                break;
            }
            if message.input_state == Some(from) {
                message.input_state = Some(to);
                changed += 1;
            }
        }
        changed
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

#[cfg(test)]
mod tests {
    use super::*;

    fn input(history: &NotificationHistory, text: &str) -> NotificationId {
        history.push(
            DisplayMessage::new(DisplayMessageKind::User, text)
                .with_input_state(InputState::Pending),
        )
    }

    fn states(history: &NotificationHistory) -> Vec<Option<InputState>> {
        history
            .subscribe(None)
            .0
            .into_iter()
            .map(|(_, message)| message.input_state)
            .collect()
    }

    #[test]
    fn only_inputs_have_a_state() {
        let message = DisplayMessage::new(DisplayMessageKind::Status, "hi");
        assert_eq!(message.input_state, None);
        assert_eq!(
            message.with_input_state(InputState::Pending).input_state,
            Some(InputState::Pending)
        );
    }

    #[test]
    fn transition_moves_the_run_up_to_the_watermark_and_no_further() {
        let history = NotificationHistory::new();
        let a = input(&history, "a");
        history.push(DisplayMessage::new(DisplayMessageKind::Status, "not an input"));
        let b = input(&history, "b");
        let _c = input(&history, "c");

        let changed =
            history.transition_inputs(b, InputState::Pending, InputState::Processing);

        assert_eq!(changed, 2);
        assert_eq!(a, NotificationId(1));
        assert_eq!(
            states(&history),
            vec![
                Some(InputState::Processing),
                None,
                Some(InputState::Processing),
                Some(InputState::Pending),
            ]
        );
    }

    #[test]
    fn transition_only_touches_inputs_in_the_expected_state() {
        let history = NotificationHistory::new();
        let a = input(&history, "a");
        let b = input(&history, "b");
        history.transition_inputs(a, InputState::Pending, InputState::Done);

        // `a` is Done, so moving Pending inputs through `b` leaves it alone.
        let changed =
            history.transition_inputs(b, InputState::Pending, InputState::Processing);

        assert_eq!(changed, 1);
        assert_eq!(
            states(&history),
            vec![Some(InputState::Done), Some(InputState::Processing)]
        );
    }

    #[test]
    fn a_failed_turn_returns_its_run_to_pending() {
        let history = NotificationHistory::new();
        let _a = input(&history, "a");
        let b = input(&history, "b");
        history.transition_inputs(b, InputState::Pending, InputState::Processing);
        history.transition_inputs(b, InputState::Processing, InputState::Pending);
        assert_eq!(
            states(&history),
            vec![Some(InputState::Pending), Some(InputState::Pending)]
        );
    }

    #[test]
    fn transition_survives_history_trimming() {
        let history = NotificationHistory::new();
        for i in 0..NOTIFICATION_HISTORY_LIMIT + 5 {
            input(&history, &i.to_string());
        }
        let last = NotificationId((NOTIFICATION_HISTORY_LIMIT + 5) as u64);
        let changed = history.transition_inputs(last, InputState::Pending, InputState::Done);
        assert_eq!(changed, NOTIFICATION_HISTORY_LIMIT);
    }
}
