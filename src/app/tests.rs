use super::*;
use crate::chat_session::{EVENT_FLUSH_DELAY, PERIODIC_CHECK_INTERVAL};
use crate::events::{ExternalEvent, GameEvent};
use crate::notifications::{HistoryEvent, NotificationId};
use tokio::sync::broadcast;

/// Waits for the next new message (skipping state-change and reset events)
/// and returns its id and contents.
async fn recv_added(
    notifications: &mut broadcast::Receiver<HistoryEvent>,
) -> (u64, DisplayMessage) {
    loop {
        if let HistoryEvent::Added(id, message) = notifications.recv().await.unwrap() {
            return (id, message);
        }
    }
}

// Find next new message of a specific kind in the notifications stream.
async fn recv_kind(
    notifications: &mut broadcast::Receiver<HistoryEvent>,
    kind: DisplayMessageKind,
) -> DisplayMessage {
    loop {
        let (_, message) = recv_added(notifications).await;
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
async fn lifecycle_transitions_map_to_states_and_keep_the_fortress_name() {
    let (app, _queue_rx) = CompanionApp::new();
    assert_eq!(app.lifecycle().state, LifecycleState::Unreachable);

    app.update_lifecycle(false, false, Some(String::new()), None);
    assert_eq!(app.lifecycle().state, LifecycleState::Initialized);
    assert_eq!(app.lifecycle().fortress_name, None);

    app.update_lifecycle(true, false, None, None);
    assert_eq!(app.lifecycle().state, LifecycleState::WorldLoaded);

    app.update_lifecycle(true, true, Some("Bridgedmine".to_owned()), Some(41));
    assert_eq!(app.lifecycle().state, LifecycleState::MapLoaded);
    assert_eq!(
        app.lifecycle().fortress_name.as_deref(),
        Some("Bridgedmine")
    );
    assert_eq!(app.lifecycle().site_id, Some(41));

    // Unloading the map keeps the last fortress name.
    app.update_lifecycle(true, false, None, None);
    assert_eq!(app.lifecycle().state, LifecycleState::WorldLoaded);
    assert_eq!(
        app.lifecycle().fortress_name.as_deref(),
        Some("Bridgedmine")
    );
    assert_eq!(app.lifecycle().site_id, Some(41));

    // Losing DFHack keeps the name too.
    app.mark_lifecycle_unreachable();
    assert_eq!(app.lifecycle().state, LifecycleState::Unreachable);
    assert_eq!(
        app.lifecycle().fortress_name.as_deref(),
        Some("Bridgedmine")
    );
    assert_eq!(app.lifecycle().site_id, Some(41));
}

#[tokio::test]
async fn a_new_fortress_event_preempts_and_clears_the_full_normal_queue() {
    let (app, queue_rx) = CompanionApp::new();
    for i in 0..EVENT_QUEUE_CAPACITY {
        app.queue_tx
            .try_send(ExternalEvent::UserInput(format!("stale {i}")))
            .unwrap();
    }
    app.queue_tx
        .try_send_lifecycle(LifecycleEvent::Snapshot(LifecycleSnapshot {
            world_loaded: true,
            map_loaded: true,
            fortress_name: "Bridgedmine".to_owned(),
            site_id: Some(41),
        }))
        .unwrap();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));

    for _ in 0..100 {
        if app.lifecycle().state == LifecycleState::MapLoaded {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(
        (
            app.lifecycle().site_id,
            app.pending_user_message_count(),
            app.pending_report_count(),
        ),
        (Some(41), 0, 0)
    );

    app.queue_tx
        .try_send(ExternalEvent::UserInput(
            "pending in first session".to_owned(),
        ))
        .unwrap();
    for _ in 0..100 {
        if app.pending_user_message_count() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    app.queue_tx
        .try_send_lifecycle(LifecycleEvent::Snapshot(LifecycleSnapshot {
            world_loaded: true,
            map_loaded: false,
            fortress_name: String::new(),
            site_id: None,
        }))
        .unwrap();
    app.queue_tx
        .try_send_lifecycle(LifecycleEvent::Snapshot(LifecycleSnapshot {
            world_loaded: true,
            map_loaded: true,
            fortress_name: "Bridgedmine".to_owned(),
            site_id: Some(41),
        }))
        .unwrap();
    for _ in 0..100 {
        if app.lifecycle().state == LifecycleState::MapLoaded
            && app.pending_user_message_count() == 0
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        (app.lifecycle().state, app.pending_user_message_count()),
        (LifecycleState::MapLoaded, 0)
    );
    worker.abort();
}

#[test]
fn a_chat_restore_installs_valid_history_for_the_current_site() {
    let (app, _queue_rx) = CompanionApp::new();
    app.update_lifecycle(true, true, Some("Bridgedmine".to_owned()), Some(41));
    app.lifecycle_generation.store(3, Ordering::Relaxed);
    let saved = ai::History::with_test_summary("Build a northern wall.", &ai::History::default())
        .to_snapshot_json(41, "250-01-01:600")
        .unwrap();

    app.finish_chat_restore(
        3,
        41,
        Ok(Some(LoadedChatState {
            state: saved,
            current_game_date: "250-02-03:400".to_owned(),
        })),
    );

    assert_eq!(
        app.session.lock().unwrap().history().summary(),
        "Build a northern wall."
    );
}

#[test]
fn a_stale_chat_restore_cannot_replace_the_current_sites_history() {
    let (app, _queue_rx) = CompanionApp::new();
    app.update_lifecycle(true, true, Some("Bridgedmine".to_owned()), Some(41));
    app.lifecycle_generation.store(4, Ordering::Relaxed);
    let saved = ai::History::with_test_summary("Old site context.", &ai::History::default())
        .to_snapshot_json(41, "250-01-01:600")
        .unwrap();

    app.finish_chat_restore(
        3,
        41,
        Ok(Some(LoadedChatState {
            state: saved,
            current_game_date: "250-02-03:400".to_owned(),
        })),
    );

    assert!(app.session.lock().unwrap().history().is_empty());
}

#[test]
fn an_unloaded_site_restore_does_not_start_an_orientation_turn() {
    let (app, _queue_rx) = CompanionApp::new();
    let configuration = app
        .ai
        .configurations
        .create(
            "test config",
            genai::adapter::AdapterKind::OpenAI,
            "test-model",
            "http://localhost/",
            crate::ai::DEFAULT_SYSTEM_TEXT,
        )
        .unwrap();
    app.ai
        .configurations
        .set_active(Some(&configuration.id))
        .unwrap();
    app.update_lifecycle(false, false, Some("Bridgedmine".to_owned()), Some(41));
    app.lifecycle_generation.store(3, Ordering::Relaxed);

    app.finish_chat_restore(3, 41, Err("RPC error: Failure".to_owned()));

    assert_eq!(app.pending_user_message_count(), 0);
    assert_eq!(app.chat_state(), ChatState::Idle);
}

#[test]
fn user_event_effects_reach_notification_history() {
    use tracing_subscriber::prelude::*;

    let (app, _queue_rx) = CompanionApp::new();
    let layer = crate::user_events::UserEventLayer::new(app.notifier().clone());
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        app.perform(Effect::UserEvent(crate::user_events::UserEvent::new(
            crate::user_events::UserEventKind::Diagnostic,
            "from tracing",
        )));
    });

    let message = &app.history.subscribe_from(None).backlog[0].1;
    assert_eq!(message.kind, DisplayMessageKind::Diagnostic);
    assert_eq!(message.text, "from tracing");
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
        HistoryEvent::Added(5, DisplayMessage::new(DisplayMessageKind::Warning, "third"))
    );

    let (resumed, _) = app.history.subscribe(Some(4));
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].1.kind, DisplayMessageKind::Warning);
}

use axum::{
    body::Body,
    http::{header, Request as HttpRequest, StatusCode},
};
use tower::ServiceExt;

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
        ExternalEvent::UserInput("hello Companion".to_owned())
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
            .try_send(ExternalEvent::UserInput(text.to_owned()))
            .unwrap();
    }

    let call = recv_added(&mut notifier).await.1;
    assert_eq!(call.kind, DisplayMessageKind::Diagnostic);
    assert!(call.text.contains("/call no_such_tool"), "{call:?}");
    let error = recv_added(&mut notifier).await.1;
    assert_eq!(error.kind, DisplayMessageKind::Diagnostic);
    assert!(
        error.text.contains("Unknown tool: no_such_tool"),
        "{error:?}"
    );
    let call = recv_added(&mut notifier).await.1;
    assert_eq!(call.kind, DisplayMessageKind::Diagnostic);
    assert!(call.text.contains("/bogus"), "{call:?}");
    let error = recv_added(&mut notifier).await.1;
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
        .try_send(ExternalEvent::GameEvent(GameEvent {
            id: None,
            date: "250-01-01:0".to_owned(),
            report_type: None,
            category: "test".to_owned(),
            text: "A dwarf arrived.".to_owned(),
        }))
        .unwrap();
    let (event_id, event) = recv_added(&mut notifications).await;
    assert_eq!(event.kind, DisplayMessageKind::GameEvent);
    queue_tx
        .try_send(ExternalEvent::UserInput("hello".to_owned()))
        .unwrap();
    let (user_id, user) = recv_added(&mut notifications).await;
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
        .try_send(ExternalEvent::UserInput("hello".to_owned()))
        .unwrap();
    let (user_id, _) = recv_added(&mut notifications).await;

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
        .try_send(ExternalEvent::UserInput("hello".to_owned()))
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
async fn configuration_routes_reject_cross_origin_requests() {
    let (app, mut queue_rx) = CompanionApp::new();
    let router = crate::web::router(app);
    let settings_response = router
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/fragments/ai-configurations/new")
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::ORIGIN, "https://attacker.example")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "name=Profile&interaction_method=Anthropic&model_id=model&url=https%3A%2F%2Fapi.anthropic.com%2F&system_text=text&context_window_tokens=32768&max_output_tokens=8192",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(settings_response.status(), StatusCode::FORBIDDEN);
    assert!(queue_rx.try_recv().is_err());
}

#[tokio::test]
async fn rollback_route_rejects_cross_origin_requests() {
    let (app, _queue_rx) = CompanionApp::new();
    let router = crate::web::router(app);
    let response = router
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rollback/1")
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::ORIGIN, "https://attacker.example")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("message=hi"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rollback_recovers_a_queued_request_then_rejects_the_same_target() {
    let (app, queue_rx) = CompanionApp::new();
    // No active configuration, so the request stays queued instead of the AI
    // being asked anything.
    let mut notifications = app.history.subscribe(None).1;
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    app.queue_tx
        .try_send(ExternalEvent::UserInput("hello".to_owned()))
        .unwrap();

    let (id, _) = recv_added(&mut notifications).await;
    let editor = app
        .rollback(NotificationId(id), "typed".to_owned())
        .unwrap();
    assert_eq!(editor, "hello\n\n\ntyped");

    // The input is gone, so the same target is no longer eligible.
    assert!(app
        .rollback(NotificationId(id), "typed".to_owned())
        .is_err());
    worker.abort();
}

#[tokio::test]
async fn configuration_api_supports_rename_and_delete() {
    let (app, _) = CompanionApp::new();
    let router = crate::web::router(app.clone());
    let configurations = &app.ai.configurations;
    let create_body = |name: &str| {
        format!(
            "name={name}&interaction_method=OpenAI&model_id=gpt-test&url=https%3A%2F%2Fapi.openai.com%2Fv1%2F&system_text=Use+the+provided+evidence.&context_window_tokens=32768&max_output_tokens=8192"
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
    assert!(html.contains("AI Model Profiles"));
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
async fn chat_without_api_key_retains_pending_context_after_failure() {
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
        .try_send(ExternalEvent::GameEvent(GameEvent {
            id: None,
            date: "254-07-05:1061".to_owned(),
            report_type: None,
            category: "arrivals".to_owned(),
            text: "A caravan arrived.".to_owned(),
        }))
        .unwrap();
    app.queue_tx
        .try_send(ExternalEvent::UserInput("What happened?".to_owned()))
        .unwrap();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));

    assert!(recv_added(&mut notifications_rx).await.1.kind == DisplayMessageKind::GameEvent);
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

fn sample_event() -> ExternalEvent {
    ExternalEvent::GameEvent(GameEvent {
        id: None,
        date: "254-07-05:1061".to_owned(),
        report_type: None,
        category: "arrivals".to_owned(),
        text: "A caravan arrived.".to_owned(),
    })
}

#[tokio::test(start_paused = true)]
async fn events_trigger_an_ai_turn_after_the_flush_delay_then_pause_automatic_turns_on_failure() {
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

    assert!(recv_added(&mut notifications).await.1.kind == DisplayMessageKind::GameEvent);
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
    assert!(recv_added(&mut notifications).await.1.kind == DisplayMessageKind::GameEvent);
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

async fn post_plugin_event(app: Arc<CompanionApp>, body: &'static str) -> axum::response::Response {
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
    assert!(html.contains("show-ai-usage"));
    assert!(html.contains("show-status"));
    assert!(html.contains("show-diagnostic"));
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
        ExternalEvent::GameEvent(GameEvent {
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
    for i in 0..EVENT_QUEUE_CAPACITY {
        app.queue_tx
            .try_send(ExternalEvent::UserInput(format!("filler {i}")))
            .unwrap();
    }

    let response = post_plugin_event(app, EVENT_BODY).await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(queue_rx.len(), EVENT_QUEUE_CAPACITY);
}
