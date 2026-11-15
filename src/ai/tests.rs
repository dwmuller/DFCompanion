use super::*;
use crate::{
    ai_config::AiConfiguration,
    notifications::{NotificationHistory, Notifier},
};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;

#[derive(Clone)]
struct MockProviderState {
    responses: std::sync::Arc<Vec<Value>>,
    next_response: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    requests: tokio::sync::mpsc::UnboundedSender<(String, String, String, Value)>,
}

async fn mock_provider(
    State(state): State<MockProviderState>,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_owned();
    let auth_header = if request.headers().contains_key("authorization") {
        "authorization"
    } else if request.headers().contains_key("x-api-key") {
        "x-api-key"
    } else {
        ""
    }
    .to_owned();
    let auth = request
        .headers()
        .get("authorization")
        .or_else(|| request.headers().get("x-api-key"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let _ = state.requests.send((path, auth_header, auth, body));
    let index = state
        .next_response
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let response = state
        .responses
        .get(index)
        .or_else(|| state.responses.last())
        .cloned()
        .unwrap();
    (StatusCode::OK, axum::Json(response)).into_response()
}

async fn start_mock_provider(
    responses: Vec<Value>,
) -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<(String, String, String, Value)>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new()
        .fallback(any(mock_provider))
        .with_state(MockProviderState {
            responses: std::sync::Arc::new(responses),
            next_response: std::sync::Arc::default(),
            requests: requests_tx,
        });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/v1/"), requests_rx, task)
}

fn build_game_event(id: i64, text: &str) -> GameEvent {
    GameEvent {
        id: Some(id),
        date: "date".to_owned(),
        report_type: None,
        category: "test".to_owned(),
        text: text.to_owned(),
    }
}

/// A queue of inputs and a history, driven the way a session drives them.
#[derive(Default)]
struct Conversation {
    history: History,
    pending_input: VecDeque<AiInput>,
}

impl Conversation {
    fn push_report(&mut self, event: GameEvent, id: Option<NotificationId>) {
        self.pending_input
            .push_back(AiInput::GameReport { event, id });
    }

    fn push_user_message(&mut self, text: &str, id: Option<NotificationId>) {
        self.pending_input.push_back(AiInput::UserRequest {
            text: text.to_owned(),
            id,
        });
    }

    fn pending_through(&self) -> Option<NotificationId> {
        self.pending_input
            .iter()
            .rev()
            .find_map(AiInput::notification_id)
    }

    fn build_prompt(&self, system_text: &str) -> Option<Prompt> {
        build_prompt(&self.history, &self.pending_input, system_text)
    }
}

/// Completes `prompt` the way a successful turn that answered `answer` does.
fn finish_turn(conversation: &mut Conversation, prompt: Prompt, answer: &str) {
    conversation.history = history_after_turn(&conversation.history, &prompt, answer);
    conversation.pending_input.drain(..prompt.included_inputs);
}

#[test]
fn successful_chat_completion_clears_pending_inputs_of_same_type() {
    let mut conversation = Conversation::default();
    conversation.push_report(build_game_event(1, "just testing"), None);
    conversation.push_user_message("question", None);
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();

    finish_turn(&mut conversation, prompt, "answer");

    assert_eq!(conversation.pending_input.len(), 1);
    assert_eq!(conversation.history.messages().len(), 2);
}

#[test]
fn periodic_turn_without_a_player_message_asks_for_no_comment_when_idle() {
    let mut conversation = Conversation::default();
    conversation.push_report(build_game_event(1, "a report"), None);
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();

    assert_eq!(prompt.included_inputs, 1);
    assert!(prompt.content.contains("a report"));
    assert!(prompt.content.contains(NO_COMMENT));
    assert!(!prompt.content.contains("Player message"));
}

#[test]
fn report_then_no_comment_does_not_add_history() {
    let mut conversation = Conversation::default();
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT);
    assert!(prompt.is_none());

    conversation.push_report(build_game_event(1, "a report"), None);
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();
    finish_turn(&mut conversation, prompt, NO_COMMENT);
    assert_eq!(conversation.history.messages().len(), 0);
    assert!(conversation.pending_input.is_empty());
}

#[test]
fn turns_name_the_last_included_input_that_has_a_notification() {
    let mut conversation = Conversation::default();
    assert_eq!(conversation.pending_through(), None);
    conversation.push_report(build_game_event(1, "a report"), Some(NotificationId(1)));
    conversation.push_user_message("first", Some(NotificationId(2)));
    conversation.push_user_message("second", Some(NotificationId(4)));
    // The orientation prompt has no notification, so it never becomes the
    // watermark even when it is last.
    conversation.push_user_message("orient", None);
    assert_eq!(conversation.pending_through(), Some(NotificationId(4)));

    // No request leads the queue, so the first turn is an automated one for
    // the report alone.
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();
    assert_eq!(prompt.kind, PromptKind::Automated);
    assert_eq!(prompt.through, Some(NotificationId(1)));
    finish_turn(&mut conversation, prompt, NO_COMMENT);

    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();
    assert_eq!(prompt.kind, PromptKind::UserRequest);
    assert_eq!(prompt.included_inputs, 3);
    assert_eq!(prompt.through, Some(NotificationId(4)));
    finish_turn(&mut conversation, prompt, "answer");
    assert_eq!(conversation.pending_through(), None);
}

#[test]
fn a_turn_of_inputs_without_notifications_has_no_watermark() {
    let mut conversation = Conversation::default();
    conversation.push_user_message("orient", None);
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();
    assert_eq!(prompt.included_inputs, 1);
    assert_eq!(prompt.through, None);
}

#[test]
fn ids_never_appear_in_the_prompt_text() {
    let mut conversation = Conversation::default();
    conversation.push_report(
        build_game_event(7, "a report"),
        Some(NotificationId(987_654)),
    );
    let prompt = conversation.build_prompt(DEFAULT_SYSTEM_TEXT).unwrap();
    assert!(!prompt.content.contains("987654"));
    assert!(!prompt.content.contains("987_654"));
}

#[test]
fn no_comment_is_recognized_only_as_the_whole_answer() {
    assert!(is_no_comment("NO_COMMENT"));
    assert!(is_no_comment("  no_comment.\n"));
    assert!(!is_no_comment("NO_COMMENT, but the dwarves are starving"));
    assert!(!is_no_comment("The drink is low."));
}

fn test_prompt(kind: PromptKind, content: &str) -> Prompt {
    Prompt {
        kind,
        content: content.to_owned(),
        included_inputs: 1,
        through: None,
        token_estimates: AiPromptTokenEstimates::default(),
    }
}

#[test]
fn a_turn_returns_a_new_history_and_leaves_the_snapshot_untouched() {
    let before = History::default();
    let prompt = test_prompt(PromptKind::UserRequest, "question");
    let after = history_after_turn(&before, &prompt, "answer");
    assert_eq!(before.messages().len(), 0);
    assert_eq!(after.messages().len(), 2);
    assert_eq!(after.messages()[0].content, "question");
    assert_eq!(after.messages()[1].content, "answer");
}

#[test]
fn history_is_trimmed_from_the_front() {
    let mut history = History::default();
    let prompt = test_prompt(PromptKind::UserRequest, "q");
    for i in 0..MAX_HISTORY_MESSAGES {
        history = history_after_turn(&history, &prompt, &format!("a{i}"));
    }
    assert_eq!(history.messages().len(), MAX_HISTORY_MESSAGES);
    let last = history.messages().last().unwrap();
    assert_eq!(last.content, format!("a{}", MAX_HISTORY_MESSAGES - 1));
    assert_eq!(history.messages()[0].role, ChatRole::User);
}

#[test]
fn automated_no_comment_turns_leave_history_unchanged() {
    let before = history_after_turn(
        &History::default(),
        &test_prompt(PromptKind::UserRequest, "q"),
        "a",
    );
    let prompt = test_prompt(PromptKind::Automated, "periodic");
    let after = history_after_turn(&before, &prompt, NO_COMMENT);
    assert_eq!(after.messages().len(), 2);
    let after = history_after_turn(&before, &prompt, "Something happened.");
    assert_eq!(after.messages().len(), 4);
    // A user turn is remembered even when the answer is NO_COMMENT.
    let prompt = test_prompt(PromptKind::UserRequest, "q2");
    let after = history_after_turn(&before, &prompt, NO_COMMENT);
    assert_eq!(after.messages().len(), 4);
}

#[tokio::test]
async fn a_cancelled_turn_sends_nothing_and_reports_cancellation() {
    use genai::adapter::AdapterKind;

    let runtime = make_test_runtime();
    let (url, mut requests, server) = start_mock_provider(vec![json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "pong"},
            "finish_reason": "stop"
        }]
    })])
    .await;
    let configuration = AiConfiguration {
        id: "test-config".to_owned(),
        name: "Test".to_owned(),
        interaction_method: AdapterKind::OpenAI,
        model_id: "test-model".to_owned(),
        url,
        system_text: "Custom system instructions.".to_owned(),
    };
    let prompt = test_prompt(PromptKind::UserRequest, "Hello.");
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = runtime
        .run_turn_with_key(
            &configuration,
            "test-secret",
            &History::default(),
            &prompt,
            &cancel,
        )
        .await;
    let request_sent = requests.try_recv().is_ok();
    server.abort();

    assert!(matches!(result, Err(TurnError::Cancelled)));
    assert!(!request_sent);
}

#[tokio::test]
async fn cancelling_while_waiting_for_the_provider_ends_the_turn() {
    use genai::adapter::AdapterKind;

    let runtime = make_test_runtime();
    // A server that accepts the connection and never answers.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            if let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        }
    });
    let configuration = AiConfiguration {
        id: "test-config".to_owned(),
        name: "Test".to_owned(),
        interaction_method: AdapterKind::OpenAI,
        model_id: "test-model".to_owned(),
        url,
        system_text: "Custom system instructions.".to_owned(),
    };
    let prompt = test_prompt(PromptKind::UserRequest, "Hello.");
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        canceller.cancel();
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.run_turn_with_key(
            &configuration,
            "test-secret",
            &History::default(),
            &prompt,
            &cancel,
        ),
    )
    .await
    .expect("cancellation should end the turn well before the provider timeout");
    server.abort();

    assert!(matches!(result, Err(TurnError::Cancelled)));
}

fn make_test_runtime() -> AiRuntime {
    use crate::ai_config::ConfigurationRepository;
    let history = NotificationHistory::new();
    let notifier = Notifier::new(history);
    let game_state = GameState::new(notifier);
    AiRuntime::new(ConfigurationRepository::in_memory(), game_state)
}

#[tokio::test]
async fn genai_dispatches_to_each_configured_protocol_and_url() {
    use genai::adapter::AdapterKind;

    let runtime = make_test_runtime();
    let prompt = Prompt {
        kind: PromptKind::UserRequest,
        content: "Hello.".to_owned(),
        included_inputs: 1,
        through: None,
        token_estimates: AiPromptTokenEstimates::default(),
    };
    let responses = [
        (
            AdapterKind::OpenAI,
            json!({
                "id": "chatcmpl-test",
                "object": "chat.completion",
                "created": 1,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "pong"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
            "/v1/chat/completions",
            "authorization",
        ),
        (
            AdapterKind::OpenAIResp,
            json!({
                "id": "resp_test",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "model": "test-model",
                "output": [{
                    "id": "msg_test",
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "pong", "annotations": []}]
                }],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            }),
            "/v1/responses",
            "authorization",
        ),
        (
            AdapterKind::Anthropic,
            json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "model": "test-model",
                "content": [{"type": "text", "text": "pong"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }),
            "/v1/messages",
            "x-api-key",
        ),
    ];

    for (method, response, expected_path, expected_auth_header) in responses {
        let (url, mut requests, server) = start_mock_provider(vec![response]).await;
        let configuration = AiConfiguration {
            id: "test-config".to_owned(),
            name: "Test".to_owned(),
            interaction_method: method,
            model_id: "test-model".to_owned(),
            url,
            system_text: "Custom system instructions.".to_owned(),
        };
        let result = runtime
            .run_turn_with_key(
                &configuration,
                "test-secret",
                &History::default(),
                &prompt,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (path, auth_header, authorization, request_body) = requests.recv().await.unwrap();
        server.abort();

        assert_eq!(result.answer, "pong");
        assert_eq!(result.usage.len(), 1);
        assert_eq!(result.usage[0].prompt_tokens, Some(1));
        assert_eq!(result.usage[0].completion_tokens, Some(1));
        assert_eq!(result.usage[0].total_reported, Some(2));
        assert_eq!(path, expected_path);
        assert_eq!(auth_header, expected_auth_header);
        assert!(authorization.contains("test-secret"));
        assert!(request_body
            .to_string()
            .contains("Custom system instructions."));
    }
}

#[tokio::test]
async fn anthropic_tool_results_share_one_message() {
    use genai::adapter::AdapterKind;

    let runtime = make_test_runtime();
    let (url, mut requests, server) = start_mock_provider(vec![
        json!({
            "id": "msg_tools",
            "type": "message",
            "role": "assistant",
            "model": "test-model",
            "content": [
                {"type": "tool_use", "id": "call-1", "name": "unknown_a", "input": {}},
                {"type": "tool_use", "id": "call-2", "name": "unknown_b", "input": {}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 10}
        }),
        json!({
            "id": "msg_final",
            "type": "message",
            "role": "assistant",
            "model": "test-model",
            "content": [{"type": "text", "text": "done"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 150, "output_tokens": 20}
        }),
    ])
    .await;
    let configuration = AiConfiguration {
        id: "test-config".to_owned(),
        name: "Test".to_owned(),
        interaction_method: AdapterKind::Anthropic,
        model_id: "test-model".to_owned(),
        url,
        system_text: "Custom system instructions.".to_owned(),
    };
    let prompt = Prompt {
        kind: PromptKind::Automated,
        content: "Use tools.".to_owned(),
        included_inputs: 1,
        through: None,
        token_estimates: AiPromptTokenEstimates::default(),
    };

    let result = runtime
        .run_turn_with_key(
            &configuration,
            "test-secret",
            &History::default(),
            &prompt,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let _ = requests.recv().await.unwrap();
    let (_, _, _, second_request) = requests.recv().await.unwrap();
    server.abort();

    assert_eq!(result.answer, "done");
    assert!(result.usage.len() == 2);
    assert_eq!(result.usage[0].prompt_tokens, Some(100));
    assert_eq!(result.usage[0].completion_tokens, Some(10));
    assert_eq!(result.usage[0].total_reported, Some(110));
    assert_eq!(result.usage[1].prompt_tokens, Some(150));
    assert_eq!(result.usage[1].completion_tokens, Some(20));
    assert_eq!(result.usage[1].total_reported, Some(170));
    let messages = second_request["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "{second_request}");
    let results = messages[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|part| part["type"] == "tool_result"));
}

#[tokio::test]
async fn genai_tool_calls_are_dispatched_and_returned_to_the_model() {
    use genai::adapter::AdapterKind;

    let runtime = make_test_runtime();
    let (url, mut requests, server) = start_mock_provider(vec![
        json!({
            "id": "chatcmpl-tool",
            "object": "chat.completion",
            "created": 1,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-test",
                        "type": "function",
                        "function": {"name": "unknown_test_tool", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
        json!({
            "id": "chatcmpl-final",
            "object": "chat.completion",
            "created": 2,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "tool result received"},
                "finish_reason": "stop"
            }]
        }),
    ])
    .await;
    let configuration = AiConfiguration {
        id: "test-config".to_owned(),
        name: "Test".to_owned(),
        interaction_method: AdapterKind::OpenAI,
        model_id: "test-model".to_owned(),
        url,
        system_text: "Custom system instructions.".to_owned(),
    };
    let prompt = Prompt {
        kind: PromptKind::UserRequest,
        content: "Use a tool.".to_owned(),
        included_inputs: 1,
        through: None,
        token_estimates: AiPromptTokenEstimates::default(),
    };

    let result = runtime
        .run_turn_with_key(
            &configuration,
            "test-secret",
            &History::default(),
            &prompt,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let _ = requests.recv().await.unwrap();
    let (_, _, _, second_request) = requests.recv().await.unwrap();
    server.abort();

    assert_eq!(result.answer, "tool result received");
    assert_eq!(result.usage.len(), 2);
    assert_eq!(result.usage[0].prompt_tokens, None);
    assert_eq!(result.usage[0].completion_tokens, None);
    assert_eq!(result.usage[0].total_reported, None);
    assert!(second_request
        .to_string()
        .contains("Unknown tool: unknown_test_tool"));
}
