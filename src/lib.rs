use std::time::{SystemTime, UNIX_EPOCH};

use prost_types::Timestamp;
use rand::RngCore;
use tokio::sync::{broadcast, mpsc};

pub mod ai;
pub mod ai_config;
pub mod ai_providers;
pub mod ai_tools;
pub mod chat_commands;
pub mod game_state;

pub mod api {
    tonic::include_proto!("dfcompanion.v1");
}
pub mod rpc;
pub mod web;

pub const EVENT_QUEUE_CAPACITY: usize = 50;
pub const USER_INPUT_CATEGORY: &str = "user_input";
pub const AI_RETRY_CATEGORY: &str = "ai_retry";

#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    pub category: String,
    pub real_world_time_utc: Timestamp,
    pub game_world_time: Option<api::SimulationTimestamp>,
    pub text: String,
}

#[derive(Clone)]
pub struct AppState {
    pub queue_tx: mpsc::Sender<QueueItem>,
    pub notifications_tx: broadcast::Sender<String>,
    pub plugin_token: Option<String>,
    pub ai: std::sync::Arc<ai::AiRuntime>,
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

        (
            Self {
                queue_tx,
                notifications_tx,
                plugin_token: None,
                ai,
            },
            queue_rx,
        )
    }

    pub fn with_plugin_token(mut self, token: String) -> Self {
        self.plugin_token = Some(token);
        self
    }

    pub fn notify(&self, message: impl Into<String>) {
        // No connected browser is a normal state; notifications are best-effort.
        let _ = self.notifications_tx.send(message.into());
    }
}

pub fn generate_plugin_token() -> Result<String, rand::Error> {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn utc_now() -> Result<Timestamp, Box<dyn std::error::Error + Send + Sync>> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(Timestamp {
        seconds: i64::try_from(elapsed.as_secs())?,
        nanos: i32::try_from(elapsed.subsec_nanos())?,
    })
}

pub fn run_companion_worker(
    mut queue_rx: mpsc::Receiver<QueueItem>,
    state: AppState,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    async move {
        while let Some(item) = queue_rx.recv().await {
            match item.category.as_str() {
                AI_RETRY_CATEGORY => {
                    try_ai_turn(&state).await;
                }
                USER_INPUT_CATEGORY => {
                    if let Some(command) = chat_commands::ChatCommand::parse(&item.text) {
                        run_chat_command(&state, command).await;
                        continue;
                    }
                    state
                        .ai
                        .conversation
                        .lock()
                        .await
                        .pending_user_messages
                        .push(item.text);
                    try_ai_turn(&state).await;
                }
                _ => {
                    let dropped_reports = state.ai.conversation.lock().await.push_report(format!(
                        "{} at game time {}-{}-{} tick {}: {}",
                        item.category,
                        item.game_world_time
                            .as_ref()
                            .map(|time| time.year)
                            .unwrap_or_default(),
                        item.game_world_time
                            .as_ref()
                            .map(|time| time.month)
                            .unwrap_or_default(),
                        item.game_world_time
                            .as_ref()
                            .map(|time| time.day)
                            .unwrap_or_default(),
                        item.game_world_time
                            .as_ref()
                            .map(|time| time.tick)
                            .unwrap_or_default(),
                        item.text
                    ));
                    if dropped_reports > 0 {
                        state.notify(format!(
                            "{dropped_reports} oldest pending report(s) were discarded to keep the AI context within its input limit."
                        ));
                    }
                    state.notify(format!("DFHack report [{}]: {}", item.category, item.text));
                }
            }
        }
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
            state.notify(format!("Tool {name} result:\n{}", format_tool_result(&result)));
        }
        Ok(chat_commands::ChatCommand::ListTools) => state.notify(ai_tools::describe_tools()),
        Ok(chat_commands::ChatCommand::DescribeTool { name }) => {
            state.notify(ai_tools::describe_tool(&name).unwrap_or_else(|error| error))
        }
        Err(error) => state.notify(error),
    }
}

async fn try_ai_turn(state: &AppState) {
    let configuration = match state.ai.configurations.active() {
        Ok(Some(configuration)) => configuration,
        Ok(None) => {
            state.notify("Select an AI configuration before sending a chat message.");
            return;
        }
        Err(error) => {
            state.notify(error);
            return;
        }
    };
    let mut conversation = state.ai.conversation.lock().await;
    if conversation.pending_user_messages.is_empty() {
        state.notify("There is no failed AI request to retry.");
        return;
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
            Ok(answer) => {
                conversation.complete(prompt, answer.clone());
                state.notify(format!("Companion: {answer}"));
                if conversation.pending_user_messages.is_empty() {
                    return;
                }
            }
            Err(error) => {
                state.notify(format!(
                    "AI request failed. Your chat and collected reports are retained; correct the configuration or retry. {error}"
                ));
                return;
            }
        }
    }
}

pub fn validate_timestamp(timestamp: &Timestamp) -> bool {
    const MIN_TIMESTAMP_SECONDS: i64 = -62_135_596_800;
    const MAX_TIMESTAMP_SECONDS: i64 = 253_402_300_799;

    (MIN_TIMESTAMP_SECONDS..=MAX_TIMESTAMP_SECONDS).contains(&timestamp.seconds)
        && (0..1_000_000_000).contains(&timestamp.nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
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
                real_world_time_utc: Some(Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 123_000_000,
                }),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_234,
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

        let queued = queue_rx.recv().await.unwrap();
        assert_eq!(queued.category, "unit_death");
        assert_eq!(queued.text, "A dwarf was found dead.");
        assert_eq!(queued.real_world_time_utc.seconds, 1_800_000_000);
        assert_eq!(queued.real_world_time_utc.nanos, 123_000_000);
        assert_eq!(queued.game_world_time.unwrap().tick, 1_234);

        let reserved_category_error = client
            .submit_simulation_event(SimulationEvent {
                category: USER_INPUT_CATEGORY.to_owned(),
                real_world_time_utc: Some(Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 0,
                }),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_235,
                }),
                description: "must not trigger an AI request".to_owned(),
            })
            .await
            .unwrap_err();
        assert_eq!(reserved_category_error.code(), tonic::Code::InvalidArgument);
        assert!(queue_rx.try_recv().is_err());

        server_task.abort();
    }

    #[tokio::test]
    async fn full_event_queue_responds_with_pause_request_and_notifies_ui() {
        let (state, _queue_rx) = AppState::new();
        let mut notifications = state.notifications_tx.subscribe();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            state
                .queue_tx
                .try_send(QueueItem {
                    category: "test".to_owned(),
                    real_world_time_utc: Timestamp {
                        seconds: 1_800_000_000,
                        nanos: 0,
                    },
                    game_world_time: None,
                    text: "filling queue".to_owned(),
                })
                .unwrap();
        }
        let (address, server_task) = start_test_grpc_server(state.clone()).await;
        let mut client = CompanionClient::connect(format!("http://{address}"))
            .await
            .unwrap();

        let response = client
            .submit_simulation_event(SimulationEvent {
                category: "unit_death".to_owned(),
                real_world_time_utc: Some(Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 0,
                }),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_234,
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
    async fn web_chat_input_is_enqueued_with_user_category() {
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
        let item = queue_rx.recv().await.unwrap();
        assert_eq!(item.category, USER_INPUT_CATEGORY);
        assert_eq!(item.text, "hello Companion");
        assert!(validate_timestamp(&item.real_world_time_utc));
    }

    #[tokio::test]
    async fn worker_runs_chat_commands_without_involving_the_ai() {
        let (state, queue_rx) = AppState::new();
        let mut notifications = state.notifications_tx.subscribe();
        let queue_tx = state.queue_tx.clone();
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));
        for text in ["/call no_such_tool a=1", "/bogus"] {
            queue_tx
                .send(QueueItem {
                    category: USER_INPUT_CATEGORY.to_owned(),
                    real_world_time_utc: utc_now().unwrap(),
                    game_world_time: None,
                    text: text.to_owned(),
                })
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
                    .uri("/api/ai-configurations")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"name":"Profile","interaction_method":"Anthropic","model_id":"model","url":"https://api.anthropic.com/","system_text":"text","api_key":"secret"}"#,
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
    async fn configuration_api_supports_create_select_rename_and_delete() {
        let (state, _) = AppState::new();
        let router = crate::web::router(state);
        let create_body = |name: &str| {
            format!(
                r#"{{"name":"{name}","interaction_method":"OpenAI","model_id":"gpt-test","url":"https://api.openai.com/v1/","system_text":"Use the provided evidence."}}"#
            )
        };
        let create = |name: &str| {
            HttpRequest::builder()
                .method("POST")
                .uri("/api/ai-configurations")
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create_body(name)))
                .unwrap()
        };
        let first = router.clone().oneshot(create("Primary")).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), 32_768)
            .await
            .unwrap();
        let first_result: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let first_id = first_result["configurations"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(first_result["active_id"], first_id);
        assert_eq!(first_result["configurations"][0]["api_key_saved"], false);
        assert!(first_result["configurations"][0].get("api_key").is_none());

        let second = router.clone().oneshot(create("Secondary")).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = axum::body::to_bytes(second.into_body(), 32_768)
            .await
            .unwrap();
        let second_result: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
        let second_id = second_result["configurations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|configuration| configuration["name"] == "Secondary")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(second_result["active_id"], first_id);

        let select = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("PUT")
                    .uri("/api/ai-configurations/active")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"id":"{second_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(select.status(), StatusCode::OK);
        let select_body = axum::body::to_bytes(select.into_body(), 32_768)
            .await
            .unwrap();
        let select_result: serde_json::Value = serde_json::from_slice(&select_body).unwrap();
        assert_eq!(select_result["active_id"], second_id);

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
        assert!(html.contains("configuration-system-text"));
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
            .try_send(QueueItem {
                category: "dfhack_report".to_owned(),
                real_world_time_utc: Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 0,
                },
                game_world_time: None,
                text: "A caravan arrived.".to_owned(),
            })
            .unwrap();
        state
            .queue_tx
            .try_send(QueueItem {
                category: USER_INPUT_CATEGORY.to_owned(),
                real_world_time_utc: Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 0,
                },
                game_world_time: None,
                text: "What happened?".to_owned(),
            })
            .unwrap();
        let worker = tokio::spawn(run_companion_worker(queue_rx, state.clone()));

        assert!(notifications
            .recv()
            .await
            .unwrap()
            .contains("DFHack report"));
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

    #[tokio::test]
    async fn main_page_displays_the_plugin_command_and_prevents_token_caching() {
        let token = "ab".repeat(32);
        let (state, _) = AppState::new();
        let response = crate::web::router(state.with_plugin_token(token.clone()))
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
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(&format!("dfcompanion {token}")));
        assert!(html.contains("disable dfcompanion"));
        assert!(html.contains("Copy DFHack command"));
    }

    #[test]
    fn generated_plugin_tokens_are_256_bit_hex_strings() {
        let token = generate_plugin_token().unwrap();

        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn lua_http_event_is_authenticated_and_enqueued() {
        let (state, mut queue_rx) = AppState::new();
        let state = state.with_plugin_token("test-token-that-is-at-least-32-bytes".to_owned());
        let response = crate::web::router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/plugin/events")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::AUTHORIZATION, "Bearer test-token-that-is-at-least-32-bytes")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"category":"manual_test","real_world_time_utc":{"seconds":1800000000,"nanos":0},"game_world_time":{"year":252,"month":3,"day":7,"tick":1234},"description":"Lua HTTP test event."}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let queued = queue_rx.recv().await.unwrap();
        assert_eq!(queued.category, "manual_test");
        assert_eq!(queued.text, "Lua HTTP test event.");
        assert_eq!(queued.real_world_time_utc.seconds, 1_800_000_000);
        assert_eq!(queued.game_world_time.unwrap().tick, 1_234);
    }

    #[tokio::test]
    async fn lua_http_event_rejects_browser_and_unauthenticated_requests() {
        let (state, mut queue_rx) = AppState::new();
        let state = state.with_plugin_token("test-token-that-is-at-least-32-bytes".to_owned());
        let router = crate::web::router(state);
        let body = r#"{"category":"manual_test","real_world_time_utc":{"seconds":1800000000,"nanos":0},"game_world_time":{"year":252,"month":3,"day":7,"tick":1234},"description":"must not enqueue"}"#;

        let browser_response = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/plugin/events")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(
                        header::AUTHORIZATION,
                        "Bearer test-token-that-is-at-least-32-bytes",
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(browser_response.status(), StatusCode::FORBIDDEN);

        let unauthenticated_response = router
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
            .unwrap();
        assert_eq!(unauthenticated_response.status(), StatusCode::UNAUTHORIZED);
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn lua_http_event_cannot_submit_chat_or_retry_control_categories() {
        let (state, mut queue_rx) = AppState::new();
        let state = state.with_plugin_token("test-token-that-is-at-least-32-bytes".to_owned());
        for category in [USER_INPUT_CATEGORY, AI_RETRY_CATEGORY] {
            let response = crate::web::router(state.clone())
                .oneshot(
                    HttpRequest::builder()
                        .method("POST")
                        .uri("/api/plugin/events")
                        .header(header::HOST, "127.0.0.1:3000")
                        .header(header::AUTHORIZATION, "Bearer test-token-that-is-at-least-32-bytes")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(format!(
                            r#"{{"category":"{category}","real_world_time_utc":{{"seconds":1800000000,"nanos":0}},"game_world_time":{{"year":252,"month":3,"day":7,"tick":1234}},"description":"must not trigger AI"}}"#
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        assert!(queue_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn lua_http_event_reports_full_queue_without_accepting_event() {
        let (state, queue_rx) = AppState::new();
        let state = state.with_plugin_token("test-token-that-is-at-least-32-bytes".to_owned());
        for _ in 0..EVENT_QUEUE_CAPACITY {
            state
                .queue_tx
                .try_send(QueueItem {
                    category: "test".to_owned(),
                    real_world_time_utc: Timestamp {
                        seconds: 1_800_000_000,
                        nanos: 0,
                    },
                    game_world_time: None,
                    text: "filling queue".to_owned(),
                })
                .unwrap();
        }

        let response = crate::web::router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/plugin/events")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::AUTHORIZATION, "Bearer test-token-that-is-at-least-32-bytes")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"category":"manual_test","real_world_time_utc":{"seconds":1800000000,"nanos":0},"game_world_time":{"year":252,"month":3,"day":7,"tick":1234},"description":"queue full test"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

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
