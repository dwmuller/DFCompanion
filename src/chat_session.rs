//! The AI conversation as a state machine.
//!
//! A [`Session`] owns everything that decides what the AI is asked and when:
//! the queue of inputs waiting for the AI, the conversation history, the turn
//! in flight, whether processing is blocked, and the timers for automatic
//! turns. It performs no I/O and reads no clock. The worker feeds it
//! [`Event`]s along with a [`Context`] describing the outside world, and
//! carries out the [`Effect`]s it returns. That keeps every interleaving of
//! player input, game events, timers and turn results testable without tasks,
//! timers or a provider.

use std::{collections::VecDeque, time::Duration};

use itertools::Itertools;
use name_variant::NamedVariant;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::{
    ai::{
        build_prompt, is_no_comment, should_compact, AiInput, CompactionOutput, History, Prompt,
        TurnError, TurnOutput,
    },
    ai_config::AiConfiguration,
    events::GameEvent,
    notifications::{DisplayMessage, DisplayMessageKind, InputState, NotificationId},
};

/// How often the AI gets a turn on its own while the game is running.
pub const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// How long a game event may wait before the AI gets a turn that includes it.
pub const EVENT_FLUSH_DELAY: Duration = Duration::from_secs(5);
/// How many completed turns are kept as rollback checkpoints.
const MAX_TURN_RECORDS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatState {
    Idle,
    Processing,
    Blocked,
}

/// What the session needs to know about the world to handle an event.
#[derive(Clone, Debug)]
pub struct Context {
    pub now: Instant,
    /// The active AI configuration, if there is one.
    pub configuration: Result<Option<AiConfiguration>, String>,
    /// Whether the AI gets a turn shortly after game events arrive.
    pub event_turns: bool,
    /// Whether the AI gets a turn at regular intervals while the game runs.
    pub periodic_turns: bool,
}

impl Context {
    fn has_configuration(&self) -> bool {
        matches!(self.configuration, Ok(Some(_)))
    }
}

/// Inputs to the chat session, representing events that the session needs to
/// handle.
#[derive(Debug, NamedVariant)]
pub enum ChatEvents {
    /// Something to ask the AI, which gets a turn right away. `id` is the id of
    /// the notification that displays it, if any.
    UserRequest {
        text: String,
        id: Option<NotificationId>,
    },
    /// A game report for the AI. It waits for a turn started by something else
    /// or by the flush timer. `id` is the id of the notification that displays
    /// it, if any.
    Report {
        event: GameEvent,
        id: Option<NotificationId>,
    },
    /// The time to flush game reports arrived. See [`Session::flush_deadline`].
    FlushDue,
    /// The time for a periodic check arrived. See [`Session::check_deadline`].
    CheckDue,
    /// The answer to [`Effect::QueryGameRunning`]; `None` if the game could not
    /// be reached.
    GameRunning(Option<bool>),
    /// A turn started by [`Effect::StartTurn`] ended.
    TurnFinished {
        turn_id: u64,
        result: Result<TurnOutput, TurnError>,
    },
    /// A compaction started by [`Effect::StartCompaction`] ended.
    /// `compaction_id` must match the one the effect carried, or this is a
    /// stale result and is ignored.
    CompactionFinished {
        compaction_id: u64,
        result: Result<CompactionOutput, TurnError>,
    },
    /// Cancel the turn in flight, if there is one. Its inputs go back to pending.
    CancelTurn,
    /// The player asked to resume after processing was blocked.
    Resume,
    /// Roll the conversation back to just before the turn containing `target`.
    /// The AI's history is restored to what it was before that turn, every
    /// input from that turn onward (user requests and game reports, processed or
    /// still queued) is dropped, and the log is truncated at the turn's first
    /// notification. The text of the dropped user requests is prepended to
    /// `editor_text` in the reply. `target` is the notification id of a user
    /// input; if it is no longer eligible, the reply is an error.
    Rollback {
        target: NotificationId,
        editor_text: String,
    },
}

#[derive(Debug)]
pub enum Effect {
    Notify(DisplayMessage),
    /// Change the state of the inputs up to and including `through`.
    TransitionInputs {
        through: NotificationId,
        from: InputState,
        to: InputState,
    },
    /// Run a turn. Report its end with [`Event::TurnFinished`] and the same
    /// `turn_id`. The turn is cancelled through `cancel`.
    StartTurn {
        turn_id: u64,
        configuration: AiConfiguration,
        history: History,
        prompt: Prompt,
        cancel: CancellationToken,
    },
    /// Replace older history with an AI-written summary. Report its end with
    /// [`Event::CompactionFinished`] and the same `compaction_id`. Cancelled
    /// through `cancel`.
    StartCompaction {
        compaction_id: u64,
        configuration: AiConfiguration,
        history: History,
        cancel: CancellationToken,
    },
    /// Find out whether the game is running and report it with
    /// [`Event::GameRunning`].
    QueryGameRunning,
    /// AI processing just became blocked: warn the player, and pause the game.
    Block {
        reason: String,
    },
    /// Show the chat state to the player.
    PublishChatState(ChatState),
    /// Show the rollback floor to the player: the id of the oldest
    /// notification still eligible for a rollback, if any.
    PublishRollbackFloor(Option<NotificationId>),
    /// Remove every log entry at or after this id. Irrecoverable.
    TruncateLog(NotificationId),
    /// Show the combined editor text in the reply to a rollback, or an error
    /// if the rollback could not be performed (for example, a stale target).
    ReplyEditorText(Result<String, String>),
}

struct InFlight {
    turn_id: u64,
    /// The inputs the turn consumes are the first `prompt.included_inputs` of
    /// the queue; they stay there until the turn succeeds.
    prompt: Prompt,
    cancel: CancellationToken,
}

/// A history-compaction job in flight. At most one of this and [`InFlight`]
/// exists at a time: compaction gates starting a turn, and a turn gates
/// starting compaction.
struct InFlightCompaction {
    compaction_id: u64,
    cancel: CancellationToken,
}

/// A completed turn, retained so its inputs' user text can be recovered if the
/// conversation is rolled back to before it.
struct TurnRecord {
    inputs: Vec<AiInput>,
    history_before: History,
}

impl TurnRecord {
    /// Whether this turn included the input with notification id `id`.
    fn contains(&self, id: NotificationId) -> bool {
        self.inputs
            .iter()
            .any(|input| input.notification_id() == Some(id))
    }

    /// The first notification id among this turn's inputs, if any.
    fn first_notification(&self) -> Option<NotificationId> {
        self.inputs.iter().find_map(AiInput::notification_id)
    }
}

/// The session state for a conversation, including the input queue, history,
/// completed turns, and in-flight operations.
///
/// Decides when when AI runs, tracks turns for rollback, and manages history
/// compaction. Uses a turn-dispatch pattern to facilitate testing.
pub struct ChatSession {
    /// Inputs for the AI, oldest first.
    queue: VecDeque<AiInput>,
    history: History,
    /// Completed turns, oldest first, for rollback. At most
    /// [`MAX_TURN_RECORDS`].
    records: VecDeque<TurnRecord>,
    in_flight: Option<InFlight>,
    /// A compaction job in flight, if any.
    in_flight_compaction: Option<InFlightCompaction>,
    blocked: bool,
    /// Whether the next turn should carry the "the conversation was rolled
    /// back" note. Cleared once a turn that included it succeeds.
    rolled_back_note_due: bool,
    next_turn_id: u64,
    next_compaction_id: u64,
    flush_at: Option<Instant>,
    next_check: Instant,
    /// The last chat state sent out in an [`Effect::PublishChatState`].
    published: ChatState,
    /// The last rollback floor sent out in an
    /// [`Effect::PublishRollbackFloor`].
    published_floor: Option<NotificationId>,
}

fn failure_message(error: &str) -> String {
    format!(
        "AI request failed. Your chat and collected reports are retained; AI processing is paused until you resume. {error}"
    )
}

impl ChatSession {
    pub fn new(now: Instant) -> Self {
        Self {
            queue: VecDeque::new(),
            history: History::default(),
            records: VecDeque::new(),
            in_flight: None,
            in_flight_compaction: None,
            blocked: false,
            rolled_back_note_due: false,
            next_turn_id: 0,
            next_compaction_id: 0,
            flush_at: None,
            next_check: now + PERIODIC_CHECK_INTERVAL,
            published: ChatState::Idle,
            published_floor: None,
        }
    }

    pub fn chat_state(&self) -> ChatState {
        if self.blocked {
            ChatState::Blocked
        } else if self.in_flight.is_some() {
            ChatState::Processing
        } else {
            ChatState::Idle
        }
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// When to send [`Event::FlushDue`], if a flush is waiting.
    pub fn flush_deadline(&self) -> Option<Instant> {
        self.flush_at
    }

    /// When to send [`Event::CheckDue`].
    pub fn check_deadline(&self) -> Instant {
        self.next_check
    }

    pub fn pending_report_count(&self) -> usize {
        self.queue
            .iter()
            .filter(|i| matches!(i, AiInput::GameReport { .. }))
            .count()
    }

    pub fn pending_user_message_count(&self) -> usize {
        self.queue
            .iter()
            .filter(|i| matches!(i, AiInput::UserRequest { .. }))
            .count()
    }

    /// The id of the newest queued input that has a notification. Everything
    /// queued with a notification is at or before it.
    pub fn pending_through(&self) -> Option<NotificationId> {
        self.queue.iter().rev().find_map(AiInput::notification_id)
    }

    /// The id of the oldest notification still eligible for a rollback: the
    /// first notification covered by a retained checkpoint, or, if no
    /// completed turn has a notification yet, the first still-queued input.
    /// `None` when there is nothing eligible to roll back to.
    pub fn rollback_floor(&self) -> Option<NotificationId> {
        self.records
            .iter()
            .find_map(TurnRecord::first_notification)
            .or_else(|| self.queue.iter().find_map(AiInput::notification_id))
    }

    /// Handles an event and returns what the caller has to do about it, in order.
    pub fn handle(&mut self, context: &Context, event: ChatEvents) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.step(context, event, &mut effects);
        let state = self.chat_state();
        if state != self.published {
            self.published = state;
            effects.push(Effect::PublishChatState(state));
        }
        let floor = self.rollback_floor();
        if floor != self.published_floor {
            self.published_floor = floor;
            effects.push(Effect::PublishRollbackFloor(floor));
        }
        effects
    }

    #[instrument(level = "debug", skip_all, fields(event = event.variant_name()))]
    fn step(&mut self, context: &Context, event: ChatEvents, effects: &mut Vec<Effect>) {
        match event {
            ChatEvents::UserRequest { text, id } => {
                tracing::trace!(
                    "Handling user request with id {}",
                    id.map_or("<none>".to_string(), |id| id.to_string())
                );
                self.queue.push_back(AiInput::UserRequest { text, id });
                self.flush_at = None;
                self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                self.try_start_turn(context, effects);
            }
            ChatEvents::Report { event, id } => {
                tracing::trace!(
                    "Handling game report with id {}",
                    id.map_or("<none>".to_string(), |id| id.to_string())
                );
                self.queue.push_back(AiInput::GameReport { event, id });
                self.flush_at.get_or_insert(context.now + EVENT_FLUSH_DELAY);
            }
            ChatEvents::FlushDue => {
                self.flush_at = None;
                if context.event_turns && !self.blocked && context.has_configuration() {
                    self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                    self.try_start_turn(context, effects);
                }
            }
            ChatEvents::CheckDue => {
                self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                if context.periodic_turns && !self.blocked && context.has_configuration() {
                    effects.push(Effect::QueryGameRunning);
                }
            }
            ChatEvents::GameRunning(running) => {
                tracing::trace!("Handling game running event: {running:?}");
                if running == Some(true)
                    && context.periodic_turns
                    && !self.blocked
                    && context.has_configuration()
                {
                    self.flush_at = None;
                    self.try_start_turn(context, effects);
                }
            }
            ChatEvents::TurnFinished { turn_id, result } => {
                tracing::trace!("Handling turn finished event: {turn_id:?}");
                tracing::trace!("Turn finished with result: {result:?}");
                self.finish_turn(context, turn_id, result, effects)
            }
            ChatEvents::CompactionFinished {
                compaction_id,
                result,
            } => {
                tracing::trace!(
                    "Handling compaction finished event: {compaction_id:?}, {result:?}"
                );
                self.finish_compaction(context, compaction_id, result, effects)
            }
            ChatEvents::CancelTurn => {
                if let Some(in_flight) = &self.in_flight {
                    in_flight.cancel.cancel();
                }
            }
            ChatEvents::Resume => {
                self.blocked = false;
            }
            ChatEvents::Rollback {
                target,
                editor_text,
            } => {
                tracing::trace!("Handling rollback event: {target:?}");
                self.rollback(context, target, editor_text, effects);
            }
        }
    }
}

impl ChatSession {
    /// Stops processing until the player resumes it, and says why.
    fn block(&mut self, reason: String, effects: &mut Vec<Effect>) {
        if !self.blocked {
            self.blocked = true;
            effects.push(Effect::Block { reason });
        }
    }

    /// Starts a turn if nothing prevents it: processing is not blocked, no turn
    /// is in flight, and there is something to send.
    fn try_start_turn(&mut self, context: &Context, effects: &mut Vec<Effect>) {
        if self.blocked || self.in_flight.is_some() || self.in_flight_compaction.is_some() {
            return;
        }
        let configuration = match &context.configuration {
            Ok(Some(configuration)) => configuration.clone(),
            Ok(None) => {
                self.block("No active AI configuration.".to_owned(), effects);
                return;
            }
            Err(error) => {
                self.block(failure_message(error), effects);
                return;
            }
        };
        if should_compact(&self.history) {
            self.start_compaction(configuration, effects);
            return;
        }
        let Some(prompt) = build_prompt(
            &self.history,
            &self.queue,
            &configuration.system_text,
            self.rolled_back_note_due,
        ) else {
            return;
        };
        let turn_id = self.next_turn_id;
        self.next_turn_id += 1;
        if let Some(through) = prompt.through {
            effects.push(Effect::TransitionInputs {
                through,
                from: InputState::Pending,
                to: InputState::Processing,
            });
        }
        effects.push(Effect::Notify(DisplayMessage::new(
            DisplayMessageKind::AiUsage,
            prompt.token_estimates_summary(),
        )));
        let cancel = CancellationToken::new();
        self.in_flight = Some(InFlight {
            turn_id,
            prompt: prompt.clone(),
            cancel: cancel.clone(),
        });
        effects.push(Effect::StartTurn {
            turn_id,
            configuration,
            history: self.history.clone(),
            prompt,
            cancel,
        });
    }

    /// Starts a compaction job that reports back with
    /// [`Event::CompactionFinished`].
    fn start_compaction(&mut self, configuration: AiConfiguration, effects: &mut Vec<Effect>) {
        let compaction_id = self.next_compaction_id;
        self.next_compaction_id += 1;
        let cancel = CancellationToken::new();
        self.in_flight_compaction = Some(InFlightCompaction {
            compaction_id,
            cancel: cancel.clone(),
        });
        effects.push(Effect::StartCompaction {
            compaction_id,
            configuration,
            history: self.history.clone(),
            cancel,
        });
    }

    fn finish_turn(
        &mut self,
        context: &Context,
        turn_id: u64,
        result: Result<TurnOutput, TurnError>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(InFlight { prompt, .. }) = self.in_flight.take_if(|f| f.turn_id == turn_id) else {
            return; // Not the turn in flight: a stale report.
        };
        let finish_inputs = |to: InputState| {
            prompt.through.map(|through| Effect::TransitionInputs {
                through,
                from: InputState::Processing,
                to,
            })
        };
        match result {
            Ok(TurnOutput {
                history,
                answer,
                usage,
            }) => {
                let is_user_interaction = prompt.kind.is_user_interaction();
                let inputs = self.queue.drain(..prompt.included_inputs).collect();
                let history_before = std::mem::replace(&mut self.history, history);
                self.records.push_back(TurnRecord {
                    inputs,
                    history_before,
                });
                while self.records.len() > MAX_TURN_RECORDS {
                    self.records.pop_front();
                }
                if prompt.included_rollback_note {
                    self.rolled_back_note_due = false;
                }
                if is_user_interaction || !is_no_comment(&answer) {
                    effects.push(Effect::Notify(
                        DisplayMessage::new(DisplayMessageKind::Companion, answer)
                            .with_beep(!is_user_interaction),
                    ));
                }
                effects.push(Effect::Notify(DisplayMessage::new(
                    DisplayMessageKind::AiUsage,
                    usage.iter().map(|u| u.summary()).join("\n"),
                )));
                effects.extend(finish_inputs(InputState::Done));
                if !self.queue.is_empty() {
                    self.try_start_turn(context, effects);
                }
            }
            Err(TurnError::Failed(error)) => {
                effects.extend(finish_inputs(InputState::Pending));
                self.block(failure_message(&error), effects);
            }
            Err(TurnError::Cancelled) => {
                effects.extend(finish_inputs(InputState::Pending));
            }
        }
    }

    /// Applies the result of a compaction. A stale result, whose id no longer
    /// matches the job in flight (for example after a rollback), is ignored.
    fn finish_compaction(
        &mut self,
        context: &Context,
        compaction_id: u64,
        result: Result<CompactionOutput, TurnError>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(_) = self
            .in_flight_compaction
            .take_if(|c| c.compaction_id == compaction_id)
        else {
            return;
        };
        match result {
            Ok(output) => {
                self.history = output.history;
                if !output.usage.is_empty() {
                    effects.push(Effect::Notify(DisplayMessage::new(
                        DisplayMessageKind::AiUsage,
                        output.usage.iter().map(|u| u.summary()).join("\n"),
                    )));
                }
                effects.push(Effect::Notify(DisplayMessage::new(
                    DisplayMessageKind::Status,
                    "Conversation history was compacted to stay within the AI context budget.",
                )));
                self.try_start_turn(context, effects);
            }
            Err(TurnError::Failed(error)) => {
                self.block(format!("AI history compaction failed. {error}"), effects);
            }
            Err(TurnError::Cancelled) => {}
        }
    }

    /// Rolls the conversation back to just before the turn containing
    /// `target`. See [`Event::Rollback`].
    fn rollback(
        &mut self,
        context: &Context,
        target: NotificationId,
        editor_text: String,
        effects: &mut Vec<Effect>,
    ) {
        // A compaction in flight captured the pre-rollback history. Cancel it
        // so a late result cannot overwrite the history being restored.
        if let Some(compaction) = self.in_flight_compaction.take() {
            compaction.cancel.cancel();
        }
        // The inputs to drop, and the id to truncate the log from. `None` when
        // the target is not eligible.
        let dropped: Option<(Option<NotificationId>, Vec<AiInput>)> = if let Some(index) = self
            .records
            .iter()
            .position(|record| record.contains(target))
        {
            // The target is in a completed turn: restore history to before it
            // and drop it and everything newer, committing nothing new. Any
            // turn in flight postdates it and is cancelled.
            if let Some(in_flight) = self.in_flight.take() {
                in_flight.cancel.cancel();
            }
            let dropped_records = self.records.split_off(index);
            self.history = dropped_records[0].history_before.clone();
            let mut inputs: Vec<AiInput> = dropped_records
                .iter()
                .flat_map(|record| record.inputs.iter().cloned())
                .collect();
            inputs.extend(self.queue.drain(..));
            Some((dropped_records[0].first_notification(), inputs))
        } else if let Some(position) = self
            .queue
            .iter()
            .position(|input| input.notification_id() == Some(target))
        {
            // The target is still queued. History is untouched. If it is part
            // of the batch in flight, that turn is cancelled and the whole
            // queue goes; otherwise the turn in flight is left alone.
            let in_flight_batch = self
                .in_flight
                .as_ref()
                .is_some_and(|f| position < f.prompt.included_inputs);
            let cut = if in_flight_batch {
                if let Some(in_flight) = self.in_flight.take() {
                    in_flight.cancel.cancel();
                }
                0
            } else {
                position
            };
            let inputs: Vec<AiInput> = self.queue.split_off(cut).into_iter().collect();
            let first_notification = inputs.iter().find_map(AiInput::notification_id);
            Some((first_notification, inputs))
        } else {
            None
        };

        let Some((first_notification, inputs)) = dropped else {
            effects.push(Effect::ReplyEditorText(Err(
                "This message is too old to roll back, or has already been rolled back.".to_owned(),
            )));
            return;
        };

        if let Some(from) = first_notification {
            effects.push(Effect::TruncateLog(from));
        }
        effects.push(Effect::ReplyEditorText(Ok(combine_editor_text(
            &inputs,
            &editor_text,
        ))));
        effects.push(Effect::Notify(DisplayMessage::new(
            DisplayMessageKind::Status,
            "Conversation rolled back. This cannot be undone.",
        )));
        self.rolled_back_note_due = true;
        self.blocked = false;
        if !self
            .queue
            .iter()
            .any(|input| matches!(input, AiInput::GameReport { .. }))
        {
            self.flush_at = None;
        }
        if !self.queue.is_empty() {
            self.try_start_turn(context, effects);
        }
    }
}

/// The text of the dropped user requests, in order, ahead of `editor_text`,
/// separated by two blank lines. Hidden inputs (those with no notification) are
/// not recovered.
fn combine_editor_text(inputs: &[AiInput], editor_text: &str) -> String {
    let dropped = inputs
        .iter()
        .filter_map(|input| match input {
            AiInput::UserRequest { text, id: Some(_) } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n\n");
    match (dropped.is_empty(), editor_text.is_empty()) {
        (true, _) => editor_text.to_owned(),
        (false, true) => dropped,
        (false, false) => format!("{dropped}\n\n\n{editor_text}"),
    }
}

#[cfg(test)]
mod tests;
