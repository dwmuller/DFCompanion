use std::time::{SystemTime, UNIX_EPOCH};

use prost_types::Timestamp;
use rand::RngCore;
use tokio::sync::{broadcast, mpsc};

pub mod api {
    tonic::include_proto!("dfcompanion.v1");
}
pub mod rpc;
pub mod web;

pub const EVENT_QUEUE_CAPACITY: usize = 50;
pub const USER_INPUT_CATEGORY: &str = "user_input";

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
}

impl AppState {
    pub fn new() -> (Self, mpsc::Receiver<QueueItem>) {
        let (queue_tx, queue_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let (notifications_tx, _) = broadcast::channel(128);

        (
            Self {
                queue_tx,
                notifications_tx,
                plugin_token: None,
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
    notifications_tx: broadcast::Sender<String>,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    async move {
        while let Some(item) = queue_rx.recv().await {
            let response = mock_ai_response(&item);
            let _ = notifications_tx.send(response);
        }
    }
}

fn mock_ai_response(item: &QueueItem) -> String {
    if item.category == USER_INPUT_CATEGORY {
        format!("Mock AI: I received your message: {}", item.text)
    } else {
        format!(
            "Mock AI: I received a {} event: {}",
            item.category, item.text
        )
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
