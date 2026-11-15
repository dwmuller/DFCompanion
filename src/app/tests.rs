use super::*;
use crate::events::{GameEvent, QueueItem};
use crate::notifications::NotificationId;
use crate::session::{EVENT_FLUSH_DELAY, PERIODIC_CHECK_INTERVAL};
use tokio::sync::broadcast;

// Find next message of specific kind in the notifications stream.
async fn recv_kind(
    notifications: &mut broadcast::Receiver<(u64, DisplayMessage)>,
    kind: DisplayMessageKind,
) -> DisplayMessage {
    loop {
        let message = notifications.recv().await.unwrap().1;
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
    let (app, _queue_rx) = CompanionApp::new();
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
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, _queue_rx) = CompanionApp::new();
    let (_, mut notifications_rx) = app.history.subscribe(None);
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
    assert!(notifications_rx
        .recv()
        .await
        .unwrap()
        .1
        .text
        .contains("queue is full"));

    server_task.abort();
}

#[tokio::test]
async fn web_chat_input_is_enqueued_as_user_input() {
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, queue_rx) = CompanionApp::new();
    let mut notifier = app.history.subscribe(None).1;
    let queue_tx = app.queue_tx.clone();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    for text in ["/call no_such_tool a=1", "/bogus"] {
        queue_tx
            .try_send(QueueItem::UserInput(text.to_owned()))
            .unwrap();
    }

    let call = notifier.recv().await.unwrap().1;
    assert_eq!(call.kind, DisplayMessageKind::Diagnostic);
    assert!(call.text.contains("/call no_such_tool"), "{call:?}");
    let error = notifier.recv().await.unwrap().1;
    assert_eq!(error.kind, DisplayMessageKind::Diagnostic);
    assert!(
        error.text.contains("Unknown tool: no_such_tool"),
        "{error:?}"
    );
    let call = notifier.recv().await.unwrap().1;
    assert_eq!(call.kind, DisplayMessageKind::Diagnostic);
    assert!(call.text.contains("/bogus"), "{call:?}");
    let error = notifier.recv().await.unwrap().1;
    assert_eq!(error.kind, DisplayMessageKind::Diagnostic);
    assert!(
        error.text.contains("Unknown chat command: /bogus"),
        "{error:?}"
    );
    assert!(app.pending_user_message_count() == 0);
    worker.abort();
}

#[tokio::test]
async fn pending_ai_inputs_carry_the_ids_of_their_notifications() {
    let (app, queue_rx) = CompanionApp::new();
    let mut notifications = app.history.subscribe(None).1;
    let queue_tx = app.queue_tx.clone();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    queue_tx
        .try_send(QueueItem::GameEvent(GameEvent {
            id: None,
            date: "250-01-01:0".to_owned(),
            report_type: None,
            category: "test".to_owned(),
            text: "A dwarf arrived.".to_owned(),
        }))
        .unwrap();
    let (event_id, event) = notifications.recv().await.unwrap();
    assert_eq!(event.kind, DisplayMessageKind::GameEvent);
    queue_tx
        .try_send(QueueItem::UserInput("hello".to_owned()))
        .unwrap();
    let (user_id, user) = notifications.recv().await.unwrap();
    assert_eq!(user.kind, DisplayMessageKind::User);

    // With no AI configuration the inputs stay pending, so they can be
    // matched to the notifications that display them. The newest one is the
    // watermark for everything pending.
    let mut through = None;
    for _ in 0..100 {
        through = app.pending_through();
        if through == Some(NotificationId(user_id)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(through, Some(NotificationId(user_id)));
    assert!(event_id < user_id);
    worker.abort();
}

/// The input state of each recorded notification that is an AI input.
fn input_states(app: &CompanionApp) -> Vec<(u64, InputState)> {
    app.history
        .subscribe(None)
        .0
        .into_iter()
        .filter_map(|(id, message)| message.input_state.map(|state| (id, state)))
        .collect()
}

#[tokio::test]
async fn inputs_are_pending_until_an_ai_turn_takes_them() {
    let (app, queue_rx) = CompanionApp::new();
    let mut notifications = app.history.subscribe(None).1;
    let queue_tx = app.queue_tx.clone();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    queue_tx
        .try_send(QueueItem::UserInput("hello".to_owned()))
        .unwrap();
    let (user_id, _) = notifications.recv().await.unwrap();

    // No configuration: the turn never starts, so nothing leaves Pending.
    let mut chat_state = app.subscribe_chat_state();
    chat_state
        .wait_for(|state| *state == ChatState::Blocked)
        .await
        .unwrap();
    assert_eq!(input_states(&app), vec![(user_id, InputState::Pending)]);
    worker.abort();
}

#[tokio::test]
async fn a_failed_turn_returns_its_inputs_to_pending() {
    use genai::adapter::AdapterKind;

    let (app, queue_rx) = CompanionApp::new();
    // No API key is saved for this configuration, so the turn starts and
    // then fails without any network traffic.
    let configuration = app
        .ai
        .configurations
        .create(
            "Failing",
            AdapterKind::Anthropic,
            "model",
            "https://api.anthropic.com/",
            "text",
        )
        .unwrap();
    app.ai
        .configurations
        .set_active(Some(&configuration.id))
        .unwrap();
    let queue_tx = app.queue_tx.clone();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    let mut chat_state = app.subscribe_chat_state();
    queue_tx
        .try_send(QueueItem::UserInput("hello".to_owned()))
        .unwrap();

    chat_state
        .wait_for(|state| *state == ChatState::Blocked)
        .await
        .unwrap();

    let states = input_states(&app);
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].1, InputState::Pending);
    assert_eq!(app.pending_through(), Some(NotificationId(states[0].0)));
    worker.abort();
}

#[tokio::test]
async fn web_chat_rejects_cross_origin_request() {
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, _) = CompanionApp::new();
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
    let (app, _) = CompanionApp::new();
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
    let (app, _) = CompanionApp::new();
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
    let (app, queue_rx) = CompanionApp::new();
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
    let (_, mut notifications_rx) = app.history.subscribe(None);
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

    assert!(notifications_rx.recv().await.unwrap().1.kind == DisplayMessageKind::GameEvent);
    let failure = recv_kind(&mut notifications_rx, DisplayMessageKind::Warning).await;
    if !failure.text.contains("AI request failed") {
        println!("Expected AI request failure, got: {:?}", failure);
    }
    assert!(failure.text.contains("AI request failed"));
    let conversation = app.pending_report_count();
    assert_eq!(conversation, 1);
    let conversation = app.pending_user_message_count();
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
    let (app, queue_rx) = CompanionApp::new();
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
    let mut notifications = app.history.subscribe(None).1;
    app.event_turns
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    app.queue_tx.try_send(sample_event()).unwrap();

    assert!(notifications.recv().await.unwrap().1.kind == DisplayMessageKind::GameEvent);
    let started = Instant::now();
    let failure = recv_kind(&mut notifications, DisplayMessageKind::Warning).await;
    assert!(failure.text.contains("AI request failed"), "{failure:?}");
    assert_eq!(started.elapsed(), EVENT_FLUSH_DELAY);

    // Automatic turns stay paused after the failure, however long we wait.
    let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
    assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
    assert_eq!(app.pending_report_count(), 1);
    worker.abort();
}

#[tokio::test(start_paused = true)]
async fn automatic_turns_are_off_by_default() {
    let (app, queue_rx) = CompanionApp::new();
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
    let mut notifications = app.history.subscribe(None).1;
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    app.queue_tx.try_send(sample_event()).unwrap();
    assert!(notifications.recv().await.unwrap().1.kind == DisplayMessageKind::GameEvent);
    let quiet = tokio::time::timeout(PERIODIC_CHECK_INTERVAL * 3, notifications.recv()).await;
    assert!(quiet.is_err(), "unexpected notification: {quiet:?}");
    worker.abort();
}

#[tokio::test(start_paused = true)]
async fn periodic_checks_stay_silent_without_an_ai_configuration() {
    let (app, queue_rx) = CompanionApp::new();
    let mut notifications = app.history.subscribe(None).1;
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
    let (app, _) = CompanionApp::new();
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
    let (app, _) = CompanionApp::new();
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
    let (app, _) = CompanionApp::new();
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
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, mut queue_rx) = CompanionApp::new();
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
    let (app, queue_rx) = CompanionApp::new();
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
