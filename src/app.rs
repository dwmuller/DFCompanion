use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::{sleep_until, Instant};
use tracing::debug;

use crate::events::{EventQueue, QueueItem};
use crate::game_state::{GameState, LifecycleState, LifecycleStatus};
use crate::notifications::{
    DisplayMessage, DisplayMessageKind, InputState, NotificationHistory, NotificationId, Notifier,
};
use crate::session::{self, Context, Effect, Session};
use crate::{ai, ai_config, ai_tools, chat_commands};

pub use crate::session::ChatState;

const EVENT_QUEUE_CAPACITY: usize = 50;
/// How often to re-check the game's loading state when no state-change push
/// arrives, so the state stays current even if the companion starts mid-game.
const LIFECYCLE_POLL_INTERVAL: Duration = Duration::from_secs(20);

const ORIENT_MESSAGE: &str = "[Automatic session start] You are now connected to the player's Dwarf Fortress game. Use the read-only game-state tools to get your bearings on the fortress, then give the player a brief greeting and summary of its situation.";

/// Main companion application structure, managing the AI, game state, and event
/// queue.
pub struct CompanionApp {
    queue_tx: EventQueue,
    notifier: Notifier,
    pub history: NotificationHistory,
    pub ai: ai::AiRuntime,
    /// The state of the AI conversation. All changes go through [`Self::dispatch`].
    session: Mutex<Session>,
    /// Whether the AI gets a turn shortly after game events arrive. Off by
    /// default because automatic turns spend tokens without the player asking.
    pub event_turns: std::sync::atomic::AtomicBool,
    /// Whether the AI gets a turn every `PERIODIC_CHECK_INTERVAL` while the
    /// game runs. Off by default, for the same reason.
    pub periodic_turns: std::sync::atomic::AtomicBool,
    game_state: GameState,
    chat_state: watch::Sender<ChatState>,
    rollback_floor: watch::Sender<Option<NotificationId>>,
    /// How far the game has loaded, kept current by the Lua push and the
    /// lifecycle poller. Read by tools to gate fort queries.
    lifecycle: watch::Sender<LifecycleStatus>,
}

impl CompanionApp {
    pub fn new() -> (Arc<Self>, mpsc::Receiver<QueueItem>) {
        // Create a fresh NotificationHistory and pass it in.
        let history = NotificationHistory::new();
        let (s, r) = Self::with_config_and_channels(
            ai_config::ConfigurationRepository::in_memory(),
            history,
        );
        (s, r)
    }

    pub fn load() -> Result<(Arc<Self>, mpsc::Receiver<QueueItem>), String> {
        let configurations = ai_config::ConfigurationRepository::load_default()?;
        // Build a fresh NotificationHistory for the loaded configuration.
        let history = NotificationHistory::new();
        Ok(Self::with_config_and_channels(configurations, history))
    }

    // Updated to accept a ready-to-use NotificationHistory.
    fn with_config_and_channels(
        configurations: ai_config::ConfigurationRepository,
        history: NotificationHistory,
    ) -> (Arc<Self>, mpsc::Receiver<QueueItem>) {
        let (queue_tx, queue_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let event_queue = EventQueue::new(queue_tx);
        // `history` is passed in directly.
        let notifier = Notifier::new(history.clone());
        let lifecycle = watch::channel(LifecycleStatus::default()).0;
        let game_state = GameState::new(notifier.clone(), lifecycle.subscribe());
        let ai = ai::AiRuntime::new(configurations, game_state.clone());
        (
            Arc::new(Self {
                queue_tx: event_queue.clone(),
                notifier,
                history,
                ai,
                session: Mutex::new(Session::new(Instant::now())),
                event_turns: Default::default(),
                periodic_turns: Default::default(),
                game_state,
                chat_state: watch::channel(ChatState::Idle).0,
                rollback_floor: watch::channel(None).0,
                lifecycle,
            }),
            queue_rx,
        )
    }

    pub fn notifier(&self) -> &Notifier {
        &self.notifier
    }

    pub fn event_queue(&self) -> &EventQueue {
        &self.queue_tx
    }

    pub fn chat_state(&self) -> ChatState {
        *self.chat_state.borrow()
    }

    pub fn subscribe_chat_state(&self) -> watch::Receiver<ChatState> {
        self.chat_state.subscribe()
    }

    /// The id of the oldest notification currently eligible for a rollback,
    /// if any. See [`session::Session::rollback_floor`].
    pub fn rollback_floor(&self) -> Option<NotificationId> {
        *self.rollback_floor.borrow()
    }

    pub fn subscribe_rollback_floor(&self) -> watch::Receiver<Option<NotificationId>> {
        self.rollback_floor.subscribe()
    }

    /// A snapshot of the game's loading state.
    pub fn lifecycle(&self) -> LifecycleStatus {
        self.lifecycle.borrow().clone()
    }

    /// Subscribes to changes in the game's loading state.
    pub fn subscribe_lifecycle(&self) -> watch::Receiver<LifecycleStatus> {
        self.lifecycle.subscribe()
    }

    /// Records the game's loading state from a boolean reading, notifying the
    /// player when the state actually changes.
    pub fn update_lifecycle(
        &self,
        world_loaded: bool,
        map_loaded: bool,
        fortress_name: Option<String>,
    ) {
        let state = if map_loaded {
            LifecycleState::MapLoaded
        } else if world_loaded {
            LifecycleState::WorldLoaded
        } else {
            LifecycleState::Initialized
        };
        self.set_lifecycle(state, fortress_name);
    }

    /// Records that DFHack could not be reached, keeping the last fortress
    /// name so the UI can still name it.
    pub fn mark_lifecycle_unreachable(&self) {
        self.set_lifecycle(LifecycleState::Unreachable, None);
    }

    fn set_lifecycle(&self, state: LifecycleState, fortress_name: Option<String>) {
        // Only touch the watch value when something actually changed, so
        // subscribers are not woken by every poll that sees the same state.
        let name = fortress_name.filter(|name| !name.is_empty());
        let mut state_changed = false;
        self.lifecycle.send_if_modified(|status| {
            state_changed = status.state != state;
            status.state = state;
            let name_changed = match &name {
                Some(name) if status.fortress_name.as_deref() != Some(name.as_str()) => {
                    status.fortress_name = Some(name.clone());
                    true
                }
                _ => false,
            };
            state_changed || name_changed
        });
        if state_changed {
            let message = match state {
                LifecycleState::MapLoaded => match self.lifecycle.borrow().fortress_name.as_deref()
                {
                    Some(name) => format!("Fortress loaded: {name}."),
                    None => "A fortress is loaded.".to_owned(),
                },
                LifecycleState::WorldLoaded => "A world is loaded; no fortress yet.".to_owned(),
                LifecycleState::Initialized => "No world is loaded.".to_owned(),
                LifecycleState::Unreachable => "Lost contact with DFHack.".to_owned(),
            };
            self.notifier
                .notify(DisplayMessage::new(DisplayMessageKind::Status, message));
        }
    }

    /// Polls DFHack for the game's loading state so tools can be gated even when
    /// no state-change push arrives, for example when the companion starts while
    /// a fortress is already loaded, or after a missed push. Runs until aborted.
    pub async fn run_lifecycle_poller(self: Arc<CompanionApp>) {
        loop {
            match self.game_state.run(|game| game.query_lifecycle()).await {
                Ok(snapshot) => self.update_lifecycle(
                    snapshot.world_loaded,
                    snapshot.map_loaded,
                    Some(snapshot.fortress_name),
                ),
                Err(_) => self.mark_lifecycle_unreachable(),
            }
            tokio::time::sleep(LIFECYCLE_POLL_INTERVAL).await;
        }
    }

    /// What the session needs to know about the world to handle an event.
    fn session_context(&self) -> Context {
        Context {
            now: Instant::now(),
            configuration: self.ai.configurations.active(),
            event_turns: self.event_turns_enabled(),
            periodic_turns: self.periodic_turns_enabled(),
        }
    }

    /// Gives an event to the session and carries out the effects it returns.
    ///
    /// Effects are carried out while the session is locked, so that those of
    /// concurrent events keep their order. They never wait: slow work is
    /// spawned and reports back with another event.
    fn dispatch(self: &Arc<Self>, event: session::Event) {
        self.dispatch_with(event, |_| ());
    }

    /// Like [`Self::dispatch`], and also returns what `inspect` makes of the
    /// session just before the event is handled.
    fn dispatch_with<T>(
        self: &Arc<Self>,
        event: session::Event,
        inspect: impl FnOnce(&Session) -> T,
    ) -> T {
        let mut session = self.session.lock().unwrap();
        let inspected = inspect(&session);
        let effects = session.handle(&self.session_context(), event);
        for effect in effects {
            self.perform(effect);
        }
        inspected
    }

    fn perform(self: &Arc<Self>, effect: Effect) {
        match effect {
            Effect::Notify(message) => {
                self.notifier.notify(message);
            }
            Effect::TransitionInputs { through, from, to } => {
                self.notifier.transition_inputs(through, from, to);
            }
            Effect::PublishChatState(state) => {
                let previous = self.chat_state.send_replace(state);
                debug!("chat state: {:?} -> {:?}", previous, state);
            }
            Effect::PublishRollbackFloor(floor) => {
                self.rollback_floor.send_replace(floor);
            }
            Effect::TruncateLog(from) => {
                self.history.truncate_from(from);
            }
            // Rollback reads this out of the effect list itself, in
            // [`Self::rollback`], before the rest reach here. This arm keeps
            // the match exhaustive.
            Effect::ReplyEditorText(_) => {}
            Effect::StartTurn {
                turn_id,
                configuration,
                history,
                prompt,
                cancel,
            } => {
                let this = self.clone();
                tokio::spawn(async move {
                    let result = this
                        .ai
                        .run_turn(&configuration, &history, &prompt, &cancel)
                        .await;
                    this.dispatch(session::Event::TurnFinished { turn_id, result });
                });
            }
            Effect::StartCompaction {
                compaction_id,
                configuration,
                history,
                cancel,
            } => {
                let this = self.clone();
                tokio::spawn(async move {
                    let result = this
                        .ai
                        .compact_history(&configuration, &history, &cancel)
                        .await;
                    this.dispatch(session::Event::CompactionFinished {
                        compaction_id,
                        result,
                    });
                });
            }
            Effect::QueryGameRunning => {
                let this = self.clone();
                tokio::spawn(async move {
                    let running = this.game_is_running().await;
                    this.dispatch(session::Event::GameRunning(running));
                });
            }
            Effect::Block { reason } => {
                let this = self.clone();
                tokio::spawn(async move {
                    let pause_result = this.pause_for_ai_block(&reason).await;
                    this.notifier.notify(DisplayMessage::new(
                        DisplayMessageKind::Warning,
                        blocked_warning_message(&reason, pause_result),
                    ));
                });
            }
        }
    }

    /// Pauses a running fortress if AI processing becomes blocked.
    pub async fn pause_for_ai_block(&self, reason: &str) -> Result<bool, String> {
        // If this triggers we might have to move a test to live tests, or find
        // another way to simulate AI blocking.
        debug!("Pausing for AI block: {}", reason);
        let reason = reason.to_owned();
        if cfg!(test) {
            return Ok(false);
        }
        self.game_state
            .run(move |game| game.pause_for_ai_block(&reason))
            .await
            .map_err(|error| format!("Failed to pause for AI block: {error}"))
    }

    fn has_active_configuration(&self) -> bool {
        matches!(self.ai.configurations.active(), Ok(Some(_)))
    }
    /// Verifies the game link at startup. Returns the status line, or the error.
    async fn startup_summary(&self) -> Result<String, String> {
        self.game_state.run(|game| game.startup_summary()).await
    }

    /// Whether the game is running; `None` when it cannot be reached.
    async fn game_is_running(&self) -> Option<bool> {
        self.game_state
            .run(|game| game.is_paused())
            .await
            .ok()
            .map(|paused| !paused)
    }

    /// Checks the game connection once at startup, reports the result, and if the
    /// game responds gives the AI an orientation turn.
    pub async fn check_game_at_startup(&self) {
        match self.startup_summary().await {
            Ok(summary) => {
                {
                    self.notifier
                        .notify(DisplayMessage::new(DisplayMessageKind::Status, summary));
                };
                if self.has_active_configuration() {
                    let _ = self.queue_tx.try_send(QueueItem::Orient);
                }
            }
            Err(error) => {
                self.notifier.notify(DisplayMessage::new(
                            DisplayMessageKind::Warning,
                            format!("The game is not responding yet ({error}). Is Dwarf Fortress running with a fortress loaded?"),
                        ));
            }
        }
    }

    pub fn event_turns_enabled(&self) -> bool {
        self.event_turns.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn periodic_turns_enabled(&self) -> bool {
        self.periodic_turns
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Processes the queue by turning its items into session events and firing
    /// the session's timers. The session decides when the AI gets a turn: right
    /// away for player input, at most `EVENT_FLUSH_DELAY` after a game event
    /// arrives (if event turns are on), and every `PERIODIC_CHECK_INTERVAL`
    /// while the game is running (if periodic turns are on). After an AI
    /// failure, automatic turns stop until the player resumes and sends a
    /// message, so a broken configuration cannot flood the chat or burn
    /// requests.
    pub async fn run_companion_worker(
        self: Arc<CompanionApp>,
        mut queue_rx: mpsc::Receiver<QueueItem>,
    ) {
        loop {
            let (flush_at, check_at) = {
                let session = self.session.lock().unwrap();
                (session.flush_deadline(), session.check_deadline())
            };
            tokio::select! {
                item = queue_rx.recv() => match item {
                    Some(item) => self.handle_queue_item(item).await,
                    None => break,
                },
                _ = sleep_until(flush_at.unwrap_or(check_at)), if flush_at.is_some() => {
                    self.dispatch(session::Event::FlushDue);
                }
                _ = sleep_until(check_at) => {
                    self.dispatch(session::Event::CheckDue);
                }
            }
        }
    }

    async fn handle_queue_item(self: &Arc<Self>, item: QueueItem) {
        match item {
            QueueItem::Orient => {
                if self.has_active_configuration() {
                    // The orientation prompt is not shown to the player, so it
                    // has no notification.
                    self.dispatch(session::Event::UserRequest {
                        text: ORIENT_MESSAGE.to_owned(),
                        id: None,
                    });
                }
            }
            QueueItem::UserInput(text) => {
                if let Some(command) = chat_commands::ChatCommand::parse(&text) {
                    self.notifier
                        .notify(DisplayMessage::new(DisplayMessageKind::Diagnostic, &text));
                    run_chat_command(self, command).await;
                    return;
                }
                let id = self.notifier.notify(
                    DisplayMessage::new(DisplayMessageKind::User, &text)
                        .with_input_state(InputState::Pending),
                );
                self.dispatch(session::Event::UserRequest { text, id: Some(id) });
            }
            QueueItem::GameEvent(event) => {
                let id = self.notifier.notify(
                    DisplayMessage::new(
                        DisplayMessageKind::GameEvent,
                        format!("{} [{}] {}", event.date, event.category, event.text),
                    )
                    .with_input_state(InputState::Pending),
                );
                self.dispatch(session::Event::Report {
                    event,
                    id: Some(id),
                });
            }
        }
    }

    /// The id of the newest input still waiting for the AI that has a
    /// notification. Everything waiting with a notification is at or before it.
    pub fn pending_through(&self) -> Option<NotificationId> {
        self.session.lock().unwrap().pending_through()
    }

    pub fn pending_user_message_count(&self) -> usize {
        self.session.lock().unwrap().pending_user_message_count()
    }

    pub fn pending_report_count(&self) -> usize {
        self.session.lock().unwrap().pending_report_count()
    }

    /// Clears the blocked state. Returns whether chat was blocked.
    pub fn resume_if_blocked(self: &Arc<Self>) -> bool {
        self.dispatch_with(session::Event::Resume, |session| {
            session.chat_state() == ChatState::Blocked
        })
    }

    /// Rolls the AI conversation back to just before the turn containing
    /// `target`, truncating the notification log at the same point. Returns the
    /// combined editor text (dropped user requests ahead of `editor_text`), or
    /// an error if `target` is no longer eligible. See [`session::Event::Rollback`].
    pub fn rollback(
        self: &Arc<Self>,
        target: NotificationId,
        editor_text: String,
    ) -> Result<String, String> {
        let mut reply = None;
        let mut session = self.session.lock().unwrap();
        let effects = session.handle(
            &self.session_context(),
            session::Event::Rollback {
                target,
                editor_text,
            },
        );
        for effect in effects {
            match effect {
                Effect::ReplyEditorText(result) => reply = Some(result),
                other => self.perform(other),
            }
        }
        reply.unwrap_or_else(|| Err("Rollback failed.".to_owned()))
    }

    /// Classifies a chat message as an ordinary user message or a slash
    /// command, and returns a syntax error for malformed commands.
    pub fn is_chat_message(&self, message: &str) -> Result<bool, String> {
        match chat_commands::ChatCommand::parse(message) {
            None => Ok(false),
            Some(Ok(_)) => Ok(true),
            Some(Err(error)) => Err(error),
        }
    }
}

fn blocked_warning_message(reason: &str, pause_result: Result<bool, String>) -> String {
    let pause_message = match pause_result {
        Ok(true) => {
            "DFCompanion paused Dwarf Fortress because AI processing is blocked.".to_owned()
        }
        Ok(false) => {
            "Dwarf Fortress was already paused; DFCompanion did not change its pause state."
                .to_owned()
        }
        Err(error) => format!("DFCompanion could not pause Dwarf Fortress: {error}"),
    };
    format!("{reason} {pause_message}")
}

async fn run_chat_command(
    state: &CompanionApp,
    command: Result<chat_commands::ChatCommand, String>,
) {
    match command {
        Ok(chat_commands::ChatCommand::CallTool { name, arguments }) => {
            let result = state.ai.call_tool(&name, arguments).await;
            state.notifier.notify(DisplayMessage::new(
                DisplayMessageKind::Diagnostic,
                format!(
                    "Tool {name} result:\n{}",
                    format_tool_result(&result).unwrap_or_else(|error| {
                        eprintln!("Error formatting tool result: {error}");
                        result
                    })
                ),
            ));
        }
        Ok(chat_commands::ChatCommand::ListTools) => {
            state.notifier.notify(DisplayMessage::new(
                DisplayMessageKind::Diagnostic,
                ai_tools::describe_tools(),
            ));
        }
        Ok(chat_commands::ChatCommand::DescribeTool { name }) => {
            state.notifier.notify(DisplayMessage::new(
                DisplayMessageKind::Diagnostic,
                ai_tools::describe_tool(&name).unwrap_or_else(|error| error),
            ));
        }
        Err(error) => {
            state
                .notifier
                .notify(DisplayMessage::new(DisplayMessageKind::Diagnostic, error));
        }
    }
}

/// Lays out tool output for reading.
fn format_tool_result(result: &str) -> Result<String, String> {
    match serde_json::from_str::<serde_json::Value>(result) {
        Ok(value) => Ok(serde_json_pretty::to_string(&value).unwrap()),
        Err(error) => Err(format!("Failed to parse JSON: {error}")),
    }
}

#[cfg(test)]
mod tests;
