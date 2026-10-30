use std::collections::VecDeque;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep_until, Instant};

pub mod ai;
pub mod ai_config;
pub mod ai_providers;
pub mod ai_tools;
pub mod ai_usage;
pub mod chat_commands;
pub mod game_state;

pub mod api {
    tonic::include_proto!("dfcompanion.v1");
}
pub mod rpc;
pub mod web;
pub mod web_templates;

pub const EVENT_QUEUE_CAPACITY: usize = 50;
/// Marks UI notifications that echo game events, so the browser can hide them.
pub const GAME_EVENT_PREFIX: &str = "Game event: ";
/// Marks a companion comment the player did not ask for (from a timed turn),
/// so the page can beep.
/// Marks token-usage notices so the page can hide them.
pub const AI_USAGE_PREFIX: &str = "AI usage: ";
pub const UNSOLICITED_PREFIX: &str = "Unsolicited companion comment: ";

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
        game_state::validate_game_date(&self.date).map_err(|error| format!("Invalid date: {error}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueueItem {
    UserInput(String),
    AiRetry,
    /// An AI configuration was just selected: give the AI a turn to look around.
    Orient,
    GameEvent(GameEvent),
}

const ORIENT_MESSAGE: &str = "[Automatic session start] You are now connected to the player's Dwarf Fortress game. Use the read-only game-state tools to get your bearings on the fortress, then give the player a brief greeting and summary of its situation.";

const NOTIFICATION_HISTORY_LIMIT: usize = 500;

/// Keeps recent UI notifications so a page that reloads (for example after
/// visiting the configuration page) can replay the conversation.
#[derive(Clone)]
pub struct NotificationHistory {
    inner: std::sync::Arc<std::sync::Mutex<HistoryState>>,
    live_tx: broadcast::Sender<(u64, String)>,
}

#[derive(Default)]
struct HistoryState {
    entries: VecDeque<(u64, String)>,
    last_id: u64,
}

impl NotificationHistory {
    /// Records everything sent on `notifications_tx`. Without a Tokio runtime
    /// nothing is recorded.
    fn record(notifications_tx: &broadcast::Sender<String>) -> Self {
        let history = Self {
            inner: Default::default(),
            live_tx: broadcast::channel(128).0,
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let mut rx = notifications_tx.subscribe();
            let recorder = history.clone();
            runtime.spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(message) => recorder.push(message),
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            recorder.push(format!("{count} notifications were missed."))
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        history
    }

    fn push(&self, message: String) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.entries.len() == NOTIFICATION_HISTORY_LIMIT {
            inner.entries.pop_front();
        }
        inner.last_id += 1;
        let id = inner.last_id;
        inner.entries.push_back((id, message.clone()));
        // Sent under the lock so subscribers never see a gap or a duplicate.
        let _ = self.live_tx.send((id, message));
    }

    /// The recorded messages with an id greater than `after` (all of them when
    /// `after` is `None`) and a receiver for every message after those. Ids
    /// increase with each message, so a reconnecting browser can resume where
    /// it left off.
    pub fn subscribe(
        &self,
        after: Option<u64>,
    ) -> (Vec<(u64, String)>, broadcast::Receiver<(u64, String)>) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let backlog = inner
            .entries
            .iter()
            .filter(|(id, _)| after.map_or(true, |after| *id > after))
            .cloned()
            .collect();
        (backlog, self.live_tx.subscribe())
    }
}

#[derive(Clone)]
pub struct AppState {
    pub queue_tx: mpsc::Sender<QueueItem>,
    pub notifications_tx: broadcast::Sender<String>,
    pub history: NotificationHistory,
    pub ai: std::sync::Arc<ai::AiRuntime>,
    /// Whether the AI gets a turn shortly after game events arrive. Off by
    /// default because automatic turns spend tokens without the player asking.
    pub event_turns: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether the AI gets a turn every `PERIODIC_CHECK_INTERVAL` while the
    /// game runs. Off by default, for the same reason.
    pub periodic_turns: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    pub fn new() -> (Self, mpsc::Receiver<QueueItem>) {
        Self::with_ai_configurations(ai_config::ConfigurationRepository::in_memory())
    }

    pub fn load() -> Result<(Self, mpsc::Receiver<QueueItem>), String> {
        let configurations = ai_config::ConfigurationRepository::load_default()?;
        Ok(Self::with_ai_configurations(configurations))
    }

    fn with_ai_configurations(
        configurations: ai_config::ConfigurationRepository,
    ) -> (Self, mpsc::Receiver<QueueItem>) {
        let (queue_tx, queue_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let (notifications_tx, _) = broadcast::channel(128);
        let ai = std::sync::Arc::new(ai::AiRuntime::new(configurations, notifications_tx.clone()));
        let history = NotificationHistory::record(&notifications_tx);

        (
            Self {
                queue_tx,
                notifications_tx,
                history,
                ai,
                event_turns: Default::default(),
                periodic_turns: Default::default(),
            },
            queue_rx,
        )
    }

    pub fn event_turns_enabled(&self) -> bool {
        self.event_turns.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn periodic_turns_enabled(&self) -> bool {
        self.periodic_turns
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn notify(&self, message: impl Into<String>) {
        // No connected browser is a normal state; notifications are best-effort.
        let _ = self.notifications_tx.send(message.into());
    }
}

/// How long a game event may wait before the AI gets a turn that includes it.
pub const EVENT_FLUSH_DELAY: Duration = Duration::from_secs(5);
/// How often the AI gets a turn on its own while the game is running.
pub const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(30);

enum Wake {
    Item(QueueItem),
    Flush,
    Check,
}

/// Processes the queue. Besides player input, the AI gets a turn (with all
/// pending reports) at most `EVENT_FLUSH_DELAY` after a game event arrives,
/// and every `PERIODIC_CHECK_INTERVAL` while the game is running. After an AI
/// failure, automatic turns stop until the player sends a message or retries,
/// so a broken configuration cannot flood the chat or burn requests.
pub fn run_companion_worker(
    mut queue_rx: mpsc::Receiver<QueueItem>,
    state: AppState,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    async move {
        let mut flush_at: Option<Instant> = None;
        let mut next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
        let mut suspended = false;
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
                    suspended = !try_ai_turn(&state, TurnKind::Retry).await;
                    flush_at = None;
                    next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                }
                Wake::Item(QueueItem::Orient) => {
                    if has_active_configuration(&state) {
                        state
                            .ai
                            .conversation
                            .lock()
                            .await
                            .pending_user_messages
                            .push(ORIENT_MESSAGE.to_owned());
                        suspended = !try_ai_turn(&state, TurnKind::Player).await;
                        flush_at = None;
                        next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                    }
                }
                Wake::Item(QueueItem::UserInput(text)) => {
                    if let Some(command) = chat_commands::ChatCommand::parse(&text) {
                        run_chat_command(&state, command).await;
                        continue;
                    }
                    state
                        .ai
                        .conversation
                        .lock()
                        .await
                        .pending_user_messages
                        .push(text);
                    suspended = !try_ai_turn(&state, TurnKind::Player).await;
                    flush_at = None;
                    next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                }
                Wake::Item(QueueItem::GameEvent(event)) => {
                    let line = serde_json::to_string(&event).unwrap_or_else(|_| event.text.clone());
                    let dropped_reports = state.ai.conversation.lock().await.push_report(line);
                    if dropped_reports > 0 {
                        state.notify(format!(
                            "{dropped_reports} oldest pending report(s) were discarded to keep the AI context within its input limit."
                        ));
                    }
                    state.notify(format!(
                        "{GAME_EVENT_PREFIX}{} [{}] {}",
                        event.date, event.category, event.text
                    ));
                    flush_at.get_or_insert_with(|| Instant::now() + EVENT_FLUSH_DELAY);
                }
                Wake::Flush => {
                    flush_at = None;
                    if state.event_turns_enabled() && !suspended && has_active_configuration(&state)
                    {
                        suspended = !try_ai_turn(&state, TurnKind::Periodic).await;
                        next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                    }
                }
                Wake::Check => {
                    next_check = Instant::now() + PERIODIC_CHECK_INTERVAL;
                    if state.periodic_turns_enabled()
                        && !suspended
                        && has_active_configuration(&state)
                        && state.ai.game_is_running().await == Some(true)
                    {
                        flush_at = None;
                        suspended = !try_ai_turn(&state, TurnKind::Periodic).await;
                    }
                }
            }
        }
    }
}

fn has_active_configuration(state: &AppState) -> bool {
    matches!(state.ai.configurations.active(), Ok(Some(_)))
}

/// Checks the game connection once at startup, reports the result, and if the
/// game responds gives the AI an orientation turn.
pub async fn check_game_at_startup(state: AppState) {
    match state.ai.startup_summary().await {
        Ok(summary) => {
            state.notify(summary);
            if has_active_configuration(&state) {
                let _ = state.queue_tx.try_send(QueueItem::Orient);
            }
        }
        Err(error) => state.notify(format!(
            "The game is not responding yet ({error}). Is Dwarf Fortress running with a fortress loaded?"
        )),
    }
}

/// Lays out tool output for reading: one top-level field per line, and one
/// line per array element, with everything else compact.
fn format_tool_result(result: &str) -> String {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(result) else {
        return result.to_owned();
    };
    let mut lines = Vec::new();
    for (key, value) in fields {
        match value {
            serde_json::Value::Array(items) if !items.is_empty() => {
                lines.push(format!("{key}:"));
                lines.extend(items.iter().map(|item| format!("  {item}")));
            }
            other => lines.push(format!("{key}: {other}")),
        }
    }
    lines.join("\n")
}

async fn run_chat_command(state: &AppState, command: Result<chat_commands::ChatCommand, String>) {
    match command {
        Ok(chat_commands::ChatCommand::CallTool { name, arguments }) => {
            let result = state.ai.call_tool(&name, arguments).await;
            state.notify(format!(
                "Tool {name} result:\n{}",
                format_tool_result(&result)
            ));
        }
        Ok(chat_commands::ChatCommand::ListTools) => state.notify(ai_tools::describe_tools()),
        Ok(chat_commands::ChatCommand::DescribeTool { name }) => {
            state.notify(ai_tools::describe_tool(&name).unwrap_or_else(|error| error))
        }
        Err(error) => state.notify(error),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TurnKind {
    /// Answers a chat message the player just sent.
    Player,
    /// Re-sends the player's pending messages after a failure.
    Retry,
    /// An automatic turn with no player message.
    Periodic,
}

/// Runs AI turns until the player's pending messages are answered. Returns
/// false if the AI request failed.
async fn try_ai_turn(state: &AppState, kind: TurnKind) -> bool {
    let configuration = match state.ai.configurations.active() {
        Ok(Some(configuration)) => configuration,
        Ok(None) => {
            if kind != TurnKind::Periodic {
                state.notify("Select an AI configuration before sending a chat message.");
            }
            return kind == TurnKind::Periodic;
        }
        Err(error) => {
            state.notify(error);
            return false;
        }
    };
    let mut conversation = state.ai.conversation.lock().await;
    if kind != TurnKind::Periodic && conversation.pending_user_messages.is_empty() {
        state.notify("There is no failed AI request to retry.");
        return true;
    }
    loop {
        let prompt = conversation.build_prompt(&configuration.system_text);
        if prompt.omitted_report_count > 0 {
            state.notify(format!(
                "{} oldest pending report(s) were left out to fit the AI context limit.",
                prompt.omitted_report_count
            ));
        }
        match state.ai.complete(&configuration, &prompt).await {
            Ok(result) => {
                let uneventful = kind == TurnKind::Periodic && ai::is_no_comment(&result.answer);
                conversation.complete(prompt, result.answer.clone());
                if !uneventful {
                    let prefix = if kind == TurnKind::Periodic {
                        UNSOLICITED_PREFIX
                    } else {
                        "Companion: "
                    };
                    state.notify(format!("{prefix}{})", result.answer));
                    state.notify(format!("{AI_USAGE_PREFIX}{}", result.usage.summary()))
                }
                if kind == TurnKind::Periodic || conversation.pending_user_messages.is_empty() {
                    return true;
                }
            }
            Err(error) => {
                state.notify(format!(
                    "AI request failed. Your chat and collected reports are retained; automatic checks are paused until you send a message or retry. {error}"
                ));
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notification_history_replays_then_streams_live() {
        let (state, _queue_rx) = AppState::new();
        state.notify("first");
        state.notify("second");
        for _ in 0..50 {
            if state.history.subscribe(None).0.len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let (backlog, mut live) = state.history.subscribe(None);
        assert_eq!(backlog, [(1, "first".to_owned()), (2, "second".to_owned())]);
        state.notify("third");
        assert_eq!(live.recv().await.unwrap(), (3, "third".to_owned()));

        let (resumed, _) = state.history.subscribe(Some(2));
        assert_eq!(resumed, [(3, "third".to_owned())]);
    }
    use api::{
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
        let (state, mut queue_rx) = AppState::new();
        let (address, server_task) = start_test_grpc_server(state.clone()).await;

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
        let (state, _queue_rx) = AppState::new();
        let mut notifications = state.notifications_tx.subscribe();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            state.queue_tx.try_send(QueueItem::AiRetry).unwrap();
        }
        let (address, server_task) = start_test_grpc_server(state.clone()).await;
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
            .contains("queue is full"));

        server_task.abort();
    }

    #[tokio::test]
    async fn web_chat_input_is_enqueued_as_user_input() {
        let (state, mut queue_rx) = AppState::new();
        let response = crate::web::router(state)
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
        let (state, queue_rx) = AppState::new();
        let mut notifications = state.notifications_tx.subscribe();
        let queue_tx = state.queue_tx.clone();
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));
        for text in ["/call no_such_tool a=1", "/bogus"] {
            queue_tx
                .send(QueueItem::UserInput(text.to_owned()))
                .await
                .unwrap();
        }

        let result = notifications.recv().await.unwrap();
        assert!(result.contains("Unknown tool: no_such_tool"), "{result}");
        let error = notifications.recv().await.unwrap();
        assert!(error.contains("Unknown chat command: /bogus"), "{error}");
        assert!(state
            .ai
            .conversation
            .lock()
            .await
            .pending_user_messages
            .is_empty());
        worker.abort();
    }

    #[tokio::test]
    async fn web_chat_rejects_cross_origin_request() {
        let (state, mut queue_rx) = AppState::new();
        let response = crate::web::router(state)
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
        let (state, mut queue_rx) = AppState::new();
        let router = crate::web::router(state);
        let settings_response = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/fragments/ai-configurations")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "name=Profile&interaction_method=Anthropic&model_id=model&url=https%3A%2F%2Fapi.anthropic.com%2F&system_text=text&api_key=secret",
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
        let (state, _) = AppState::new();
        let router = crate::web::router(state.clone());
        let configurations = &state.ai.configurations;
        let create_body = |name: &str| {
            format!(
                r#"{{"name":"{name}","interaction_method":"OpenAI","model_id":"gpt-test","url":"https://api.openai.com/v1/","system_text":"Use the provided evidence."}}"#
            )
        };
        let create = |name: &str| {
            configurations
                .create(
                    name.to_owned(),
                    genai::adapter::AdapterKind::OpenAI,
                    "gpt-test".to_owned(),
                    "https://api.openai.com/v1/".to_owned(),
                    "Use the provided evidence.".to_owned(),
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
                    .uri(format!("/api/ai-configurations/{first_id}"))
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/json")
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
                    .uri(format!("/api/ai-configurations/{second_id}"))
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
        let delete_body = axum::body::to_bytes(delete.into_body(), 32_768)
            .await
            .unwrap();
        let delete_result: serde_json::Value = serde_json::from_slice(&delete_body).unwrap();
        assert!(delete_result["active_id"].is_null());
        assert_eq!(delete_result["configurations"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn configuration_management_page_is_served_separately() {
        let (state, _) = AppState::new();
        let response = crate::web::router(state)
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
        let (state, _) = AppState::new();
        let response = crate::web::router(state)
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
        let (state, queue_rx) = AppState::new();
        state
            .ai
            .configurations
            .create(
                "test config".to_owned(),
                genai::adapter::AdapterKind::OpenAI,
                "test-model".to_owned(),
                "https://api.openai.com/v1/".to_owned(),
                crate::ai::DEFAULT_SYSTEM_TEXT.to_owned(),
            )
            .unwrap();
        let mut notifications = state.notifications_tx.subscribe();
        state
            .queue_tx
            .try_send(QueueItem::GameEvent(GameEvent {
                id: None,
                date: "254-07-05:1061".to_owned(),
                report_type: None,
                category: "arrivals".to_owned(),
                text: "A caravan arrived.".to_owned(),
            }))
            .unwrap();
        state
            .queue_tx
            .try_send(QueueItem::UserInput("What happened?".to_owned()))
            .unwrap();
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));

        assert!(notifications
            .recv()
            .await
            .unwrap()
            .starts_with(GAME_EVENT_PREFIX));
        assert!(notifications
            .recv()
            .await
            .unwrap()
            .contains("AI request failed"));
        let conversation = state.ai.conversation.lock().await;
        assert_eq!(conversation.pending_reports.len(), 1);
        assert_eq!(
            conversation.pending_user_messages,
            vec!["What happened?".to_owned()]
        );
        drop(conversation);
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
        let (state, queue_rx) = AppState::new();
        state
            .ai
            .configurations
            .create(
                "test config".to_owned(),
                genai::adapter::AdapterKind::OpenAI,
                "test-model".to_owned(),
                "https://api.openai.com/v1/".to_owned(),
                crate::ai::DEFAULT_SYSTEM_TEXT.to_owned(),
            )
            .unwrap();
        let mut notifications = state.notifications_tx.subscribe();
        state
            .event_turns
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));
        state.queue_tx.send(sample_event()).await.unwrap();

        assert!(notifications
            .recv()
            .await
            .unwrap()
            .starts_with(GAME_EVENT_PREFIX));
        let started = Instant::now();
        let failure = notifications.recv().await.unwrap();
        assert!(failure.contains("AI request failed"), "{failure}");
        assert_eq!(started.elapsed(), EVENT_FLUSH_DELAY);

        // Automatic turns stay paused after the failure, however long we wait.
        let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
        assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
        assert_eq!(state.ai.conversation.lock().await.pending_reports.len(), 1);
        worker.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn automatic_turns_are_off_by_default() {
        let (state, queue_rx) = AppState::new();
        state
            .ai
            .configurations
            .create(
                "test config".to_owned(),
                genai::adapter::AdapterKind::OpenAI,
                "test-model".to_owned(),
                "https://api.openai.com/v1/".to_owned(),
                crate::ai::DEFAULT_SYSTEM_TEXT.to_owned(),
            )
            .unwrap();
        let mut notifications = state.notifications_tx.subscribe();
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));
        state.queue_tx.send(sample_event()).await.unwrap();
        assert!(notifications
            .recv()
            .await
            .unwrap()
            .starts_with(GAME_EVENT_PREFIX));
        let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
        assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
        worker.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_checks_stay_silent_without_an_ai_configuration() {
        let (state, queue_rx) = AppState::new();
        let mut notifications = state.notifications_tx.subscribe();
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));
        let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
        assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
        worker.abort();
    }

    async fn post_plugin_event(state: AppState, body: &'static str) -> axum::response::Response {
        crate::web::router(state)
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
        let (state, _) = AppState::new();
        for (body, events, periodic) in [
            ("event_turns=on", true, false),
            ("periodic_turns=on", false, true),
            ("event_turns=on&periodic_turns=on", true, true),
            ("", false, false),
        ] {
            let response = crate::web::router(state.clone())
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
            assert_eq!(state.event_turns_enabled(), events, "{body}");
            assert_eq!(state.periodic_turns_enabled(), periodic, "{body}");
        }
    }

    #[tokio::test]
    async fn main_page_prevents_caching_and_offers_the_event_toggle() {
        let (state, _) = AppState::new();
        let response = crate::web::router(state)
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
        let (state, _) = AppState::new();
        for path in ["/assets/htmx.min.js", "/assets/hx-sse.min.js"] {
            let response = crate::web::router(state.clone())
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
        let (state, mut queue_rx) = AppState::new();
        let response = post_plugin_event(state, EVENT_BODY).await;

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
        let (state, mut queue_rx) = AppState::new();
        let response = crate::web::router(state)
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
        let (state, mut queue_rx) = AppState::new();
        for body in [
            r#"{"date":"nope","category":"jobs","text":"bad date"}"#,
            r#"{"date":"254-07-05:1061","category":"","text":"no category"}"#,
            r#"{"date":"254-07-05:1061","category":"jobs","text":"  "}"#,
            r#"{"date":"254-07-05:1061","category":"jobs","text":"x","real_world_time_utc":1}"#,
        ] {
            let response = post_plugin_event(state.clone(), body).await;
            assert!(response.status().is_client_error(), "{body}");
        }
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn lua_http_event_reports_full_queue_without_accepting_event() {
        let (state, queue_rx) = AppState::new();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            state.queue_tx.try_send(QueueItem::AiRetry).unwrap();
        }

        let response = post_plugin_event(state, EVENT_BODY).await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(queue_rx.len(), EVENT_QUEUE_CAPACITY);
    }
    async fn start_test_grpc_server(
        state: AppState,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Server::builder()
            .add_service(CompanionServer::new(crate::rpc::service(state)))
            .serve_with_incoming(TcpListenerStream::new(listener));

        (address, tokio::spawn(server))
    }
}
