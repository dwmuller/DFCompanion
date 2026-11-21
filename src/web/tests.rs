use super::*;
use crate::app::{ChatState, CompanionApp};
use crate::game_state::LifecycleState;
use axum::body::to_bytes;
use tower::ServiceExt;

#[test]
fn list_items_maps_summaries_and_marks_the_active_configuration() {
    let items = vec![
        AiConfiguration {
            id: "test-id-1".to_owned(),
            name: "Test configuration 1".to_owned(),
            interaction_method: genai::adapter::AdapterKind::OpenAIResp,
            model_id: String::new(),
            url: String::new(),
            system_text: String::new(),
        },
        AiConfiguration {
            id: "test-id-2".to_owned(),
            name: "Test configuration 2".to_owned(),
            interaction_method: genai::adapter::AdapterKind::OpenAIResp,
            model_id: String::new(),
            url: String::new(),
            system_text: String::new(),
        },
    ]
    .into_iter()
    .map(|c| ConfigurationListItem::from_configuration(&c, false))
    .collect::<Vec<_>>();

    let list = templates::render_configuration_list(&items).expect("list should render");
    let selector = templates::render_active_configuration(&items, Some("test-id-1"), false, "")
        .expect("selector should render");

    assert!(list.contains("Test configuration 2"));
    assert!(list.contains("OpenAI"));
    assert!(selector.contains("value=\"test-id-1\" checked"));
    assert!(!selector.contains("value=\"test-id-2\" checked"));
}

#[tokio::test]
async fn conversation_markdown_endpoint_uses_all_messages() {
    let (app, _) = CompanionApp::new();
    for message in [
        DisplayMessage::new(DisplayMessageKind::User, "Question"),
        DisplayMessage::new(DisplayMessageKind::GameEvent, "Arrival"),
        DisplayMessage::new(DisplayMessageKind::Diagnostic, "/tools"),
        DisplayMessage::new(DisplayMessageKind::AiUsage, "2 requests"),
        DisplayMessage::new(DisplayMessageKind::Status, "Connected"),
        DisplayMessage::new(DisplayMessageKind::Warning, "Heads up"),
        DisplayMessage::new(DisplayMessageKind::Companion, "**Answer**"),
    ] {
        app.notifier().notify(message);
    }
    for _ in 0..50 {
        if app.history.subscribe(None).0.len() == 7 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let response = router(app)
        .oneshot(
            Request::builder()
                .uri("/conversation.md")
                .header(header::HOST, HTTP_HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        String::from_utf8(body.to_vec()).unwrap(),
        "**You:**\n\nQuestion\n\n**Game event:**\n\nArrival\n\n**Diagnostic:**\n\n/tools\n\n**AI usage:**\n\n2 requests\n\n**Status:**\n\nConnected\n\n**Warning:**\n\nHeads up\n\n**Companion:**\n\n**Answer**"
    );
}

#[tokio::test]
async fn resume_chat_requires_local_origin_and_clears_blocked_state() {
    let (app, queue_rx) = CompanionApp::new();
    let svc = router(app.clone());
    let status_response = svc
        .clone()
        .oneshot(
            Request::builder()
                .uri("/chat-state")
                .header(header::HOST, HTTP_HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_response.status(), StatusCode::OK);
    assert!(status_response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let mut status_events = status_response.into_body().into_data_stream();
    let initial_status =
        tokio::time::timeout(std::time::Duration::from_secs(2), status_events.next())
            .await
            .expect("initial chat status should be sent")
            .unwrap()
            .unwrap();
    assert!(!String::from_utf8_lossy(&initial_status).contains("AI processing is blocked"));

    let mut state_changes = app.subscribe_chat_state();
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    app.event_queue()
        .try_send(ExternalEvent::UserInput(
            "A request without an active configuration".to_owned(),
        ))
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if *state_changes.borrow_and_update() == ChatState::Blocked {
                break;
            }
            state_changes.changed().await.unwrap();
        }
    })
    .await
    .expect("chat should become blocked without an active configuration");

    let blocked_status =
        tokio::time::timeout(std::time::Duration::from_secs(2), status_events.next())
            .await
            .expect("updated chat status should be sent")
            .unwrap()
            .unwrap();
    assert!(String::from_utf8_lossy(&blocked_status).contains("AI processing is blocked"));

    let rejected = svc
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/chat/resume")
                .header(header::HOST, HTTP_HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert_eq!(app.chat_state(), ChatState::Blocked);

    let resumed = svc
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/chat/resume")
                .header(header::HOST, HTTP_HOST)
                .header(header::ORIGIN, HTTP_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.chat_state(), ChatState::Idle);
    let idle_status = tokio::time::timeout(std::time::Duration::from_secs(2), status_events.next())
        .await
        .expect("resumed chat status should be sent")
        .unwrap()
        .unwrap();
    assert!(!String::from_utf8_lossy(&idle_status).contains("AI processing is blocked"));
    worker.abort();
}

#[test]
fn replay_and_live_messages_render_identically_and_only_live_beeps() {
    let message = DisplayMessage::new(DisplayMessageKind::Companion, "**Hello**").with_beep(true);
    let replay = notification_events(0, 7, &message, false, false);
    let live = notification_events(0, 7, &message, true, false);

    assert_eq!(replay.len(), 1);
    assert_eq!(live.len(), 2);
    assert_eq!(format!("{:?}", replay[0]), format!("{:?}", live[0]));
}

#[test]
fn only_eligible_user_inputs_offer_a_rollback_button() {
    let input = DisplayMessage::new(DisplayMessageKind::User, "hello")
        .with_input_state(InputState::Pending);
    assert!(rollback_eligible(&input, 5, Some(NotificationId(5))));
    assert!(rollback_eligible(&input, 6, Some(NotificationId(5))));
    assert!(!rollback_eligible(&input, 4, Some(NotificationId(5))));
    assert!(!rollback_eligible(&input, 5, None));
    let companion = DisplayMessage::new(DisplayMessageKind::Companion, "hi");
    assert!(!rollback_eligible(&companion, 5, Some(NotificationId(5))));
}

#[test]
fn an_eligible_notification_renders_a_rollback_button_and_an_ineligible_one_does_not() {
    let message =
        DisplayMessage::new(DisplayMessageKind::User, "hello").with_input_state(InputState::Done);
    let eligible = templates::render_notification(1, &message, true).unwrap();
    assert!(eligible.contains("rollback-form"));
    assert!(eligible.contains("/rollback/1"));
    // `novalidate` stops htmx from validating the (possibly empty) required
    // editor it includes, which would abort the request.
    assert!(eligible.contains("novalidate"));
    let ineligible = templates::render_notification(1, &message, false).unwrap();
    assert!(!ineligible.contains("rollback-form"));
}

#[tokio::test]
async fn rollback_floor_events_streams_the_floor_as_a_named_event() {
    let (app, _queue_rx) = CompanionApp::new();
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/rollback-floor-events")
                .header(header::HOST, HTTP_HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(std::time::Duration::from_millis(500), stream.next())
        .await
        .expect("the initial floor should be sent")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&chunk).contains("event: rollback-floor"));
}

#[tokio::test]
async fn lifecycle_pushes_update_the_state_and_reject_browser_origins() {
    let (app, _) = CompanionApp::new();
    assert_eq!(app.lifecycle().state, LifecycleState::Unreachable);

    let accepted = router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugin/lifecycle")
                .header(header::HOST, HTTP_HOST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"world_loaded":true,"map_loaded":true,"fortress_name":"Bridgedmine"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(app.lifecycle().state, LifecycleState::MapLoaded);
    assert_eq!(
        app.lifecycle().fortress_name.as_deref(),
        Some("Bridgedmine")
    );

    let rejected = router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugin/lifecycle")
                .header(header::HOST, HTTP_HOST)
                .header(header::ORIGIN, HTTP_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"world_loaded":false,"map_loaded":false,"fortress_name":""}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert_eq!(app.lifecycle().state, LifecycleState::MapLoaded);
}

async fn get_events_body(app: &Arc<CompanionApp>, last_event_id: Option<&str>) -> String {
    let mut request = Request::builder()
        .uri("/events")
        .header(header::HOST, HTTP_HOST);
    if let Some(last_event_id) = last_event_id {
        request = request.header("last-event-id", last_event_id);
    }
    let response = router(app.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut body = String::new();
    // Collect whatever has arrived so far; the stream never ends on its
    // own, so a short timeout stands in for "nothing more is coming".
    while let Ok(Some(Ok(chunk))) =
        tokio::time::timeout(std::time::Duration::from_millis(200), stream.next()).await
    {
        body.push_str(&String::from_utf8_lossy(&chunk));
    }
    body
}

#[tokio::test]
async fn a_fresh_connection_gets_the_full_backlog_with_epoch_qualified_ids() {
    let (app, _queue_rx) = CompanionApp::new();
    app.notifier()
        .notify(DisplayMessage::new(DisplayMessageKind::Status, "first"));
    for _ in 0..50 {
        if app.history.subscribe(None).0.len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let body = get_events_body(&app, None).await;

    assert!(body.contains("id: 0:1"));
    assert!(body.contains("notif-1"));
    assert!(!body.contains("event: reset"));
}

#[tokio::test]
async fn reconnecting_within_the_same_epoch_resumes_after_the_given_id() {
    let (app, _queue_rx) = CompanionApp::new();
    app.notifier()
        .notify(DisplayMessage::new(DisplayMessageKind::Status, "first"));
    app.notifier()
        .notify(DisplayMessage::new(DisplayMessageKind::Status, "second"));
    for _ in 0..50 {
        if app.history.subscribe(None).0.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let body = get_events_body(&app, Some("0:1")).await;

    assert!(!body.contains("event: reset"));
    assert!(!body.contains("notif-1\""));
    assert!(body.contains("id: 0:2"));
    assert!(body.contains("notif-2"));
}

#[tokio::test]
async fn reconnecting_with_a_stale_epoch_gets_a_reset_then_the_full_backlog() {
    let (app, _queue_rx) = CompanionApp::new();
    let id = app
        .notifier()
        .notify(DisplayMessage::new(DisplayMessageKind::Status, "first"));
    for _ in 0..50 {
        if app.history.subscribe(None).0.len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    app.history.truncate_from(id);
    app.notifier().notify(DisplayMessage::new(
        DisplayMessageKind::Status,
        "after truncation",
    ));
    for _ in 0..50 {
        if app.history.subscribe(None).0.len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // This client remembers epoch 0's last id, which no longer exists.
    let body = get_events_body(&app, Some("0:1")).await;

    let reset_at = body
        .find("event: reset")
        .expect("a reset event should be sent");
    let backlog_at = body
        .find("after truncation")
        .expect("the full backlog should replay after the reset");
    assert!(reset_at < backlog_at);
    assert!(body.contains("id: 1:"));
}

#[tokio::test]
async fn a_live_input_state_change_is_sent_as_an_out_of_band_swap() {
    let (app, queue_rx) = CompanionApp::new();
    // An active (if unreachable) configuration, so the turn actually
    // starts and transitions the input to Processing instead of the
    // session blocking for lack of one.
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
    let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
    let request = Request::builder()
        .uri("/events")
        .header(header::HOST, HTTP_HOST)
        .body(Body::empty())
        .unwrap();
    let response = router(app.clone()).oneshot(request).await.unwrap();
    let mut stream = response.into_body().into_data_stream();

    app.event_queue()
        .try_send(ExternalEvent::UserInput("hello".to_owned()))
        .unwrap();

    let mut body = String::new();
    let found_state_change = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let chunk = stream.next().await.unwrap().unwrap();
            body.push_str(&String::from_utf8_lossy(&chunk));
            if body.contains("hx-swap-oob") {
                break;
            }
        }
    })
    .await;

    assert!(
        found_state_change.is_ok(),
        "expected an input-state swap; got: {body}"
    );
    assert!(body.contains("notif-state-1"));
    assert!(body.contains("data-input-state=\"processing\""));
    worker.abort();
}
