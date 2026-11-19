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
        matches!(self, Self::User | Self::Companion)
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

impl InputState {
    /// A stable, lowercase identifier for use in HTML attributes.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Done => "done",
        }
    }
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

/// What a live subscriber receives: a new message, a state change to an
/// already-delivered message, or notice that the log was truncated and the
/// epoch moved on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEvent {
    Added(u64, DisplayMessage),
    StateChanged(u64, InputState),
    /// The log was truncated; this is the new epoch. Everything the
    /// subscriber has seen before this point may no longer be valid and
    /// should be discarded in favor of a fresh backlog.
    Reset(u64),
}

/// Keeps recent UI notifications so a page that reloads (for example after
/// visiting the configuration page) can replay the conversation.
#[derive(Clone)]
pub struct NotificationHistory {
    inner: std::sync::Arc<std::sync::Mutex<HistoryState>>,
    live_tx: broadcast::Sender<HistoryEvent>,
}

#[derive(Default)]
struct HistoryState {
    entries: VecDeque<(u64, DisplayMessage)>,
    last_id: u64,
    /// Incremented each time [`NotificationHistory::truncate_from`] removes
    /// entries, so stale ids from before a truncation are never mistaken for
    /// ids in the current log.
    epoch: u64,
}

const NOTIFICATION_HISTORY_LIMIT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NotificationId(pub u64);

impl std::fmt::Display for NotificationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NotificationId({})", self.0)
    }
}

/// What a caller gets back from [`NotificationHistory::subscribe_from`]: the
/// messages to replay, a receiver for what comes after them, the current
/// epoch, and whether the caller's own last-known epoch was stale (in which
/// case `backlog` is everything, not just what follows the caller's `after`).
pub struct Subscription {
    pub backlog: Vec<(u64, DisplayMessage)>,
    pub receiver: broadcast::Receiver<HistoryEvent>,
    pub epoch: u64,
    pub reset: bool,
}

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
        let _ = self.live_tx.send(HistoryEvent::Added(id, message));
        NotificationId(id)
    }

    /// Moves every AI input with an id up to and including `through` that is in
    /// state `from` to state `to`, and returns how many changed. Messages that
    /// are not AI inputs (no input state) are left alone, as are inputs in
    /// other states. Ids increase along the history, so the scan stops at the
    /// watermark. Each change is broadcast so connected browsers can update
    /// the message they already rendered instead of waiting for a reconnect.
    pub fn transition_inputs(
        &self,
        through: NotificationId,
        from: InputState,
        to: InputState,
    ) -> usize {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed_ids = Vec::new();
        for (id, message) in inner.entries.iter_mut() {
            if *id > through.0 {
                break;
            }
            if message.input_state == Some(from) {
                message.input_state = Some(to);
                changed_ids.push(*id);
            }
        }
        // Sent under the lock, same as `push`, so subscribers never see a
        // state that is already stale by the time they receive it.
        for id in &changed_ids {
            let _ = self.live_tx.send(HistoryEvent::StateChanged(*id, to));
        }
        changed_ids.len()
    }

    /// Removes every entry with an id at or after `from`, including non-AI
    /// messages, and moves to a new epoch. Returns how many entries were
    /// removed. Ids are never reused and nothing may assume the remaining
    /// ids are contiguous.
    pub fn truncate_from(&self, from: NotificationId) -> usize {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let kept = inner
            .entries
            .iter()
            .take_while(|(id, _)| *id < from.0)
            .count();
        let removed = inner.entries.len() - kept;
        inner.entries.truncate(kept);
        inner.epoch += 1;
        let epoch = inner.epoch;
        // Sent under the lock so a subscriber can never receive a message
        // from before the cut after already having been told to reset.
        let _ = self.live_tx.send(HistoryEvent::Reset(epoch));
        removed
    }

    /// The recorded messages with an id greater than `after` (all of them when
    /// `after` is `None`) and a receiver for every message after those. Ids
    /// increase with each message, so a reconnecting browser can resume where
    /// it left off. Ignores epochs: callers that also need to detect a
    /// truncation that happened while disconnected should use
    /// [`Self::subscribe_from`] instead.
    pub fn subscribe(
        &self,
        after: Option<u64>,
    ) -> (
        Vec<(u64, DisplayMessage)>,
        broadcast::Receiver<HistoryEvent>,
    ) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let backlog = inner
            .entries
            .iter()
            .filter(|(id, _)| after.is_none_or(|after| *id > after))
            .cloned()
            .collect();
        (backlog, self.live_tx.subscribe())
    }

    /// Like [`Self::subscribe`], but takes the caller's last-known `(epoch,
    /// id)` cursor. When the epoch no longer matches the current one (or the
    /// caller has none, meaning a fresh connection), the full backlog is
    /// returned and `reset` is `true`: the ids the caller remembers may no
    /// longer exist, so nothing can be resumed from them. The epoch check and
    /// backlog read happen under the same lock as pushes and truncations, so
    /// a truncation can never be missed between the two.
    pub fn subscribe_from(&self, cursor: Option<(u64, u64)>) -> Subscription {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let epoch = inner.epoch;
        let after = match cursor {
            Some((client_epoch, id)) if client_epoch == epoch => Some(id),
            _ => None,
        };
        let reset = after.is_none() && cursor.is_some();
        let backlog = inner
            .entries
            .iter()
            .filter(|(id, _)| after.is_none_or(|after| *id > after))
            .cloned()
            .collect();
        Subscription {
            backlog,
            receiver: self.live_tx.subscribe(),
            epoch,
            reset,
        }
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

impl Default for NotificationHistory {
    fn default() -> Self {
        Self::new()
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
        history.push(DisplayMessage::new(
            DisplayMessageKind::Status,
            "not an input",
        ));
        let b = input(&history, "b");
        let _c = input(&history, "c");

        let changed = history.transition_inputs(b, InputState::Pending, InputState::Processing);

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
        let changed = history.transition_inputs(b, InputState::Pending, InputState::Processing);

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

    #[test]
    fn transition_broadcasts_a_state_changed_event_per_changed_id() {
        let history = NotificationHistory::new();
        let (_, mut live) = history.subscribe(None);
        let a = input(&history, "a");
        let b = input(&history, "b");
        live.try_recv().unwrap(); // Added(a)
        live.try_recv().unwrap(); // Added(b)

        history.transition_inputs(b, InputState::Pending, InputState::Processing);

        assert_eq!(
            live.try_recv().unwrap(),
            HistoryEvent::StateChanged(a.0, InputState::Processing)
        );
        assert_eq!(
            live.try_recv().unwrap(),
            HistoryEvent::StateChanged(b.0, InputState::Processing)
        );
        assert!(live.try_recv().is_err());
    }

    #[test]
    fn truncate_from_drops_entries_at_or_after_the_cut_and_leaves_earlier_ones() {
        let history = NotificationHistory::new();
        let a = input(&history, "a");
        let b = input(&history, "b");
        let _c = input(&history, "c");

        let removed = history.truncate_from(b);

        assert_eq!(removed, 2);
        let backlog = history.subscribe(None).0;
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].0, a.0);
    }

    #[test]
    fn truncate_from_bumps_the_epoch_and_broadcasts_a_reset() {
        let history = NotificationHistory::new();
        let a = input(&history, "a");
        let (_, mut live) = history.subscribe(None);

        let removed = history.truncate_from(a);

        assert_eq!(removed, 1);
        assert_eq!(live.try_recv().unwrap(), HistoryEvent::Reset(1));
    }

    #[test]
    fn subscribe_from_resumes_within_the_same_epoch() {
        let history = NotificationHistory::new();
        let a = input(&history, "a");
        let b = input(&history, "b");

        let subscription = history.subscribe_from(Some((0, a.0)));

        assert!(!subscription.reset);
        assert_eq!(subscription.epoch, 0);
        assert_eq!(subscription.backlog.len(), 1);
        assert_eq!(subscription.backlog[0].0, b.0);
    }

    #[test]
    fn subscribe_from_resets_on_a_stale_epoch() {
        let history = NotificationHistory::new();
        let a = input(&history, "a");
        history.truncate_from(a);
        let b = input(&history, "b");

        // The caller's cursor is from epoch 0, which no longer exists.
        let subscription = history.subscribe_from(Some((0, a.0)));

        assert!(subscription.reset);
        assert_eq!(subscription.epoch, 1);
        assert_eq!(subscription.backlog.len(), 1);
        assert_eq!(subscription.backlog[0].0, b.0);
    }

    #[test]
    fn subscribe_from_with_no_cursor_is_not_a_reset() {
        let history = NotificationHistory::new();
        let _a = input(&history, "a");

        let subscription = history.subscribe_from(None);

        assert!(!subscription.reset);
        assert_eq!(subscription.backlog.len(), 1);
    }
}
