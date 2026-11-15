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
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    ai::{build_prompt, is_no_comment, AiInput, History, Prompt, TurnError, TurnOutput},
    ai_config::AiConfiguration,
    events::GameEvent,
    notifications::{DisplayMessage, DisplayMessageKind, InputState, NotificationId},
};

/// How often the AI gets a turn on its own while the game is running.
pub const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// How long a game event may wait before the AI gets a turn that includes it.
pub const EVENT_FLUSH_DELAY: Duration = Duration::from_secs(5);

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

#[derive(Debug)]
pub enum Event {
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
    /// The player asked to try again.
    Retry,
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
    /// Cancel the turn in flight, if there is one. Its inputs go back to pending.
    CancelTurn,
    /// The player asked to resume after processing was blocked.
    Resume,
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
}

struct InFlight {
    turn_id: u64,
    /// The inputs the turn consumes are the first `prompt.included_inputs` of
    /// the queue; they stay there until the turn succeeds.
    prompt: Prompt,
    cancel: CancellationToken,
}

pub struct Session {
    /// Inputs for the AI, oldest first.
    queue: VecDeque<AiInput>,
    history: History,
    in_flight: Option<InFlight>,
    blocked: bool,
    next_turn_id: u64,
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
        "AI request failed. Your chat and collected reports are retained; AI processing is paused until you send a message or retry. {error}"
    )
}

impl Session {
    pub fn new(now: Instant) -> Self {
        Self {
            queue: VecDeque::new(),
            history: History::default(),
            in_flight: None,
            blocked: false,
            next_turn_id: 0,
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

    /// The id of the oldest notification still eligible for a rollback:
    /// everything from here on is still queued (Pending or, if in flight,
    /// Processing), so no history has been committed past it and no turn has
    /// completed. `None` once nothing is queued, since without checkpoints
    /// (added in a later stage) there is nothing earlier left to roll back
    /// to.
    pub fn rollback_floor(&self) -> Option<NotificationId> {
        self.queue.iter().find_map(AiInput::notification_id)
    }

    /// Handles an event and returns what the caller has to do about it, in order.
    pub fn handle(&mut self, context: &Context, event: Event) -> Vec<Effect> {
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

    fn step(&mut self, context: &Context, event: Event, effects: &mut Vec<Effect>) {
        match event {
            Event::UserRequest { text, id } => {
                self.queue.push_back(AiInput::UserRequest { text, id });
                self.flush_at = None;
                self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                self.try_start_turn(context, effects);
            }
            Event::Report { event, id } => {
                self.queue.push_back(AiInput::GameReport { event, id });
                self.flush_at.get_or_insert(context.now + EVENT_FLUSH_DELAY);
            }
            Event::Retry => {
                if self.pending_user_message_count() == 0 {
                    effects.push(Effect::Notify(DisplayMessage::new(
                        DisplayMessageKind::Status,
                        "There is no failed AI request to retry.",
                    )));
                    return;
                }
                self.flush_at = None;
                self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                self.try_start_turn(context, effects);
            }
            Event::FlushDue => {
                self.flush_at = None;
                if context.event_turns && !self.blocked && context.has_configuration() {
                    self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                    self.try_start_turn(context, effects);
                }
            }
            Event::CheckDue => {
                self.next_check = context.now + PERIODIC_CHECK_INTERVAL;
                if context.periodic_turns && !self.blocked && context.has_configuration() {
                    effects.push(Effect::QueryGameRunning);
                }
            }
            Event::GameRunning(running) => {
                if running == Some(true)
                    && context.periodic_turns
                    && !self.blocked
                    && context.has_configuration()
                {
                    self.flush_at = None;
                    self.try_start_turn(context, effects);
                }
            }
            Event::TurnFinished { turn_id, result } => {
                self.finish_turn(context, turn_id, result, effects)
            }
            Event::CancelTurn => {
                if let Some(in_flight) = &self.in_flight {
                    in_flight.cancel.cancel();
                }
            }
            Event::Resume => {
                self.blocked = false;
            }
        }
    }
}

impl Session {
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
        if self.blocked || self.in_flight.is_some() {
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
        let Some(prompt) = build_prompt(&self.history, &self.queue, &configuration.system_text)
        else {
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
                self.queue.drain(..prompt.included_inputs);
                self.history = history;
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
}

#[cfg(test)]
mod tests;
