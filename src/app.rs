use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::{sleep_until, Instant};
use tracing::debug;

use crate::chat_session::{self, ChatSession, Context, Effect};
use crate::events::{
    ChatSaveRequest, EventQueue, EventReceiver, ExternalEvent, LifecycleEvent, LoadedChatState,
};
use crate::game_state::{GameState, LifecycleSnapshot, LifecycleState, LifecycleStatus};
use crate::notifications::{
    DisplayMessage, DisplayMessageKind, InputState, NotificationHistory, NotificationId, Notifier,
};
use crate::{ai, ai_config, ai_tools, chat_commands};

pub use crate::chat_session::ChatState;

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
    session: Mutex<ChatSession>,
    /// Distinguishes restore completions from later fortress activations.
    lifecycle_generation: AtomicU64,
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
    pub fn new() -> (Arc<Self>, EventReceiver) {
        // Create a fresh NotificationHistory and pass it in.
        let history = NotificationHistory::new();
        let (s, r) = Self::with_config_and_channels(
            ai_config::ConfigurationRepository::in_memory(),
            history,
        );
        (s, r)
    }

    pub fn load() -> Result<(Arc<Self>, EventReceiver), String> {
        let configurations = ai_config::ConfigurationRepository::load_default()?;
        // Build a fresh NotificationHistory for the loaded configuration.
        let history = NotificationHistory::new();
        Ok(Self::with_config_and_channels(configurations, history))
    }

    // Updated to accept a ready-to-use NotificationHistory.
    fn with_config_and_channels(
        configurations: ai_config::ConfigurationRepository,
        history: NotificationHistory,
    ) -> (Arc<Self>, EventReceiver) {
        let (queue_tx, queue_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel();
        let event_queue = EventQueue::new(queue_tx, lifecycle_tx);
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
                session: Mutex::new(ChatSession::new(Instant::now())),
                lifecycle_generation: AtomicU64::new(0),
                event_turns: Default::default(),
                periodic_turns: Default::default(),
                game_state,
                chat_state: watch::channel(ChatState::Idle).0,
                rollback_floor: watch::channel(None).0,
                lifecycle,
            }),
            EventReceiver {
                normal: queue_rx,
                lifecycle: lifecycle_rx,
            },
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
        site_id: Option<i64>,
    ) {
        let state = if map_loaded {
            LifecycleState::MapLoaded
        } else if world_loaded {
            LifecycleState::WorldLoaded
        } else {
            LifecycleState::Initialized
        };
        self.set_lifecycle(state, fortress_name, site_id);
    }

    /// Records that DFHack could not be reached, keeping the last fortress
    /// name so the UI can still name it.
    pub fn mark_lifecycle_unreachable(&self) {
        self.set_lifecycle(LifecycleState::Unreachable, None, None);
    }

    fn set_lifecycle(
        &self,
        state: LifecycleState,
        fortress_name: Option<String>,
        site_id: Option<i64>,
    ) {
        // Only touch the watch value when something actually changed, so
        // subscribers are not woken by every poll that sees the same state.
        let name = fortress_name.filter(|name| !name.is_empty());
        let mut lifecycle_changed = false;
        self.lifecycle.send_if_modified(|status| {
            let state_changed = status.state != state;
            status.state = state;
            let name_changed = match &name {
                Some(name) if status.fortress_name.as_deref() != Some(name.as_str()) => {
                    status.fortress_name = Some(name.clone());
                    true
                }
                _ => false,
            };
            let site_changed = match site_id {
                Some(site_id) if status.site_id != Some(site_id) => {
                    status.site_id = Some(site_id);
                    true
                }
                _ => false,
            };
            lifecycle_changed = state_changed || name_changed || site_changed;
            lifecycle_changed
        });
        if lifecycle_changed {
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
                Ok(snapshot) => {
                    let _ = self
                        .queue_tx
                        .try_send_lifecycle(LifecycleEvent::Snapshot(snapshot));
                }
                Err(_) => {
                    let _ = self
                        .queue_tx
                        .try_send_lifecycle(LifecycleEvent::Unreachable);
                }
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
    fn dispatch(self: &Arc<Self>, event: chat_session::ChatEvents) {
        self.dispatch_with(event, |_| ());
    }

    /// Like [`Self::dispatch`], and also returns what `inspect` makes of the
    /// session just before the event is handled.
    fn dispatch_with<T>(
        self: &Arc<Self>,
        event: chat_session::ChatEvents,
        inspect: impl FnOnce(&ChatSession) -> T,
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
            Effect::PersistChatState(history) => {
                let lifecycle = self.lifecycle.borrow();
                let Some(site_id) = lifecycle
                    .site_id
                    .filter(|_| lifecycle.state == LifecycleState::MapLoaded)
                else {
                    debug!(
                        "Committed chat state is not being saved because no fortress is loaded."
                    );
                    return;
                };
                let request = ChatSaveRequest {
                    generation: self.lifecycle_generation.load(Ordering::Relaxed),
                    site_id,
                    history,
                };
                if let Err(error) = self.queue_tx.try_send_chat_state_save_requested(request) {
                    tracing::warn!("{error}");
                }
            }
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
                    this.dispatch(chat_session::ChatEvents::TurnFinished { turn_id, result });
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
                    this.dispatch(chat_session::ChatEvents::CompactionFinished {
                        compaction_id,
                        result,
                    });
                });
            }
            Effect::QueryGameRunning => {
                let this = self.clone();
                tokio::spawn(async move {
                    let running = this.game_is_running().await;
                    this.dispatch(chat_session::ChatEvents::GameRunning(running));
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
    /// Whether the game is running; `None` when it cannot be reached.
    async fn game_is_running(&self) -> Option<bool> {
        self.game_state
            .run(|game| game.is_paused())
            .await
            .ok()
            .map(|paused| !paused)
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
    pub async fn run_companion_worker(self: Arc<CompanionApp>, mut queue_rx: EventReceiver) {
        let mut restoring_chat = false;
        let mut next_save_id = 0;
        let mut save_in_flight: Option<u64> = None;
        let mut pending_save: Option<ChatSaveRequest> = None;
        loop {
            let (flush_at, check_at) = {
                let session = self.session.lock().unwrap();
                (session.flush_deadline(), session.check_deadline())
            };
            tokio::select! {
                biased;
                item = queue_rx.lifecycle.recv(), if !queue_rx.lifecycle.is_closed() => match item {
                    Some(item) => {
                        if self.is_new_fortress_event(&item) {
                            let dropped = queue_rx.drain_normal();
                            debug!("Dropped {dropped} queued external events for a new fortress.");
                            restoring_chat = true;
                            pending_save = None;
                        }
                        if self.is_current_chat_load_completion(&item) {
                            restoring_chat = false;
                        }
                        match item {
                            ExternalEvent::ChatStateSaveRequested(request) => {
                                if self.chat_save_request_is_current(&request) {
                                    if save_in_flight.is_some() {
                                        pending_save = Some(request);
                                    } else {
                                        let save_id = next_save_id;
                                        next_save_id += 1;
                                        save_in_flight = Some(save_id);
                                        self.start_chat_save(save_id, request);
                                    }
                                }
                            }
                            ExternalEvent::ChatStateSaved {
                                save_id,
                                generation,
                                site_id,
                                result,
                            } => {
                                if save_in_flight != Some(save_id) {
                                    debug!("Ignoring unexpected chat save completion {save_id}.");
                                    continue;
                                }
                                save_in_flight = None;
                                if generation
                                    == self.lifecycle_generation.load(Ordering::Relaxed)
                                    && self.lifecycle.borrow().site_id == Some(site_id)
                                {
                                    if let Err(error) = result {
                                        self.notifier.notify(DisplayMessage::new(
                                            DisplayMessageKind::Warning,
                                            format!(
                                                "Companion chat state was not saved with the fortress. {error}"
                                            ),
                                        ));
                                    }
                                }
                                if let Some(request) = pending_save
                                    .take_if(|request| self.chat_save_request_is_current(request))
                                {
                                    let save_id = next_save_id;
                                    next_save_id += 1;
                                    save_in_flight = Some(save_id);
                                    self.start_chat_save(save_id, request);
                                }
                            }
                            item => self.handle_external_event(item).await,
                        }
                    },
                    None => break,
                },
                item = queue_rx.normal.recv(), if !restoring_chat => match item {
                    Some(item) => self.handle_external_event(item).await,
                    None => break,
                },
                _ = sleep_until(flush_at.unwrap_or(check_at)), if !restoring_chat && flush_at.is_some() => {
                    self.dispatch(chat_session::ChatEvents::FlushDue);
                }
                _ = sleep_until(check_at), if !restoring_chat => {
                    self.dispatch(chat_session::ChatEvents::CheckDue);
                }
            }
        }
    }

    fn is_new_fortress_event(&self, item: &ExternalEvent) -> bool {
        let ExternalEvent::Lifecycle(LifecycleEvent::Snapshot(snapshot)) = item else {
            return false;
        };
        self.snapshot_starts_new_session(snapshot)
    }

    fn snapshot_starts_new_session(&self, snapshot: &LifecycleSnapshot) -> bool {
        let Some(site_id) = snapshot.site_id.filter(|_| snapshot.map_loaded) else {
            return false;
        };
        let lifecycle = self.lifecycle.borrow();
        lifecycle.site_id != Some(site_id)
            || matches!(
                lifecycle.state,
                LifecycleState::Initialized | LifecycleState::WorldLoaded
            )
    }

    fn is_current_chat_load_completion(&self, item: &ExternalEvent) -> bool {
        matches!(
            item,
            ExternalEvent::ChatStateLoaded { generation, .. }
                if *generation == self.lifecycle_generation.load(Ordering::Relaxed)
        )
    }

    fn chat_save_request_is_current(&self, request: &ChatSaveRequest) -> bool {
        self.lifecycle_generation.load(Ordering::Relaxed) == request.generation
            && self.lifecycle.borrow().state == LifecycleState::MapLoaded
            && self.lifecycle.borrow().site_id == Some(request.site_id)
    }

    async fn handle_external_event(self: &Arc<Self>, item: ExternalEvent) {
        match item {
            ExternalEvent::Lifecycle(LifecycleEvent::Unreachable) => {
                self.dispatch(chat_session::ChatEvents::CancelUnnotifiedTurn);
                self.mark_lifecycle_unreachable();
            }
            ExternalEvent::Lifecycle(LifecycleEvent::Snapshot(snapshot)) => {
                let is_new_fortress = self.snapshot_starts_new_session(&snapshot);
                if !snapshot.map_loaded
                    && self.lifecycle.borrow().state == LifecycleState::MapLoaded
                {
                    self.dispatch(chat_session::ChatEvents::CancelUnnotifiedTurn);
                }
                if is_new_fortress {
                    {
                        let mut session = self.session.lock().unwrap();
                        session.reset_for_fortress(Instant::now());
                    }
                    self.history.clear();
                    self.chat_state.send_replace(ChatState::Idle);
                    self.rollback_floor.send_replace(None);
                }
                self.update_lifecycle(
                    snapshot.world_loaded,
                    snapshot.map_loaded,
                    Some(snapshot.fortress_name),
                    snapshot.site_id,
                );
                if let Some(site_id) = snapshot.site_id.filter(|_| is_new_fortress) {
                    let generation = self.lifecycle_generation.fetch_add(1, Ordering::Relaxed) + 1;
                    self.start_chat_restore(generation, site_id);
                }
            }
            ExternalEvent::ChatStateLoaded {
                generation,
                site_id,
                result,
            } => self.finish_chat_restore(generation, site_id, result),
            ExternalEvent::Orient => {
                self.orient();
            }
            ExternalEvent::UserInput(text) => {
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
                self.dispatch(chat_session::ChatEvents::UserRequest { text, id: Some(id) });
            }
            ExternalEvent::GameEvent(event) => {
                let id = self.notifier.notify(
                    DisplayMessage::new(
                        DisplayMessageKind::GameEvent,
                        format!("{} [{}] {}", event.date, event.category, event.text),
                    )
                    .with_input_state(InputState::Pending),
                );
                self.dispatch(chat_session::ChatEvents::Report {
                    event,
                    id: Some(id),
                });
            }
            ExternalEvent::ChatStateSaveRequested(_) | ExternalEvent::ChatStateSaved { .. } => {
                unreachable!("chat save events are coordinated by the worker loop")
            }
        }
    }

    fn start_chat_restore(self: &Arc<Self>, generation: u64, site_id: i64) {
        let this = self.clone();
        tokio::spawn(async move {
            let result = this
                .game_state
                .run(move |game| {
                    let Some(state) = game.load_chat_state(site_id)? else {
                        return Ok(None);
                    };
                    let current_game_date = game.game_date()?;
                    Ok(Some(LoadedChatState {
                        state,
                        current_game_date,
                    }))
                })
                .await;
            let _ = this
                .queue_tx
                .try_send_chat_state_loaded(generation, site_id, result);
        });
    }

    fn start_chat_save(self: &Arc<Self>, save_id: u64, request: ChatSaveRequest) {
        let this = self.clone();
        tokio::spawn(async move {
            let history = request.history;
            let site_id = request.site_id;
            let generation = request.generation;
            let result = this
                .game_state
                .run(move |game| {
                    let game_date = game.game_date()?;
                    let state = history.to_snapshot_json(site_id, &game_date)?;
                    game.save_chat_state(site_id, &state)
                })
                .await;
            let _ = this
                .queue_tx
                .try_send_chat_state_saved(save_id, generation, site_id, result);
        });
    }

    fn finish_chat_restore(
        self: &Arc<Self>,
        generation: u64,
        site_id: i64,
        result: Result<Option<LoadedChatState>, String>,
    ) {
        if generation != self.lifecycle_generation.load(Ordering::Relaxed)
            || self.lifecycle.borrow().site_id != Some(site_id)
            || self.lifecycle.borrow().state != LifecycleState::MapLoaded
        {
            debug!(
                "Ignoring stale chat restore completion for site {site_id}, generation {generation}."
            );
            return;
        }

        match result {
            Ok(None) => {
                self.notifier.notify(DisplayMessage::new(
                    DisplayMessageKind::Status,
                    "No saved companion chat state was found for this fortress.",
                ));
            }
            Ok(Some(saved)) => match ai::History::from_snapshot_json(&saved.state, site_id) {
                Ok((history, saved_game_date)) => {
                    self.session.lock().unwrap().restore_history(
                        history,
                        saved_game_date.clone(),
                        saved.current_game_date.clone(),
                    );
                    self.notifier.notify(DisplayMessage::new(
                            DisplayMessageKind::Status,
                            format!(
                                "Restored companion chat state saved at game date {saved_game_date}; current game date is {}.",
                                saved.current_game_date
                            ),
                        ));
                }
                Err(error) => {
                    self.notifier.notify(DisplayMessage::new(
                            DisplayMessageKind::Warning,
                            format!(
                                "Saved companion chat state could not be restored; starting fresh. {error}"
                            ),
                        ));
                }
            },
            Err(error) => {
                self.notifier.notify(DisplayMessage::new(
                    DisplayMessageKind::Warning,
                    format!(
                        "Saved companion chat state could not be loaded; starting fresh. {error}"
                    ),
                ));
            }
        }
        self.orient();
    }

    fn orient(self: &Arc<Self>) {
        if self.has_active_configuration()
            && self.lifecycle.borrow().state == LifecycleState::MapLoaded
        {
            // The orientation prompt is not shown to the player, so it has no
            // notification.
            self.dispatch(chat_session::ChatEvents::UserRequest {
                text: ORIENT_MESSAGE.to_owned(),
                id: None,
            });
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
        self.dispatch_with(chat_session::ChatEvents::Resume, |session| {
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
            chat_session::ChatEvents::Rollback {
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
