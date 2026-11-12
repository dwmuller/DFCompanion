use std::{sync::Arc, time::Duration};

use directories::ProjectDirs;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::{sleep_until, Instant};
use verbose_macros::debug;

use crate::ai::SendResult;
use crate::events::{EventQueue, QueueItem};
use crate::game_state::GameState;
use crate::notifications::{DisplayMessage, DisplayMessageKind, NotificationHistory, Notifier};
use crate::{ai, ai_config, ai_tools, chat_commands};

const EVENT_QUEUE_CAPACITY: usize = 50;
/// How often the AI gets a turn on its own while the game is running.
const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// How long a game event may wait before the AI gets a turn that includes it.
const EVENT_FLUSH_DELAY: Duration = Duration::from_secs(5);

const ORIENT_MESSAGE: &str = "[Automatic session start] You are now connected to the player's Dwarf Fortress game. Use the read-only game-state tools to get your bearings on the fortress, then give the player a brief greeting and summary of its situation.";

pub fn project_dir() -> ProjectDirs {
    ProjectDirs::from("org", "DFCompanion", "DFCompanion")
        .expect("Could not locate the per-user project directory.")
}

enum Wake {
    Item(QueueItem),
    Flush,
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatState {
    Idle,
    Processing,
    Blocked,
}

pub struct CompanionApp {
    queue_tx: EventQueue,
    notifier: Notifier,
    pub history: NotificationHistory,
    pub ai: ai::AiRuntime,
    /// Whether the AI gets a turn shortly after game events arrive. Off by
    /// default because automatic turns spend tokens without the player asking.
    pub event_turns: std::sync::atomic::AtomicBool,
    /// Whether the AI gets a turn every `PERIODIC_CHECK_INTERVAL` while the
    /// game runs. Off by default, for the same reason.
    pub periodic_turns: std::sync::atomic::AtomicBool,
    game_state: GameState,
    chat_state: watch::Sender<ChatState>,
}

impl CompanionApp {
    pub fn new() -> (
        Arc<Self>,
        mpsc::Receiver<QueueItem>,
        broadcast::Sender<DisplayMessage>,
    ) {
        let (notifications_tx, _) = broadcast::channel(128);
        let (s, r) = Self::with_config_and_channels(
            ai_config::ConfigurationRepository::in_memory(),
            notifications_tx.clone(),
        );
        (s, r, notifications_tx)
    }

    pub fn load() -> Result<(Arc<Self>, mpsc::Receiver<QueueItem>), String> {
        let configurations = ai_config::ConfigurationRepository::load_default()?;
        let (notifications_tx, _) = broadcast::channel(128);
        Ok(Self::with_config_and_channels(
            configurations,
            notifications_tx,
        ))
    }

    fn with_config_and_channels(
        configurations: ai_config::ConfigurationRepository,
        notifications_tx: broadcast::Sender<DisplayMessage>,
    ) -> (Arc<Self>, mpsc::Receiver<QueueItem>) {
        #[cfg(test)]
        let _ = verbose_macros::try_set_debug(true);
        let (queue_tx, queue_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let event_queue = EventQueue::new(queue_tx);
        let history = NotificationHistory::record(notifications_tx.subscribe());
        let notifier = Notifier::new(notifications_tx);
        let game_state = GameState::new(notifier.clone());
        let ai = ai::AiRuntime::new(configurations, game_state.clone(), notifier.clone());

        (
            Arc::new(Self {
                queue_tx: event_queue.clone(),
                notifier,
                history,
                ai,
                event_turns: Default::default(),
                periodic_turns: Default::default(),
                game_state,
                chat_state: watch::channel(ChatState::Idle).0,
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

    fn set_chat_state(&self, state: ChatState) -> ChatState {
        let previous_state = self.chat_state.send_replace(state);
        debug!("chat state: {:?} -> {:?}", previous_state, state);
        previous_state
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
                    let this = &self;
                    let message = DisplayMessage::new(DisplayMessageKind::Status, summary);
                    let _ = this.notifier.notify(message);
                };
                if self.has_active_configuration() {
                    let _ = (&self).queue_tx.try_send(QueueItem::Orient);
                }
            }
            Err(error) => {
                let this = &self;
                let message = DisplayMessage::new(
                            DisplayMessageKind::Warning,
                            format!("The game is not responding yet ({error}). Is Dwarf Fortress running with a fortress loaded?"),
                        );
                let _ = this.notifier.notify(message);
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

    pub fn chat_is_blocked(&self) -> bool {
        matches!(*self.chat_state.borrow(), ChatState::Blocked)
    }

    pub fn resume_if_blocked(&self) -> bool {
        self.chat_state.send_if_modified(|state| {
            if *state == ChatState::Blocked {
                debug!("chat state: {:?} -> Idle", &state);
                *state = ChatState::Idle;
                true
            } else {
                false
            }
        })
    }

    /// Classifies a chat message as an ordinary user message or a slash
    /// command, and returns a syntax error for malformed commands.
    pub fn classify_chat_message(&self, message: &str) -> Result<DisplayMessageKind, String> {
        match chat_commands::ChatCommand::parse(message) {
            None => Ok(DisplayMessageKind::User),
            Some(Ok(_)) => Ok(DisplayMessageKind::Diagnostic),
            Some(Err(error)) => Err(error),
        }
    }

    fn try_begin_processing(&self) -> Result<(), ChatState> {
        let mut previous_state = ChatState::Idle;
        let changed = self.chat_state.send_if_modified(|state| {
            previous_state = *state;
            match *state {
                ChatState::Idle => {
                    debug!("chat state: {:?} -> Processing", &state);
                    *state = ChatState::Processing;
                    true
                }
                ChatState::Processing | ChatState::Blocked => false,
            }
        });
        if changed {
            Ok(())
        } else {
            Err(previous_state)
        }
    }

    async fn block_chat(&self, reason: &str) {
        let previous_state = self.set_chat_state(ChatState::Blocked);
        if previous_state == ChatState::Blocked {
            return;
        }

        {
            let this = &self;
            let message = DisplayMessage::new(
                DisplayMessageKind::Warning,
                blocked_warning_message(reason, self.pause_for_ai_block(reason).await),
            );
            let _ = this.notifier.notify(message);
        };
    }

    /// Processes pending AI input until it is drained or an interaction fails.
    async fn flush_pending_interactions_loop(&self) {
        loop {
            match self.ai.send_next_interaction().await {
                SendResult::MorePending => {}
                SendResult::NoActiveConfiguration => {
                    self.block_chat("No active AI configuration.").await;
                    break;
                }
                SendResult::Drained => {
                    self.set_chat_state(ChatState::Idle);
                    break;
                }
                SendResult::Failed(error) => {
                    self.block_chat(&format!(
                        "AI request failed. Your chat and collected reports are retained; AI processing is paused until you send a message or retry. {error}"
                    ))
                    .await;
                    break;
                }
            }
        }
    }
    async fn flush_pending_interactions(self: &Arc<Self>) {
        match self.try_begin_processing() {
            Ok(_) => {}
            Err(ChatState::Processing) => {
                return; // We were already processing.
            }
            _ => {
                return;
            }
        };
        let this = self.clone();
        tokio::task::spawn(async move { this.flush_pending_interactions_loop().await });
    }

    /// Processes the queue. Besides player input, the AI gets a turn (with all
    /// pending reports) at most `EVENT_FLUSH_DELAY` after a game event arrives,
    /// and every `PERIODIC_CHECK_INTERVAL` while the game is running. After an AI
    /// failure, automatic turns stop until the player sends a message or retries,
    /// so a broken configuration cannot flood the chat or burn requests.
    pub fn run_companion_worker(
        self: Arc<CompanionApp>,
        mut queue_rx: mpsc::Receiver<QueueItem>,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        async move {
            let mut flush_at: Option<Instant> = None;
            let mut next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
            loop {
                let flush_deadline = flush_at.unwrap_or(next_check);
                let wake = tokio::select! {
                    item = queue_rx.recv() => match item {
                        Some(item) => Wake::Item(item),
                        None => break,
                    },
                    _ = sleep_until(flush_deadline), if flush_at.is_some() => Wake::Flush,
                    _ = sleep_until(next_check) => Wake::Check,
                };
                match wake {
                    Wake::Item(QueueItem::AiRetry) => {
                        // TODO: This should go away soon - retries will be handled by the UI.
                        if self.ai.pending_user_message_count().await == 0 {
                            {
                                let this = &self;
                                let message = DisplayMessage::new(
                                    DisplayMessageKind::Status,
                                    "There is no failed AI request to retry.",
                                );
                                let _ = this.notifier.notify(message);
                            };
                            continue;
                        }
                        self.flush_pending_interactions().await;
                        flush_at = None;
                        next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                    }
                    Wake::Item(QueueItem::Orient) => {
                        if self.has_active_configuration() {
                            self.ai.push_user_message(ORIENT_MESSAGE).await;
                            self.flush_pending_interactions().await;
                            flush_at = None;
                            next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                        }
                    }
                    Wake::Item(QueueItem::UserInput(text)) => {
                        if let Some(command) = chat_commands::ChatCommand::parse(&text) {
                            run_chat_command(&self, command).await;
                            continue;
                        }
                        self.ai.push_user_message(&text).await;
                        self.flush_pending_interactions().await;
                        flush_at = None;
                        next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                    }
                    Wake::Item(QueueItem::GameEvent(event)) => {
                        {
                            let this = &self;
                            let message = DisplayMessage::new(
                                DisplayMessageKind::GameEvent,
                                format!("{} [{}] {}", event.date, event.category, event.text),
                            );
                            let _ = this.notifier.notify(message);
                        };
                        self.ai.push_report(event).await;
                        flush_at.get_or_insert_with(|| Instant::now() + EVENT_FLUSH_DELAY);
                    }
                    Wake::Flush => {
                        flush_at = None;
                        if self.event_turns_enabled()
                            && !self.chat_is_blocked()
                            && self.has_active_configuration()
                        {
                            self.flush_pending_interactions().await;
                            next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                        }
                    }
                    Wake::Check => {
                        next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                        if self.periodic_turns_enabled()
                            && !self.chat_is_blocked()
                            && self.has_active_configuration()
                            && self.game_is_running().await == Some(true)
                        {
                            flush_at = None;
                            self.flush_pending_interactions().await;
                        }
                    }
                }
            }
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
            {
                let this = &state;
                let message = DisplayMessage::new(
                    DisplayMessageKind::Diagnostic,
                    format!(
                        "Tool {name} result:\n{}",
                        format_tool_result(&result).unwrap_or_else(|error| {
                            eprintln!("Error formatting tool result: {error}");
                            result
                        })
                    ),
                );
                let _ = this.notifier.notify(message);
            };
        }
        Ok(chat_commands::ChatCommand::ListTools) => {
            let this = &state;
            let message =
                DisplayMessage::new(DisplayMessageKind::Diagnostic, ai_tools::describe_tools());
            let _ = this.notifier.notify(message);
        }
        Ok(chat_commands::ChatCommand::DescribeTool { name }) => {
            let this = &state;
            let message = DisplayMessage::new(
                DisplayMessageKind::Diagnostic,
                ai_tools::describe_tool(&name).unwrap_or_else(|error| error),
            );
            let _ = this.notifier.notify(message);
        }
        Err(error) => {
            let this = &state;
            let message = DisplayMessage::new(DisplayMessageKind::Diagnostic, error);
            let _ = this.notifier.notify(message);
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
mod tests {
    use super::*;
    use crate::events::{GameEvent, QueueItem};

    // Find next message of specific kind in the notifications stream.
    async fn recv_kind(
        notifications: &mut broadcast::Receiver<DisplayMessage>,
        kind: DisplayMessageKind,
    ) -> DisplayMessage {
        loop {
            let message = notifications.recv().await.unwrap();
            if message.kind == kind {
                return message;
            }
        }
    }

    #[test]
    fn blocked_warning_does_not_claim_to_pause_an_already_paused_game() {
        let paused = blocked_warning_message("AI request failed.", Ok(true));
        assert!(paused.contains("DFCompanion paused Dwarf Fortress"));

        let already_paused = blocked_warning_message("AI request failed.", Ok(false));
        assert!(already_paused.contains("was already paused"));
        assert!(!already_paused.contains("DFCompanion paused Dwarf Fortress"));

        let failure = blocked_warning_message("AI request failed.", Err("DFHack offline".into()));
        assert!(failure.contains("could not pause Dwarf Fortress: DFHack offline"));
    }

    #[tokio::test]
    async fn notification_history_replays_then_streams_live() {
        let (app, _queue_rx, _) = CompanionApp::new();
        let notifier = app.notifier();
        notifier.notify(DisplayMessage::new(DisplayMessageKind::Status, "first"));
        notifier.notify(DisplayMessage::new(DisplayMessageKind::User, "hello"));
        notifier.notify(DisplayMessage::new(
            DisplayMessageKind::Diagnostic,
            "/tools",
        ));
        notifier.notify(DisplayMessage::new(DisplayMessageKind::Companion, "**hi**"));
        for _ in 0..50 {
            if app.history.subscribe(None).0.len() == 4 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let (backlog, mut live) = app.history.subscribe(None);
        assert_eq!(backlog.len(), 4);
        assert_eq!(backlog[0].1.kind, DisplayMessageKind::Status);
        assert_eq!(backlog[1].1.kind, DisplayMessageKind::User);
        assert_eq!(backlog[2].1.kind, DisplayMessageKind::Diagnostic);
        assert_eq!(backlog[3].1.kind, DisplayMessageKind::Companion);
        assert_eq!(
            app.history.display_history_markdown(),
            "**Status:**\n\nfirst\n\n**You:**\n\nhello\n\n**Diagnostic:**\n\n/tools\n\n**Companion:**\n\n**hi**"
        );
        notifier.notify(DisplayMessage::new(DisplayMessageKind::Warning, "third"));
        assert_eq!(
            live.recv().await.unwrap(),
            (5, DisplayMessage::new(DisplayMessageKind::Warning, "third"))
        );

        let (resumed, _) = app.history.subscribe(Some(4));
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].1.kind, DisplayMessageKind::Warning);
    }
    use crate::api::{
        companion_client::CompanionClient, companion_server::CompanionServer,
        EventSubmissionStatus, SimulationEvent, SimulationTimestamp,
    };
    use axum::{
        body::Body,
        http::{header, Request as HttpRequest, StatusCode},
    };
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;
    use tower::ServiceExt;

    #[tokio::test]
    async fn mock_plugin_event_flows_through_grpc_into_companion_queue() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        let (address, server_task) = start_test_grpc_server(app.clone()).await;

        let mut client = CompanionClient::connect(format!("http://{address}"))
            .await
            .unwrap();
        let response = client
            .submit_simulation_event(SimulationEvent {
                category: "unit_death".to_owned(),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_134,
                }),
                description: "A dwarf was found dead.".to_owned(),
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            EventSubmissionStatus::try_from(response.status).unwrap(),
            EventSubmissionStatus::Accepted
        );

        let QueueItem::GameEvent(queued) = queue_rx.recv().await.unwrap() else {
            panic!("expected a game event");
        };
        assert_eq!(queued.category, "unit_death");
        assert_eq!(queued.text, "A dwarf was found dead.");
        assert_eq!(queued.date, "252-04-07:1134");

        let invalid_error = client
            .submit_simulation_event(SimulationEvent {
                category: "unit_death".to_owned(),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 13,
                    day: 7,
                    tick: 1_135,
                }),
                description: "invalid month".to_owned(),
            })
            .await
            .unwrap_err();
        assert_eq!(invalid_error.code(), tonic::Code::InvalidArgument);
        assert!(queue_rx.try_recv().is_err());

        server_task.abort();
    }

    #[tokio::test]
    async fn full_event_queue_responds_with_pause_request_and_notifies_ui() {
        let (app, _queue_rx, notifications_tx) = CompanionApp::new();
        let mut notifications = notifications_tx.subscribe();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            app.queue_tx.try_send(QueueItem::AiRetry).unwrap();
        }
        let (address, server_task) = start_test_grpc_server(app.clone()).await;
        let mut client = CompanionClient::connect(format!("http://{address}"))
            .await
            .unwrap();

        let response = client
            .submit_simulation_event(SimulationEvent {
                category: "unit_death".to_owned(),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_134,
                }),
                description: "queue overflow test".to_owned(),
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            EventSubmissionStatus::try_from(response.status).unwrap(),
            EventSubmissionStatus::PauseRequestedQueueFull
        );
        assert!(response.message.contains("asked to pause"));
        assert!(notifications
            .recv()
            .await
            .unwrap()
            .text
            .contains("queue is full"));

        server_task.abort();
    }

    #[tokio::test]
    async fn web_chat_input_is_enqueued_as_user_input() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        let response = crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/chat")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("message=hello+Companion"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("hx-swap-oob"));
        assert_eq!(
            queue_rx.recv().await.unwrap(),
            QueueItem::UserInput("hello Companion".to_owned())
        );
    }

    #[tokio::test]
    async fn worker_runs_chat_commands_without_involving_the_ai() {
        let (app, queue_rx, notifications_tx) = CompanionApp::new();
        let mut notifier = notifications_tx.subscribe();
        let queue_tx = app.queue_tx.clone();
        let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
        for text in ["/call no_such_tool a=1", "/bogus"] {
            queue_tx
                .try_send(QueueItem::UserInput(text.to_owned()))
                .unwrap();
        }

        let result = notifier.recv().await.unwrap();
        assert_eq!(result.kind, DisplayMessageKind::Diagnostic);
        assert!(
            result.text.contains("Unknown tool: no_such_tool"),
            "{result:?}"
        );
        let error = notifier.recv().await.unwrap();
        assert_eq!(error.kind, DisplayMessageKind::Diagnostic);
        assert!(
            error.text.contains("Unknown chat command: /bogus"),
            "{error:?}"
        );
        assert!(app.ai.pending_user_message_count().await == 0);
        assert_eq!(app.history.display_history_markdown(), "**Diagnostic:**\n\nTool no_such_tool result:\n{ \"error\": \"Unknown tool: no_such_tool\" }\n\n\n**Diagnostic:**\n\nUnknown chat command: /bogus");
        worker.abort();
    }

    #[tokio::test]
    async fn web_chat_rejects_cross_origin_request() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        let response = crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/chat")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("message=should+not+be+accepted"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn configuration_and_retry_routes_reject_cross_origin_requests() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        let router = crate::web::router(app);
        let settings_response = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/fragments/ai-configurations/new")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Profile&interaction_method=Anthropic&model_id=model&url=https%3A%2F%2Fapi.anthropic.com%2F&system_text=text",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let retry_response = router
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/retry")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "https://attacker.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(settings_response.status(), StatusCode::FORBIDDEN);
        assert_eq!(retry_response.status(), StatusCode::FORBIDDEN);
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn configuration_api_supports_rename_and_delete() {
        let (app, _, _) = CompanionApp::new();
        let router = crate::web::router(app.clone());
        let configurations = &app.ai.configurations;
        let create_body = |name: &str| {
            format!(
                "name={name}&interaction_method=OpenAI&model_id=gpt-test&url=https%3A%2F%2Fapi.openai.com%2Fv1%2F&system_text=Use+the+provided+evidence."
            )
        };
        let create = |name: &str| {
            configurations
                .create(
                    name,
                    genai::adapter::AdapterKind::OpenAI,
                    "gpt-test",
                    "https://api.openai.com/v1/",
                    "Use the provided evidence.",
                )
                .unwrap()
                .id
        };
        let first_id = create("Primary");
        let second_id = create("Secondary");
        configurations.set_active(Some(&second_id)).unwrap();

        let rename = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("PUT")
                    .uri(format!("/fragments/ai-configurations/{first_id}"))
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(create_body("Renamed")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rename.status(), StatusCode::OK);

        let delete = router
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri(format!("/fragments/ai-configurations/{second_id}"))
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn configuration_management_page_is_served_separately() {
        let (app, _, _) = CompanionApp::new();
        let response = crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .uri("/ai-configurations")
                    .header(header::HOST, "127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 32_768)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("AI Configurations"));
        assert!(html.contains("configuration-list"));
    }

    #[tokio::test]
    async fn provider_catalog_route_returns_setup_presets_without_configuration_ids() {
        let (app, _, _) = CompanionApp::new();
        let response = crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/ai-providers")
                    .header(header::HOST, "127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 32_768)
            .await
            .unwrap();
        let providers: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let providers = providers.as_array().unwrap();
        let openai_chat = providers
            .iter()
            .find(|provider| provider["name"] == "OpenAI: Chat Completions API")
            .unwrap();
        let openai_responses = providers
            .iter()
            .find(|provider| provider["name"] == "OpenAI: Responses API")
            .unwrap();
        assert_eq!(openai_chat["adapter_kind"], "OpenAI");
        assert_eq!(openai_responses["adapter_kind"], "OpenAIResp");
        assert!(openai_chat["api_url"].is_string());
        assert!(openai_chat["models"].is_array());
        assert!(openai_chat.get("id").is_none());
    }

    #[tokio::test]
    async fn chat_without_api_key_retains_context_for_retry() {
        let (app, queue_rx, notifications_tx) = CompanionApp::new();
        let config = app
            .ai
            .configurations
            .create(
                "test config",
                genai::adapter::AdapterKind::OpenAI,
                "test-model",
                "https://api.openai.com/v1/",
                crate::ai::DEFAULT_SYSTEM_TEXT,
            )
            .unwrap();
        app.ai.configurations.set_active(Some(&config.id)).unwrap();
        let mut notifications = notifications_tx.subscribe();
        app.queue_tx
            .try_send(QueueItem::GameEvent(GameEvent {
                id: None,
                date: "254-07-05:1061".to_owned(),
                report_type: None,
                category: "arrivals".to_owned(),
                text: "A caravan arrived.".to_owned(),
            }))
            .unwrap();
        app.queue_tx
            .try_send(QueueItem::UserInput("What happened?".to_owned()))
            .unwrap();
        let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));

        assert!(notifications.recv().await.unwrap().kind == DisplayMessageKind::GameEvent);
        let failure = recv_kind(&mut notifications, DisplayMessageKind::Warning).await;
        if !failure.text.contains("AI request failed") {
            println!("Expected AI request failure, got: {:?}", failure);
        }
        assert!(failure.text.contains("AI request failed"));
        let conversation = app.ai.pending_report_count().await;
        assert_eq!(conversation, 1);
        let conversation = app.ai.pending_user_message_count().await;
        assert_eq!(conversation, 1);
        worker.abort();
    }

    const EVENT_BODY: &str = r#"{"id":42,"date":"254-07-05:1061","type":"CANCEL_JOB","category":"jobs","text":"Make ash cancelled."}"#;

    fn sample_event() -> QueueItem {
        QueueItem::GameEvent(GameEvent {
            id: None,
            date: "254-07-05:1061".to_owned(),
            report_type: None,
            category: "arrivals".to_owned(),
            text: "A caravan arrived.".to_owned(),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn events_trigger_an_ai_turn_after_the_flush_delay_then_pause_automatic_turns_on_failure()
    {
        let (app, queue_rx, notifications_tx) = CompanionApp::new();
        let config = app
            .ai
            .configurations
            .create(
                "test config",
                genai::adapter::AdapterKind::OpenAI,
                "test-model",
                "https://api.openai.com/v1/",
                crate::ai::DEFAULT_SYSTEM_TEXT,
            )
            .unwrap();
        app.ai.configurations.set_active(Some(&config.id)).unwrap();
        let mut notifications = notifications_tx.subscribe();
        app.event_turns
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
        app.queue_tx.try_send(sample_event()).unwrap();

        assert!(notifications.recv().await.unwrap().kind == DisplayMessageKind::GameEvent);
        let started = Instant::now();
        let failure = recv_kind(&mut notifications, DisplayMessageKind::Warning).await;
        assert!(failure.text.contains("AI request failed"), "{failure:?}");
        assert_eq!(started.elapsed(), EVENT_FLUSH_DELAY);

        // Automatic turns stay paused after the failure, however long we wait.
        let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
        assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
        assert_eq!(app.ai.pending_report_count().await, 1);
        worker.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn automatic_turns_are_off_by_default() {
        let (app, queue_rx, notifications_tx) = CompanionApp::new();
        app.ai
            .configurations
            .create(
                "test config",
                genai::adapter::AdapterKind::OpenAI,
                "test-model",
                "https://api.openai.com/v1/",
                crate::ai::DEFAULT_SYSTEM_TEXT,
            )
            .unwrap();
        let mut notifications = notifications_tx.subscribe();
        let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
        app.queue_tx.try_send(sample_event()).unwrap();
        assert!(notifications.recv().await.unwrap().kind == DisplayMessageKind::GameEvent);
        let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
        assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
        worker.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_checks_stay_silent_without_an_ai_configuration() {
        let (app, queue_rx, notifications_tx) = CompanionApp::new();
        let mut notifications = notifications_tx.subscribe();
        let worker = tokio::spawn(app.run_companion_worker(queue_rx));
        let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
        assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
        worker.abort();
    }

    async fn post_plugin_event(
        app: Arc<CompanionApp>,
        body: &'static str,
    ) -> axum::response::Response {
        crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/plugin/events")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn automatic_turn_form_sets_each_trigger_independently() {
        let (app, _, _) = CompanionApp::new();
        for (body, events, periodic) in [
            ("event_turns=on", true, false),
            ("periodic_turns=on", false, true),
            ("event_turns=on&periodic_turns=on", true, true),
            ("", false, false),
        ] {
            let response = crate::web::router(app.clone())
                .oneshot(
                    HttpRequest::builder()
                        .method("PUT")
                        .uri("/api/automatic-turns")
                        .header(header::HOST, "127.0.0.1:3000")
                        .header(header::ORIGIN, "http://127.0.0.1:3000")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{body}");
            assert_eq!(app.event_turns_enabled(), events, "{body}");
            assert_eq!(app.periodic_turns_enabled(), periodic, "{body}");
        }
    }

    #[tokio::test]
    async fn main_page_prevents_caching_and_offers_the_event_toggle() {
        let (app, _, _) = CompanionApp::new();
        let response = crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .header(header::HOST, "127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let csp = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("enable dfcompanion"));
        assert!(html.contains("show-game-events"));
        assert!(html.contains("hx-sse:connect=\"/events\""));
        assert!(html.contains("/assets/hx-sse.min.js"));
        assert!(!html.contains("marked") && !html.contains("purify"));
        assert!(csp.contains("script-src 'self'") && !csp.contains("img-src"));
    }

    #[tokio::test]
    async fn htmx_scripts_are_served() {
        let (app, _, _) = CompanionApp::new();
        for path in ["/assets/htmx.min.js", "/assets/hx-sse.min.js"] {
            let response = crate::web::router(app.clone())
                .oneshot(
                    HttpRequest::builder()
                        .uri(path)
                        .header(header::HOST, "127.0.0.1:3000")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn lua_http_event_is_enqueued() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        let response = post_plugin_event(app, EVENT_BODY).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            queue_rx.recv().await.unwrap(),
            QueueItem::GameEvent(GameEvent {
                id: Some(42),
                date: "254-07-05:1061".to_owned(),
                report_type: Some("CANCEL_JOB".to_owned()),
                category: "jobs".to_owned(),
                text: "Make ash cancelled.".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn lua_http_event_rejects_browser_requests() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        let response = crate::web::router(app)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/plugin/events")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(EVENT_BODY))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn lua_http_event_rejects_invalid_events() {
        let (app, mut queue_rx, _) = CompanionApp::new();
        for body in [
            r#"{"date":"nope","category":"jobs","text":"bad date"}"#,
            r#"{"date":"254-07-05:1061","category":"","text":"no category"}"#,
            r#"{"date":"254-07-05:1061","category":"jobs","text":"  "}"#,
            r#"{"date":"254-07-05:1061","category":"jobs","text":"x","real_world_time_utc":1}"#,
        ] {
            let response = post_plugin_event(app.clone(), body).await;
            assert!(response.status().is_client_error(), "{body}");
        }
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn lua_http_event_reports_full_queue_without_accepting_event() {
        let (app, queue_rx, _) = CompanionApp::new();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            app.queue_tx.try_send(QueueItem::AiRetry).unwrap();
        }

        let response = post_plugin_event(app, EVENT_BODY).await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(queue_rx.len(), EVENT_QUEUE_CAPACITY);
    }

    async fn start_test_grpc_server(
        app: Arc<CompanionApp>,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Server::builder()
            .add_service(CompanionServer::new(crate::rpc::service(app)))
            .serve_with_incoming(TcpListenerStream::new(listener));

        (address, tokio::spawn(server))
    }
}
